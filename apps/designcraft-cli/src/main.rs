//! Headless DesignCraft.
//!
//! ```text
//! designcraft-cli run [--in FILE | --sample] [--cmd COMMAND[=PARAMS]]... [--page N] [--scale S] [--pdf-options JSON] [--export OUT]... [--all-pages DIR]
//! designcraft-cli commands [FILTER]          # one line per command: id, label, parameters
//! designcraft-cli describe COMMAND           # one command, its parameters spelled out
//! designcraft-cli script [FILE|-] [--in FILE | --sample] [--connect PORT] [--save OUT] [--export OUT]... [--keep-going]
//! designcraft-cli app [--port PORT] COMMAND [PARAMS] | --method METHOD [PARAMS]
//! designcraft-cli mcp [--connect PORT] [--sample]
//! designcraft-cli perf [--pages N] [--frames N] [--chars N] [--images N] [--runs N] [--strict]
//! designcraft-cli bench FILE [--runs N]
//! designcraft-cli validate-pdf FILE.pdf
//! designcraft-cli links
//! designcraft-cli help [SUBCOMMAND]          # also SUBCOMMAND --help
//! designcraft-cli --version
//! ```
//!
//! The help pages in [`help`] explain each subcommand to an agent; every answer and error is a
//! sentence it can read as is.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
use std::process::ExitCode;

/// `println!` that ends the program quietly when stdout is closed (`designcraft-cli commands |
/// head`) instead of panicking with "failed printing to stdout: Broken pipe (os error 32)".
macro_rules! outln {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        if let Err(e) = writeln!(std::io::stdout(), $($arg)*) {
            $crate::stdout_failed(e);
        }
    }};
}

mod help;
mod perf;

use designcraft_engine::{Session, find_command};
use serde_json::{Value, json};

/// The links line under the usage block.
fn usage_footer() -> String {
    format!(
        "\nCommunity: {}  ·  {}  ·  {}",
        designcraft_engine::links::DISCORD,
        designcraft_engine::links::APP_PAGE,
        designcraft_engine::links::GITHUB
    )
}

