//! The agent-facing command line: scripts with references, describe, filtered command lists.

use std::io::Write;
use std::process::{Command, Stdio};

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_designcraft-cli"))
}

#[test]
fn script_from_stdin_with_references_saves_and_exports() {
    let dir = std::env::temp_dir().join(format!("dc-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (doc, png) = (dir.join("a.designcraft"), dir.join("a.png"));
    let mut child = cli()
        .args(["script", "-", "--save", doc.to_str().unwrap(), "--export", png.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            b"# comment\nframe.create {\"rect\": [72, 72, 300, 200], \"content\": \"text\", \"text\": \"Hi\"}\nstory.get {\"story\": \"$0.story\"}\n",
        )
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["completed"], 2);
    assert_eq!(v["results"][1]["text"], "Hi");
    assert!(doc.exists() && png.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn failing_script_exits_nonzero_with_the_step() {
    let mut child = cli().args(["script", "-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"document.inspect\nno.such.command\n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["failedIndex"], 1);
}

#[test]
fn run_cmd_references_and_describe() {
    let out = cli()
        .args([
            "run",
            "--cmd",
            r#"frame.create={"rect":[72,72,300,200],"content":"text","text":"Hello"}"#,
            "--cmd",
            r#"story.get={"story":"$0.story"}"#,
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\nstory.get → {") && stdout.contains("\"text\":\"Hello\""), "{stdout}");
    let out = cli().args(["describe", "footnote.insert"]).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("footnote.insert: Insert Footnote, in Type."));
    let out = cli().args(["commands", "footnote."]).output().unwrap();
    let list = String::from_utf8_lossy(&out.stdout);
    assert!(list.lines().count() > 1 && list.lines().all(|l| l.starts_with("footnote.")), "{list}");
}

const SUBCOMMANDS: [&str; 10] = ["run", "commands", "describe", "script", "app", "mcp", "perf", "bench", "validate-pdf", "links"];

fn words(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Asking for help is not an error: it goes to stdout and succeeds (#243); an unknown subcommand
/// is named before the overview on stderr and fails.
#[test]
fn help_lists_every_subcommand_with_a_run_example() {
    for flag in ["--help", "-h", "help"] {
        let out = cli().arg(flag).output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{flag}: {:?}", out.status);
        assert!(stdout.starts_with("designcraft-cli works on DesignCraft documents"), "{flag}: {stdout:?}");
        for sub in SUBCOMMANDS {
            assert!(stdout.contains(&format!("\n  {sub} ")), "{flag} lacks {sub}: {stdout}");
        }
        assert!(stdout.contains("designcraft-cli run --cmd 'frame.create={"), "{stdout}");
        assert!(out.stderr.is_empty(), "{flag} wrote to stderr: {}", String::from_utf8_lossy(&out.stderr));
    }
    for args in [vec!["frobnicate"], vec!["help", "frobnicate"], vec!["--no-such-option"]] {
        let out = cli().args(&args).output().unwrap();
        assert!(!out.status.success(), "{args:?}: {:?}", out.status);
        assert!(out.stdout.is_empty(), "unexpected stdout: {}", String::from_utf8_lossy(&out.stdout));
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("is not a subcommand.\n\ndesigncraft-cli works on"), "{args:?}: {err}");
    }
}

#[test]
fn every_subcommand_answers_help_and_does_nothing_else() {
    let dir = std::env::temp_dir().join(format!("dc-cli-help-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for sub in SUBCOMMANDS {
        for args in [vec![sub, "--help"], vec![sub, "-h", "out.png"], vec!["help", sub]] {
            let out = cli().args(&args).current_dir(&dir).output().unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
            assert!(stdout.starts_with(&format!("usage: designcraft-cli {sub}")), "{args:?}: {stdout}");
        }
    }
    assert!(std::fs::read_dir(&dir).unwrap().next().is_none(), "help wrote a file");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn run_help_says_what_the_ids_do_not() {
    let out = cli().args(["run", "--help"]).output().unwrap();
    let help = words(&String::from_utf8_lossy(&out.stdout));
    for fact in [
        "COMMAND is a command id, then `=`, then the command's parameters as one JSON object",
        "A rect is [x0, y0, x1, y1]: two corners, not a width and height",
        "Pages are counted from 0 in parameters",
        "frame.create selects the frame it creates",
        "selection.set {\"ids\": [...]} selects objects",
        "\"story\": \"$0.story\" passes 9",
        "If a command fails, run stops there",
    ] {
        assert!(help.contains(fact), "missing: {fact}\n{help}");
    }
}

#[test]
fn describe_spells_out_the_parameters() {
    let out = cli().args(["describe", "frame.create"]).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "frame.create: Create Frame.

Parameters:
  spread, optional
  rect                 [x0,y0,x1,y1] (spread coords)
  shape, optional      rectangle|ellipse|polygon
  content, optional    graphic|text|unassigned
  sides, optional      3–100 (polygon; default 6)
  starInset, optional  0–100 (% for a star polygon)
  text, optional       string
  caret, optional      bool
  vertical, optional   bool (text: a new vertical story)

Run it with: designcraft-cli run --cmd 'frame.create={…}'
"
    );
    let out = cli().args(["describe", "object.fill"]).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "object.fill: Fill.

Parameters:
  swatch          (a swatch's name, e.g. [Paper])
  tint, optional  0..1
  ids, optional
Acts on the objects in `ids`, else on the selected objects.

Run it with: designcraft-cli run --cmd 'object.fill={…}'
"
    );
    let out = cli().args(["describe", "file.newSample"]).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "file.newSample: Sample Document, in Help.

Takes no parameters.
Note: a multi-page magazine sample

Run it with: designcraft-cli run --cmd file.newSample
"
    );
    let out = cli().args(["describe", "xref.insert"]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("  anchor, optional\n  or paragraph, optional"), "{text}");
    let out = cli().args(["describe", "frame.crate"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stderr), "designcraft-cli: unknown command `frame.crate`; commands named like it: frame.create\n");
}

#[test]
fn commands_lists_one_line_per_command() {
    let out = cli().args(["commands", "object.fill"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "object.fill — Fill: {swatch (a swatch's name, e.g. [Paper]), tint?: 0..1, ids?}\n");
    let out = cli().args(["commands", "sample document"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "file.newSample — Sample Document — a multi-page magazine sample\n");
}

#[test]
fn a_label_before_the_command_is_read_back_to_the_agent() {
    let out = cli().args(["run", "--cmd", r#"create=frame.create={"rect":[36,36,300,200]}"#]).output().unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.starts_with(
            "designcraft-cli: --cmd 1 of 1 was read as command `create` with parameters `frame.create={\"rect\":[36,36,300,200]}`. `create` is not a command: --cmd takes COMMAND=PARAMS, a command id, then `=`, then a JSON object, e.g. --cmd 'frame.create={\"rect\":[72,72,300,200]}'. Commands named like it: frame.create, "
        ) && err.ends_with(". Nothing ran.\n"),
        "{err}"
    );
    let out = cli().args(["run", "--cmd", r#"frame.create={"rect":[36,36,300,200],}"#]).output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.starts_with(
            "designcraft-cli: --cmd 1 of 1 was read as command `frame.create` with parameters `{\"rect\":[36,36,300,200],}`, which are not JSON: "
        ),
        "{err}"
    );
    assert!(err.contains(". `frame.create` takes {spread?, rect: [x0,y0,x1,y1] (spread coords), ") && err.ends_with("}. Nothing ran.\n"), "{err}");
}

#[test]
fn a_failing_command_says_which_and_what_was_not_written() {
    let dir = std::env::temp_dir().join(format!("dc-cli-fail-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("a.png");
    let out = cli()
        .args([
            "run",
            "--cmd",
            r#"frame.create={"rect":[36,36,300,200]}"#,
            "--cmd",
            r#"frame.create={"rct":[0,0,1,1]}"#,
            "--export",
            png.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("frame.create → {\"id\":"));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        format!(
            "designcraft-cli: --cmd 2 of 2: `frame.create` was given `rct`, which it doesn't take. `frame.create` takes {}. Nothing after it ran. {} was not written.\n",
            designcraft_engine::find_command("frame.create").unwrap().params,
            png.display()
        )
    );
    assert!(!png.exists());
    let out = cli().args(["run", "--cmd", r#"text.insert={"text":"Hi"}"#]).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "designcraft-cli: --cmd 1 of 1: command `text.insert` is not available right now: no text insertion point. text.select {\"story\": …, \"anchor\": 0, \"focus\": 0} puts the caret in a story; frame.create with \"content\": \"text\" puts it in the new frame. Nothing after it ran.\n"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_command_prints_what_it_did() {
    let out = cli()
        .args([
            "run",
            "--cmd",
            r#"frame.create={"rect":[36,36,300,200],"content":"text","text":"Hello"}"#,
            "--cmd",
            r#"text.select={"story":"$0.story","anchor":0,"focus":5}"#,
            "--cmd",
            r#"type.char={"attrs":{"size":28}}"#,
            "--cmd",
            "edit.deselectAll",
            "--cmd",
            r#"selection.set={"ids":["$0.id"]}"#,
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let first: serde_json::Value = serde_json::from_str(stdout.lines().next().unwrap().trim_start_matches("frame.create → ")).unwrap();
    let (id, story) = (&first["id"], &first["story"]);
    assert_eq!(
        stdout,
        format!(
            "frame.create → {first}\ntext.select → selected bytes 0–5 of story {story}, “Hello”\ntype.char → changed story {story}\nedit.deselectAll → no text is selected\nselection.set → selected text frame {id} on page 1\n"
        )
    );
}

/// `designcraft-cli commands | head -1` panicked with "failed printing to
/// stdout: Broken pipe (os error 32)" and exit status 101.
#[test]
fn closed_stdout_ends_quietly() {
    use std::io::Read;
    let mut child = cli().arg("commands").stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut first = [0u8; 16];
    child.stdout.as_mut().unwrap().read_exact(&mut first).unwrap();
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success() && !err.contains("panicked"), "{:?}: {err}", out.status);
}

/// `--scale`, `--page` and `--pdf-options` apply to the exports after them; one with no export
/// after it was silently ignored.
#[test]
fn run_export_options_come_before_their_export() {
    let dir = std::env::temp_dir().join(format!("dc-cli-order-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b) = (dir.join("a.png"), dir.join("b.png"));
    let (a, b) = (a.to_str().unwrap(), b.to_str().unwrap());
    for (opt, val) in [("--scale", "0.5"), ("--page", "1"), ("--pdf-options", r#"{"pages":"1"}"#)] {
        let out = cli().args(["run", "--sample", "--export", a, opt, val]).output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{opt} after the last --export: {err}");
        assert!(err.contains(opt) && err.contains("before"), "{err}");
    }
    assert!(!std::path::Path::new(a).exists(), "nothing is exported when the arguments are wrong");
    let out = cli().args(["run", "--sample", "--scale", "0.5", "--export", a, "--scale", "0.25", "--export", b]).output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(err.contains("wrote") && std::path::Path::new(a).exists() && std::path::Path::new(b).exists(), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn raster_export_preserves_an_existing_reader_and_private_symlink_target() {
    use std::io::Read;
    use std::os::unix::fs::{PermissionsExt, symlink};
    struct Fixture(std::path::PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("designcraft-cli-atomic-{}-{stamp}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let fixture = Fixture(dir);
    let target = fixture.0.join(format!("{}.png", "a".repeat(243)));
    let old = b"previous private export";
    std::fs::write(&target, old).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
    let alias = fixture.0.join("linked.png");
    symlink(target.file_name().unwrap(), &alias).unwrap();
    let mut reader = std::fs::File::open(&target).unwrap();
    let out = cli().args(["run", "--sample", "--scale", "0.1", "--export", alias.to_str().unwrap()]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let mut observed = Vec::new();
    reader.read_to_end(&mut observed).unwrap();
    assert!(observed == old, "CLI export changed bytes under an existing reader");
    assert!(std::fs::read(&target).unwrap().starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(std::fs::symlink_metadata(alias).unwrap().file_type().is_symlink());
}
