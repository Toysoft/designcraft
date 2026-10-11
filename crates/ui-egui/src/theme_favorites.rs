//! Validated, persisted favorite interface themes and their original code-drawn star control.
use crate::theme::{Brightness, Tokens};
use egui::{Sense, Stroke, vec2};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Debug, Default, Serialize)]
#[serde(transparent)]
pub struct Favorites(Vec<String>);

impl<'de> Deserialize<'de> for Favorites {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(Self::from_value(&value))
    }
}

impl Favorites {
    pub fn from_value(value: &serde_json::Value) -> Self {
        let mut result = Self::default();
        if let Some(values) = value.as_array() {
            for id in values.iter().filter_map(|v| v.as_str()) {
                if let Some(b) = Brightness::parse(id) {
                    let id = b.id().to_string();
                    if !result.0.contains(&id) {
                        result.0.push(id);
                    }
                }
            }
        }
        result
    }
    pub fn contains(&self, b: Brightness) -> bool {
        self.0.iter().any(|id| id == b.id())
    }
    pub fn toggle(&mut self, b: Brightness) {
        if self.contains(b) {
            self.0.retain(|id| id != b.id());
        } else {
            self.0.push(b.id().to_string());
        }
    }
    pub fn themes(&self, light: bool) -> impl Iterator<Item = Brightness> + '_ {
        Brightness::ALL.into_iter().filter(move |b| self.contains(*b) && Tokens::for_brightness(*b).dark != light)
    }
}

/// Reserve the hit target even while its outline is hidden, so rows do not jump on hover.
pub fn star(ui: &mut egui::Ui, favorite: bool, row_hovered: bool, add_tip: &str, remove_tip: &str, theme_label: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(22.0, 20.0), Sense::click());
    if favorite || row_hovered || response.hovered() || response.has_focus() {
        let t = Tokens::get(ui.ctx());
        let color = if favorite { t.accent } else { t.text_dim };
        let center = rect.center();
        let points: Vec<_> = (0..10)
            .map(|i| {
                let angle = i as f32 * std::f32::consts::PI / 5.0 - std::f32::consts::FRAC_PI_2;
                center + vec2(angle.cos(), angle.sin()) * if i % 2 == 0 { 7.0 } else { 3.2 }
            })
            .collect();
        if favorite {
            let mut mesh = egui::Mesh::default();
            for pair in points.iter().zip(points.iter().cycle().skip(1)).take(10) {
                let start = mesh.vertices.len() as u32;
                for pos in [center, *pair.0, *pair.1] {
                    mesh.colored_vertex(pos, color);
                }
                mesh.add_triangle(start, start + 1, start + 2);
            }
            ui.painter().add(egui::Shape::mesh(mesh));
        }
        ui.painter().add(egui::Shape::closed_line(points, Stroke::new(1.2, color)));
    }
    let tip = format!("{}: {theme_label}", if favorite { remove_tip } else { add_tip });
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), favorite, &tip));
    response.on_hover_text(tip)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn favorites_reject_unknown_duplicate_and_malformed_ids() {
        let favorites: Favorites = serde_json::from_str(r#"["light","unknown","light",7,"dark"]"#).unwrap();
        assert_eq!(serde_json::to_value(&favorites).unwrap(), serde_json::json!(["light", "dark"]));
        let mut restored: Favorites = serde_json::from_value(serde_json::to_value(&favorites).unwrap()).unwrap();
        restored.toggle(Brightness::Light);
        assert!(!restored.contains(Brightness::Light));
        assert_eq!(restored.themes(false).collect::<Vec<_>>(), vec![Brightness::Dark]);
        assert!(serde_json::from_str::<Favorites>("null").unwrap().0.is_empty());
    }
    #[test]
    fn star_click_uses_its_own_hit_target() {
        let ctx = egui::Context::default();
        let mut favorite = false;
        let mut rect = egui::Rect::NOTHING;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.horizontal(|ui| {
                let _ = ui.radio(false, "Theme");
                rect = star(ui, favorite, true, "Add to favorites", "Remove from favorites", "Theme").rect;
            });
        });
        output.textures_delta.clear();
        let pos = rect.center();
        for pressed in [true, false] {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events: vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() },
                    ],
                    ..Default::default()
                },
                |ui| {
                    ui.horizontal(|ui| {
                        assert!(!ui.radio(false, "Theme").clicked());
                        if star(ui, favorite, true, "Add to favorites", "Remove from favorites", "Theme").clicked() {
                            favorite = !favorite;
                        }
                    });
                },
            );
            output.textures_delta.clear();
        }
        assert!(favorite);
    }
}