/// stdout went away. A reader that stopped early (a closed pipe) ends the program quietly, as
/// ripgrep does; any other write error is reported.
fn stdout_failed(e: std::io::Error) -> ! {
    if e.kind() == std::io::ErrorKind::BrokenPipe {
        std::process::exit(0);
    }
    eprintln!("designcraft-cli: can't write to stdout: {e}");
    std::process::exit(1);
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    let asks_help = rest.iter().any(|a| a == "--help" || a == "-h");
    if let Some(page) = args.first().filter(|_| asks_help).and_then(|n| help::page(n)) {
        outln!("{page}");
        return ExitCode::SUCCESS;
    }
    match args.first().map(String::as_str) {
        Some("--version" | "-V" | "version") => {
            outln!("designcraft-cli {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("run") => report(run(rest)),
        Some("commands") => {
            let filter = args.get(1).map(|f| f.to_lowercase());
            for c in designcraft_engine::command_specs() {
                let text = format!("{} {} {}", c.id, c.label, c.menu.join(" ")).to_lowercase();
                if filter.as_ref().is_some_and(|f| !text.contains(f.as_str())) {
                    continue;
                }
                match c.params.strip_prefix("{}") {
                    Some(answer) => outln!("{} — {}{answer}", c.id, c.label),
                    None => outln!("{} — {}: {}", c.id, c.label, c.params),
                }
            }
            ExitCode::SUCCESS
        }
        Some("describe") => report(describe(args.get(1).map(String::as_str))),
        Some("script") => report(script(rest)),
        Some("app") => report(app(rest)),
        Some("mcp") => report(mcp(rest)),
        Some("perf") => report(perf::perf(rest)),
        Some("bench") => report(perf::bench(rest)),
        Some("validate-pdf") => report(validate_pdf(args.get(1).map(String::as_str))),
        Some("links") => {
            use designcraft_engine::links::*;
            outln!("Discord   {DISCORD}\nWebsite   {WEBSITE}\nApp page  {APP_PAGE}\nGitHub    {GITHUB}\nIssues    {ISSUES}");
            ExitCode::SUCCESS
        }
        Some("--help" | "-h" | "help") => match rest.first() {
            None => {
                outln!("{}", help::overview());
                outln!("{}", usage_footer());
                ExitCode::SUCCESS
            }
            Some(name) => match help::page(name) {
                Some(page) => {
                    outln!("{page}");
                    ExitCode::SUCCESS
                }
                None => unknown_subcommand(name),
            },
        },
        Some(other) => unknown_subcommand(other),
        None => {
            eprintln!("{}", help::overview());
            eprintln!("{}", usage_footer());
            ExitCode::FAILURE
        }
    }
}

fn unknown_subcommand(name: &str) -> ExitCode {
    eprintln!("designcraft-cli: `{name}` is not a subcommand.\n");
    eprintln!("{}", help::overview());
    eprintln!("{}", usage_footer());
    ExitCode::FAILURE
}

/// `json` shortened for an error message.
fn shown(json: &str) -> String {
    match json.char_indices().nth(200) {
        Some((i, _)) => format!("{}…", json.get(..i).unwrap_or_default()),
        None => json.to_string(),
    }
}

/// The parameters of command `id` from the JSON text an agent typed, or what is wrong with them.
fn params(id: &str, json: &str) -> Result<Value, String> {
    let takes = find_command(id).map(|c| format!(" `{id}` takes {}.", c.params)).unwrap_or_default();
    serde_json::from_str(json).map_err(|e| format!("parameters `{}`, which are not JSON: {e}.{takes}", shown(json)))
}

fn report(r: Result<(), String>) -> ExitCode {
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("designcraft-cli: {e}");
            ExitCode::FAILURE
        }
    }
}

fn validate_pdf(path: Option<&str>) -> Result<(), String> {
    let path = path.ok_or("validate-pdf takes the PDF to check: designcraft-cli validate-pdf FILE.pdf")?;
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let issues = designcraft_pdf::check_pdfx4(&bytes);
    let valid = issues.is_empty();
    let report = json!({"path": path, "standard": "PDF/X-4", "valid": valid, "issues": issues});
    outln!("{}", serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?);
    if valid { Ok(()) } else { Err(format!("{path} failed PDF/X-4 validation")) }
}

/// `mcp` (headless, in-process engine) or `mcp --connect PORT|HOST:PORT` (drive a running app
/// started with `designcraft --control PORT`). JSON-RPC on stdin/stdout; logs on stderr.
fn mcp(args: &[String]) -> Result<(), String> {
    use designcraft_mcp::{Backend, Headless, Remote, Server, control_addr};
    let mut connect: Option<String> = None;
    let mut sample = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--connect" => connect = Some(it.next().cloned().ok_or("--connect needs a port or host:port")?),
            "--sample" => sample = true,
            other => return Err(format!("mcp was given `{other}`, which it doesn't take. mcp takes --connect PORT and --sample.")),
        }
    }
    let backend: Box<dyn Backend> = match connect {
        Some(c) => {
            let addr = control_addr(&c);
            Box::new(
                Remote::connect(&addr)
                    .map_err(|e| format!("cannot connect to the DesignCraft app at {addr}: {e} (start it with `designcraft --control PORT`)"))?,
            )
        }
        None => {
            let mut h = Headless::with_document();
            if sample {
                h.session.execute("file.newSample", &json!({})).map_err(|e| e.to_string())?;
            }
            Box::new(h)
        }
    };
    eprintln!("designcraft-cli: MCP server on stdio ({})", backend.describe());
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    Server::new(backend).serve(stdin.lock(), stdout.lock()).map_err(|e| e.to_string())
}

