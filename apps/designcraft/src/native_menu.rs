//! macOS: DesignCraft's menu tree as the native system menu bar (like InDesign on the Mac).
//! Items dispatch through the same command path as the in-window menus; enablement and check
//! marks are refreshed a few times per second.

use std::collections::HashMap;
use std::str::FromStr;

use designcraft_ui_egui::DesignApp;
use designcraft_ui_egui::menus::{self, Item};
use muda::accelerator::Accelerator;
use muda::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use serde_json::Value;

enum Handle {
    Plain(MenuItem),
    Check(CheckMenuItem),
}

pub struct NativeMenu {
    _menu: Menu,
    items: HashMap<String, (String, Value, Handle)>,
    last_refresh: f64,
    /// File › Open Recent, rebuilt when the list of recent files changes.
    recent: Option<RecentMenu>,
}

struct RecentMenu {
    submenu: Submenu,
    /// The files shown, and the native items standing for them (with their trailing separator).
    shown: Vec<String>,
    ids: Vec<String>,
    separator: Option<PredefinedMenuItem>,
}

/// Accelerator for "Cmd+Shift+]" (modifier-less shortcuts stay in the app so typing works).
fn accel(sc: &str) -> Option<Accelerator> {
    if !(sc.contains("Cmd") || sc.contains("Ctrl") || sc.contains("Alt")) {
        return None;
    }
    Accelerator::from_str(&sc.replace("Cmd", "CMD").replace("Alt", "ALT").replace("Shift", "SHIFT").replace("Ctrl", "CTRL")).ok()
}

impl NativeMenu {
    pub fn install(app: &mut DesignApp) -> Self {
        let menu = Menu::new();
        let mut items = HashMap::new();
        let mut counter = 0usize;
        let mut recent = None;
        // Application menu.
        let app_menu = Submenu::new("DesignCraft", true);
        // Our own About splash (with the community links) instead of the stock panel.
        let about = MenuItem::with_id("dc-about", "About DesignCraft", true, None);
        let discord = MenuItem::with_id("dc-discord", "Join the ArtCraft Discord…", true, None);
        // Not the predefined Quit: it terminates the app at once, past the unsaved-changes prompt.
        let quit = MenuItem::with_id("dc-quit", designcraft_ui_egui::i18n::tr(&app.ui.language, "Quit DesignCraft"), true, accel("Cmd+Q"));
        let _ = app_menu.append_items(&[
            &about,
            &discord,
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::services(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::hide(None),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::show_all(None),
            &PredefinedMenuItem::separator(),
            &quit,
        ]);
        items.insert("dc-about".into(), ("help.about".into(), serde_json::Value::Null, Handle::Plain(about)));
        items.insert("dc-discord".into(), ("help.discord".into(), serde_json::Value::Null, Handle::Plain(discord)));
        items.insert("dc-quit".into(), ("app.quit".into(), serde_json::Value::Null, Handle::Plain(quit)));
        let _ = menu.append(&app_menu);
        for (title, entries) in menus::menu_tree() {
            let sub = Submenu::new(designcraft_ui_egui::i18n::tr(&app.ui.language, title), true);
            build(app, &sub, &entries, &mut items, &mut counter, title, &mut recent);
            let _ = menu.append(&sub);
        }
        menu.init_for_nsapp();
        app.native_menu = true;
        app.native_shortcuts = items
            .values()
            .filter_map(|(id, p, _)| {
                let sc = menus::ui_label(id).and_then(|(_, sc)| sc).or_else(|| designcraft_engine::find_command(id).and_then(|c| c.shortcut))?;
                (p.is_null() && accel(sc).is_some()).then(|| id.clone())
            })
            .collect();
        let recent = recent.map(|submenu| RecentMenu { submenu, shown: Vec::new(), ids: Vec::new(), separator: None });
        Self { _menu: menu, items, last_refresh: 0.0, recent }
    }

    /// Put the current recent files at the top of File › Open Recent when they changed.
    fn refresh_recent(&mut self, app: &DesignApp) {
        let Some(r) = self.recent.as_mut() else { return };
        if r.shown == app.session.prefs.recent_files {
            return;
        }
        for id in r.ids.drain(..) {
            if let Some((_, _, Handle::Plain(item))) = self.items.remove(&id) {
                let _ = r.submenu.remove(&item);
            }
        }
        if let Some(sep) = r.separator.take() {
            let _ = r.submenu.remove(&sep);
        }
        let mut position = 0;
        for it in menus::recent_items(app) {
            match it {
                Item::Cmd { label, id, params, .. } => {
                    let mid = format!("dc-recent{position}");
                    let item = MenuItem::with_id(mid.clone(), label, true, None);
                    let _ = r.submenu.insert(&item, position);
                    self.items.insert(mid.clone(), (id, params, Handle::Plain(item)));
                    r.ids.push(mid);
                }
                Item::Sep => {
                    let sep = PredefinedMenuItem::separator();
                    let _ = r.submenu.insert(&sep, position);
                    r.separator = Some(sep);
                }
                Item::Sub(..) => continue,
            }
            position += 1;
        }
        r.shown = app.session.prefs.recent_files.clone();
    }

    pub fn poll(&mut self, app: &mut DesignApp, ctx: &egui::Context) {
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if let Some((cmd, params, _)) = self.items.get(ev.id.as_ref()) {
                menus::activate_native(app, ctx, &cmd.clone(), &params.clone());
            }
        }
        let now = designcraft_ui_egui::now_ms();
        if now - self.last_refresh < 250.0 {
            return;
        }
        self.last_refresh = now;
        self.refresh_recent(app);
        for (cmd, params, h) in self.items.values() {
            let en = menus::native_menu_enabled(app, ctx, cmd);
            match h {
                Handle::Plain(i) => i.set_enabled(en),
                Handle::Check(c) => {
                    c.set_enabled(en);
                    c.set_checked(menus::checked(app, cmd, params).unwrap_or(false));
                }
            }
        }
    }
}

fn build(
    app: &DesignApp,
    parent: &Submenu,
    entries: &[Item],
    items: &mut HashMap<String, (String, Value, Handle)>,
    counter: &mut usize,
    path: &str,
    recent: &mut Option<Submenu>,
) {
    for e in entries {
        match e {
            Item::Sep => {
                let _ = parent.append(&PredefinedMenuItem::separator());
            }
            Item::Sub(label, children) => {
                let sub = Submenu::new(designcraft_ui_egui::i18n::tr(&app.ui.language, label), true);
                let path = format!("{path}/{label}");
                build(app, &sub, children, items, counter, &path, recent);
                if path == menus::OPEN_RECENT {
                    *recent = Some(sub.clone());
                }
                let _ = parent.append(&sub);
            }
            Item::Cmd { label, id, params, shortcut } => {
                *counter += 1;
                let mid = format!("dc{counter}");
                let sc = if params.is_null() { shortcut.and_then(accel) } else { None };
                let label = designcraft_ui_egui::i18n::tr(&app.ui.language, label);
                let handle = if menus::checked(app, id, params).is_some() {
                    let c = CheckMenuItem::with_id(mid.clone(), label, true, false, sc);
                    let _ = parent.append(&c);
                    Handle::Check(c)
                } else {
                    let i = MenuItem::with_id(mid.clone(), label, true, sc);
                    let _ = parent.append(&i);
                    Handle::Plain(i)
                };
                items.insert(mid, (id.clone(), params.clone(), handle));
            }
        }
    }
}
