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

/// `--export` wrote a PNG of the page under every name it did not know: `out.html`, `out.txt` and
/// `OUT.PDF` were page images, and so was `out.designcraft` from `script` (#432).
#[test]
fn export_writes_the_file_type_its_extension_names() {
    let frame = r#"{"rect": [72, 72, 400, 200], "content": "text", "text": "Hello"}"#;
    let types: [(&str, &[u8]); 13] = [
        // No extension: a PNG, as before.
        ("a", b"\x89PNG"),
        ("a.png", b"\x89PNG"),
        ("a.jpg", b"\xFF\xD8"),
        ("A.JPEG", b"\xFF\xD8"),
        ("a.pdf", b"%PDF"),
        ("A.PDF", b"%PDF"),
        ("a.designcraft", b"PK"),
        ("a.idml", b"PK"),
        ("a.epub", b"PK"),
        ("a.html", b"<!DOCTYPE html>"),
        ("a.htm", b"<!DOCTYPE html>"),
        ("a.rtf", b"{\\rtf"),
        ("a.txt", b"Hello"),
    ];
    for mode in ["run", "script"] {
        let dir = std::env::temp_dir().join(format!("dc-cli-types-{mode}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("page.dcs");
        std::fs::write(&script, format!("frame.create {frame}\n")).unwrap();
        let mut args: Vec<String> = match mode {
            "run" => vec!["run".into(), "--cmd".into(), format!("frame.create={frame}")],
            _ => vec!["script".into(), script.to_str().unwrap().into()],
        };
        for (name, _) in types {
            args.extend(["--export".into(), dir.join(name).to_str().unwrap().into()]);
        }
        let out = cli().args(&args).output().unwrap();
        assert!(out.status.success(), "{mode}: {}", String::from_utf8_lossy(&out.stderr));
        for (name, start) in types {
            let bytes = std::fs::read(dir.join(name)).unwrap();
            assert!(bytes.starts_with(start), "{mode} --export {name} wrote {:?}", String::from_utf8_lossy(&bytes[..bytes.len().min(16)]));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// A file type `--export` does not write is an error, before anything runs or is written.
#[test]
fn export_to_an_unknown_file_type_is_an_error() {
    let dir = std::env::temp_dir().join(format!("dc-cli-unknown-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (png, doc, unknown) = (dir.join("a.png"), dir.join("a.designcraft"), dir.join("a.xyz"));
    let (png, doc, unknown) = (png.to_str().unwrap(), doc.to_str().unwrap(), unknown.to_str().unwrap());
    for args in [["run", "--sample", "--export", png, "--export", unknown], ["script", "-", "--save", doc, "--export", unknown]] {
        let out = cli().args(args).output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?}: {err}");
        assert!(err.contains(unknown) && err.contains(".html"), "{args:?}: {err}");
    }
    assert!(std::fs::read_dir(&dir).unwrap().next().is_none(), "nothing is written when a file type is unknown");
    let _ = std::fs::remove_dir_all(&dir);
}

/// With `--connect` the exports happen in the running app. `--export copy.designcraft` saves a
/// copy there: Save As would make the copy the file the open document saves to from then on.
#[test]
fn script_export_of_the_document_in_the_running_app_saves_a_copy() {
    use std::io::{BufRead, BufReader};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port().to_string();
    // Stands in for the app: answers every request and keeps what was asked.
    let app = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reply = stream.try_clone().unwrap();
        let mut asked = Vec::new();
        for line in BufReader::new(stream).lines() {
            let request: serde_json::Value = serde_json::from_str(&line.unwrap()).unwrap();
            asked.push((request["params"]["command"].clone(), request["params"]["params"]["path"].clone()));
            writeln!(reply, r#"{{"ok": true, "result": {{}}}}"#).unwrap();
        }
        asked
    });
    let out = cli().args(["script", "-", "--connect", &port, "--export", "copy.designcraft"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(app.join().unwrap(), [(serde_json::json!("file.saveACopy"), serde_json::json!("copy.designcraft"))]);
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
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"text\":\"Hello\""));
    let out = cli().args(["describe", "footnote.insert"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["label"], "Insert Footnote");
    let out = cli().args(["commands", "footnote."]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v.as_array().unwrap().iter().all(|c| c.to_string().contains("footnote")));
}

/// `designcraft-cli --help` printed usage to stderr and exited 1; asking for help is not an
/// error (#243). It now goes to stdout and succeeds, while an unknown command keeps the same
/// usage on stderr and fails.
#[test]
fn help_goes_to_stdout_and_unknown_args_to_stderr() {
    for flag in ["--help", "-h", "help"] {
        let out = cli().arg(flag).output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{flag}: {:?}", out.status);
        assert!(stdout.starts_with("usage: designcraft-cli"), "{flag}: {stdout:?}");
        assert!(stdout.contains("designcraft-cli --version"), "{flag}: {stdout:?}");
        assert!(out.stderr.is_empty(), "{flag} wrote to stderr: {}", String::from_utf8_lossy(&out.stderr));
    }
    // An unknown option is still an error: usage on stderr, nothing on stdout, non-zero exit.
    let out = cli().arg("--no-such-option").output().unwrap();
    assert!(!out.status.success(), "{:?}", out.status);
    assert!(out.stdout.is_empty(), "unexpected stdout: {}", String::from_utf8_lossy(&out.stdout));
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("usage: designcraft-cli"));
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

/// `--pdf-options` that was JSON but no object (`[1]`) panicked when the PDF's path was added to it.
#[test]
fn run_pdf_options_that_are_no_object_are_an_error() {
    let pdf = std::env::temp_dir().join(format!("dc-cli-options-{}.pdf", std::process::id()));
    for options in ["[1]", "\"all\"", "1", "null"] {
        let out = cli().args(["run", "--sample", "--pdf-options", options, "--export", pdf.to_str().unwrap()]).output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{options}: {err}");
        assert!(err.contains("--pdf-options") && !err.contains("panicked"), "{options}: {err}");
    }
    assert!(!pdf.exists());
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