/// The first `--page`, `--scale` or `--pdf-options` with no `--export` or `--all-pages` after it.
/// They apply to the exports that follow them, so one after the last export would do nothing.
fn option_after_last_export(args: &[String]) -> Option<&str> {
    let mut unused = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--page" | "--scale" | "--pdf-options" => {
                unused = unused.or(Some(a.as_str()));
                it.next();
            }
            "--export" | "--all-pages" => {
                unused = None;
                it.next();
            }
            "--in" | "--cmd" => {
                it.next();
            }
            _ => {}
        }
    }
    unused
}

const RUN_TAKES: &str = "run takes --in FILE or --sample, --cmd COMMAND=PARAMS (any number), --page N, --scale S, --pdf-options JSON, --export OUT (any number) and --all-pages DIR.";

fn run(args: &[String]) -> Result<(), String> {
    if let Some(opt) = option_after_last_export(args) {
        return Err(format!(
            "{opt} has no --export or --all-pages after it; options apply to the exports that follow them, so put it before its --export"
        ));
    }
    let cmds: Vec<&str> = args.iter().enumerate().filter(|(_, a)| *a == "--cmd").filter_map(|(i, _)| args.get(i + 1)).map(String::as_str).collect();
    let total = cmds.len();
    let at = |n: usize| format!("--cmd {} of {total}", n + 1);
    let steps = cmds
        .iter()
        .enumerate()
        .map(|(n, c)| {
            let (id, json) = c.split_once('=').unwrap_or((c, ""));
            if find_command(id).is_none() {
                let read = if json.is_empty() { "no parameters".to_string() } else { format!("parameters `{}`", shown(json)) };
                let like = match designcraft_engine::cmd::similar_commands(id).as_slice() {
                    [] => String::new(),
                    like => format!(" Commands named like it: {}.", like.join(", ")),
                };
                return Err(format!(
                    "{} was read as command `{id}` with {read}. `{id}` is not a command: --cmd takes COMMAND=PARAMS, a command id, then `=`, \
                     then a JSON object, e.g. --cmd 'frame.create={{\"rect\":[72,72,300,200]}}'.{like} Nothing ran.",
                    at(n)
                ));
            }
            let p = if json.is_empty() {
                json!({})
            } else {
                params(id, json).map_err(|e| format!("{} was read as command `{id}` with {e} Nothing ran.", at(n)))?
            };
            Ok((id.to_string(), p))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut s = Session::new();
    let mut page = 0usize;
    let mut scale = 1.0f64;
    let mut it = args.iter().enumerate();
    let mut opened = false;
    let mut pdf_opts = json!({});
    let mut results: Vec<Value> = Vec::new();
    while let Some((i, a)) = it.next() {
        let mut val = || it.next().map(|(_, v)| v.clone()).ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--in" => {
                let p = val()?;
                s.execute("file.open", &json!({"path": p})).map_err(|e| e.to_string())?;
                opened = true;
            }
            "--sample" => {
                s.execute("file.newSample", &json!({})).map_err(|e| e.to_string())?;
                opened = true;
            }
            "--cmd" => {
                if !opened {
                    s.execute("file.new", &json!({})).map_err(|e| e.to_string())?;
                    opened = true;
                }
                val()?;
                let n = results.len();
                let Some((id, p)) = steps.get(n) else { break };
                let stopped = |e: String| {
                    let unwritten: Vec<String> = args
                        .get(i + 2..)
                        .unwrap_or_default()
                        .windows(2)
                        .filter(|w| w[0] == "--export" || w[0] == "--all-pages")
                        .map(|w| format!(" {} was not written.", w[1]))
                        .collect();
                    format!("{}: {}. Nothing after it ran.{}", at(n), e.trim_end_matches('.'), unwritten.concat())
                };
                let p = designcraft_engine::script::resolve(p, &results).map_err(stopped)?;
                let r = s.call(id, &p).map_err(|e| stopped(e.to_string()))?;
                match &r {
                    Value::String(said) => outln!("{id} → {said}"),
                    r => outln!("{id} → {}", serde_json::to_string(r).unwrap_or_default()),
                }
                results.push(r);
            }
            "--page" => {
                let v = val()?;
                page = v.parse().map_err(|_| format!("--page takes a page number counted from 0, not `{v}`"))?;
            }
            "--scale" => {
                let v = val()?;
                scale = v.parse().map_err(|_| format!("--scale takes pixels per point, e.g. 2, not `{v}`"))?;
            }
            "--pdf-options" => {
                let v = val()?;
                pdf_opts = serde_json::from_str(&v).map_err(|e| {
                    format!(
                        "--pdf-options was given `{}`, which is not JSON: {e}. It takes the parameters of file.exportPdf as a JSON object.",
                        shown(&v)
                    )
                })?;
            }
            "--export" => {
                let out = val()?;
                if out.ends_with(".epub") {
                    let r = s.execute("file.exportEpub", &json!({"path": out})).map_err(|e| e.to_string())?;
                    eprintln!("wrote {out} ({} bytes)", r["bytes"]);
                } else if out.ends_with(".pdf") {
                    let mut p = pdf_opts.clone();
                    p["path"] = json!(out);
                    let r = s.execute("file.exportPdf", &p).map_err(|e| e.to_string())?;
                    eprintln!("wrote {out} ({} pages, {} bytes)", r["pages"], r["bytes"]);
                    for w in r["warnings"].as_array().into_iter().flatten() {
                        eprintln!("warning: {}", w.as_str().unwrap_or_default());
                    }
                } else {
                    export(&mut s, &out, page, scale)?;
                }
            }
            "--all-pages" => {
                let dir = val()?;
                let n = s.doc().map_err(|e| e.to_string())?.doc.page_count();
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                for p in 0..n {
                    export(&mut s, &format!("{dir}/page-{:03}.png", p + 1), p, scale)?;
                }
            }
            other => return Err(format!("run was given `{other}`, which it doesn't take. {RUN_TAKES}")),
        }
    }
    Ok(())
}

fn export(s: &mut Session, out: &str, page: usize, scale: f64) -> Result<(), String> {
    if out.to_ascii_lowercase().ends_with(".idml") {
        let r = s.execute("file.exportIdml", &json!({"path": out})).map_err(|e| e.to_string())?;
        eprintln!("wrote {out} ({} bytes)", r["bytes"]);
        return Ok(());
    }
    if out.ends_with(".designcraft") {
        s.execute("file.saveAs", &json!({"path": out})).map_err(|e| e.to_string())?;
        eprintln!("saved {out}");
        return Ok(());
    }
    let st = s.doc().map_err(|e| e.to_string())?;
    let mut r = designcraft_render::Renderer::new();
    let t = std::time::Instant::now();
    let img = r
        .render_page(&st.doc, &s.cache, page, scale, true, &designcraft_render::RenderOptions { printing_only: true, ..Default::default() })
        .ok_or_else(|| format!("no page {page} to export; pages are counted from 0 and run 0–{}", st.doc.page_count().saturating_sub(1)))?;
    let bytes = if out.ends_with(".jpg") || out.ends_with(".jpeg") { img.to_jpeg(90) } else { img.to_png() };
    designcraft_engine::atomic::write_atomic(std::path::Path::new(out), &bytes).map_err(|e| format!("{out}: {e}"))?;
    eprintln!("wrote {out} ({}×{}, {:.1} ms, {} glyphs)", img.width, img.height, t.elapsed().as_secs_f64() * 1000.0, r.stats.glyphs);
    Ok(())
}

fn describe(id: Option<&str>) -> Result<(), String> {
    use designcraft_engine::cmd::{Param, parse_params};
    let id = id.ok_or("describe takes a command id: designcraft-cli describe COMMAND. `designcraft-cli commands` lists them.")?;
    let c = find_command(id).ok_or_else(|| designcraft_engine::EngineError::UnknownCommand(id.into()).to_string())?;
    let mut head = format!("{}: {}", c.id, c.label);
    if !c.menu.is_empty() {
        head.push_str(&format!(", in {}", c.menu.join(" ▸ ")));
    }
    if let Some(k) = c.shortcut {
        head.push_str(&format!(", shortcut {k}"));
    }
    outln!("{head}.\n");
    let (args, answer) = c.params.split_once('→').unwrap_or((c.params, ""));
    let parsed = parse_params(c.params);
    match &parsed {
        Err(_) => outln!("Parameters: {}", args.trim()),
        Ok(p) if p.is_empty() => outln!("Takes no parameters."),
        Ok(p) => print_params(p),
    }
    if let Some(acts) = c.acts_on() {
        outln!("{acts}");
    }
    let remark = remark(args);
    if !remark.is_empty() {
        outln!("Note: {remark}");
    }
    if !answer.trim().is_empty() {
        outln!("Answers with {}.", answer.trim());
    }
    let bare = parsed.as_ref().is_ok_and(|p| p.iter().all(|p: &Param| p.optional));
    let call = if bare { c.id.to_string() } else { format!("'{}={{…}}'", c.id) };
    outln!("\nRun it with: designcraft-cli run --cmd {call}");
    Ok(())
}

/// The text after a params doc's parameters: `{} — a remark` or `{a, b} (a remark)`.
fn remark(args: &str) -> &str {
    let tail = args.rsplit_once('}').map(|(_, t)| t).unwrap_or_default().trim();
    let tail = tail.strip_prefix('—').unwrap_or(tail).trim();
    match tail.strip_prefix('(').and_then(|t| t.strip_suffix(')')) {
        Some(inner) if !inner.contains('(') => inner.trim(),
        _ => tail,
    }
}

fn print_params(params: &[designcraft_engine::cmd::Param]) {
    let name =
        |p: &designcraft_engine::cmd::Param| format!("{}{}{}", if p.or { "or " } else { "" }, p.name, if p.optional { ", optional" } else { "" });
    let width = params.iter().map(|p| name(p).chars().count()).max().unwrap_or_default();
    let grouped = params.iter().any(|p| p.object > 0);
    outln!("Parameters:");
    let mut object = usize::MAX;
    for p in params {
        if grouped && p.object != object {
            outln!("  {}", if p.object == 0 { "either:" } else { "or:" });
            object = p.object;
        }
        let indent = if grouped { "    " } else { "  " };
        let n = name(p);
        let pad = " ".repeat(width.saturating_sub(n.chars().count()));
        if p.about.is_empty() { outln!("{indent}{n}") } else { outln!("{indent}{n}{pad}  {}", p.about) }
    }
}

/// `script`: run a command script headless (or in the running app with `--connect`).
fn script(args: &[String]) -> Result<(), String> {
    use designcraft_mcp::{Backend, Headless, Remote, control_addr};
    let mut file: Option<String> = None;
    let mut setup: Vec<(String, Value)> = Vec::new();
    let mut connect: Option<String> = None;
    let mut save: Option<String> = None;
    let mut export: Vec<String> = Vec::new();
    let mut keep_going = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--in" => {
                let p = val()?;
                let cmd = if p.to_lowercase().ends_with(".idml") { "file.openIdml" } else { "file.open" };
                setup.push((cmd.into(), json!({"path": p})));
            }
            "--sample" => setup.push(("file.newSample".into(), json!({}))),
            "--connect" => connect = Some(val()?),
            "--save" => save = Some(val()?),
            "--export" => export.push(val()?),
            "--keep-going" => keep_going = true,
            f if !f.starts_with("--") && file.is_none() => file = Some(f.to_string()),
            other => {
                return Err(format!(
                    "script was given `{other}`, which it doesn't take. script takes FILE or -, --in FILE or --sample, --connect PORT, --save OUT, --export OUT (any number) and --keep-going."
                ));
            }
        }
    }
    let text = match file.as_deref() {
        None | Some("-") => {
            let mut t = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut t).map_err(|e| e.to_string())?;
            t
        }
        Some(f) => std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?,
    };
    let steps = designcraft_engine::script::parse(&text)?;
    let mut backend: Box<dyn Backend> = match &connect {
        Some(c) => {
            let addr = control_addr(c);
            Box::new(
                Remote::connect(&addr)
                    .map_err(|e| format!("cannot connect to the DesignCraft app at {addr}: {e} (start it with `designcraft --control PORT`)"))?,
            )
        }
        None => Box::new(Headless::with_document()),
    };
    let b = &mut *backend;
    let exec = |b: &mut dyn Backend, id: &str, p: Value| b.call("engine.execute", json!({"command": id, "params": p}));
    for (c, p) in &setup {
        exec(b, c, p.clone())?;
    }
    let report = if keep_going {
        // Run every step; failures become {"error": …} results.
        let mut results: Vec<Value> = Vec::new();
        for st in &steps {
            let r = designcraft_engine::script::resolve(&st.params, &results).and_then(|p| exec(b, &st.command, p));
            results.push(r.unwrap_or_else(|e| json!({"error": e, "command": st.command})));
        }
        designcraft_engine::script::Report { results, failed: None }
    } else {
        designcraft_engine::script::run(&steps, |id, p| exec(b, id, p))
    };
    if report.failed.is_none() {
        if let Some(out) = &save {
            exec(b, "file.saveAs", json!({"path": out}))?;
        }
        for out in &export {
            let lower = out.to_lowercase();
            let p = json!({"path": out});
            if lower.ends_with(".pdf") {
                exec(b, "file.exportPdf", p)?;
            } else if lower.ends_with(".idml") {
                exec(b, "file.exportIdml", p)?;
            } else if lower.ends_with(".epub") {
                exec(b, "file.exportEpub", p)?;
            } else {
                // PNG / JPEG of the first page.
                b.call("app.export", p)?;
            }
        }
    }
    outln!("{}", serde_json::to_string_pretty(&report.to_json()).unwrap_or_default());
    match report.failed {
        Some((i, c, e)) if save.is_some() || !export.is_empty() => {
            Err(format!("step {i} of {} ({c}) failed: {}. Nothing was saved or exported.", steps.len(), e.trim_end_matches('.')))
        }
        Some((i, c, e)) => Err(format!("step {i} of {} ({c}) failed: {e}", steps.len())),
        None => Ok(()),
    }
}

/// `app`: one command or control-channel method in the running app.
fn app(args: &[String]) -> Result<(), String> {
    use designcraft_mcp::{Backend, Remote, control_addr};
    let mut port = "7979".to_string();
    let mut method: Option<String> = None;
    let mut rest: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--port" | "--connect" => port = it.next().cloned().ok_or("--port needs a value")?,
            "--method" => method = Some(it.next().cloned().ok_or("--method needs a value")?),
            _ => rest.push(a.clone()),
        }
    }
    let addr = control_addr(&port);
    let mut r = Remote::connect(&addr)
        .map_err(|e| format!("cannot connect to the DesignCraft app at {addr}: {e} (start it with `designcraft --control {port}`)"))?;
    let out = match method {
        Some(m) => {
            let p = rest.first().map_or(Ok(json!({})), |t| params(&m, t).map_err(|e| format!("`{m}` was given {e}")))?;
            r.call(&m, p)?
        }
        None => {
            let cmd = rest.first().ok_or("app takes a command id: designcraft-cli app COMMAND [PARAMS]. `designcraft-cli commands` lists them.")?;
            let p = rest.get(1).map_or(Ok(json!({})), |t| params(cmd, t).map_err(|e| format!("`{cmd}` was given {e}")))?;
            r.call("engine.execute", json!({"command": cmd, "params": p}))?
        }
    };
    match out {
        Value::String(said) => outln!("{said}"),
        out => outln!("{}", serde_json::to_string_pretty(&out).unwrap_or_default()),
    }
    Ok(())
}
