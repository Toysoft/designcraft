pub struct Subcommand {
    pub name: &'static str,
    pub usage: &'static str,
    pub summary: &'static str,
    pub details: &'static str,
}

pub const SUBCOMMANDS: &[Subcommand] = &[
    Subcommand {
        name: "run",
        usage: "run [--in FILE | --sample] [--cmd COMMAND[=PARAMS]]... [--page N] [--scale S] [--pdf-options JSON] [--export OUT]... [--all-pages DIR]",
        summary: "run commands on a document, then export it",
        details: "Runs commands, in order, on one document: FILE (.designcraft or .idml) with --in, the sample magazine
with --sample, else a new one-page Letter document. The arguments run in the order given, so an
--export after some commands writes what they made, and --cmd after an --export changes the document
for the next export.

Each --cmd is one command: COMMAND is a command id, then `=`, then the command's parameters as one
JSON object. Quote it for the shell. A command that needs no parameters is just its id.

  designcraft-cli run \\
    --cmd 'frame.create={\"rect\":[72,72,540,200],\"content\":\"text\",\"text\":\"Spring sale\"}' \\
    --cmd 'text.select={\"story\":\"$0.story\",\"anchor\":0,\"focus\":11}' \\
    --cmd 'type.char={\"attrs\":{\"fontFamily\":\"Inter\",\"size\":36}}' \\
    --export sale.png --export sale.designcraft

`designcraft-cli commands` lists the command ids; `designcraft-cli describe COMMAND` explains one and its
parameters.

What the ids don't say:
- Positions are points in spread space, x to the right and y down; the top-left of a single page is
  (0, 0), and a Letter page is 612×792. A rect is [x0, y0, x1, y1]: two corners, not a width and
  height.
- Pages are counted from 0 in parameters (`page`, `--page`) and named in answers as the document
  numbers them: 1, 2, 3… unless a section renumbers them (iii, A-3). Objects get
  ids that are unique in the whole document; `spread` names the spread an object goes on (0 by
  default).
- frame.create selects the frame it creates; with \"content\": \"text\" it also puts the caret at the end
  of its text, so text.insert types there. type.char and type.para act on the selected text, else on
  all the text of the selected frames: text.select {\"story\": N, \"anchor\": 0, \"focus\": 5} selects
  bytes 0–5 of a story. A command whose parameters list `ids` acts on those objects, else on the
  selected ones; selection.set {\"ids\": [...]} selects objects.
- A string parameter \"$N.key\" is the `key` of the answer of --cmd N (counted from 0), \"$last.key\"
  the previous command's: after frame.create answers {\"id\": 8, \"story\": 9}, \"story\": \"$0.story\"
  passes 9. Inside a longer string, \"${N.key}\" is replaced by the value's text.

Every command prints one line, `COMMAND → ANSWER`: the command's own answer as JSON, or what it changed
in words. If a command fails, run stops there; what came before it ran, including earlier exports.

--export OUT writes the document by OUT's extension: .png or .jpg of one page, .pdf, .idml, .epub, or
.designcraft (a save). --page N (counted from 0, 0 by default), --scale S (pixels per point, 1 by
default) and --pdf-options JSON (the parameters of file.exportPdf) apply to the exports after them.
--all-pages DIR writes page-001.png, page-002.png… into DIR. Each export prints one line on stderr,
e.g. `wrote sale.png (612×792, 32.6 ms, 11 glyphs)`.",
    },
    Subcommand {
        name: "commands",
        usage: "commands [FILTER]",
        summary: "list the commands run can use, one per line",
        details: "Lists the commands that run --cmd, script, app and the MCP server take, one per line: the id, its
name in the app and its parameters. FILTER keeps the commands whose id, name or menu contains it,
ignoring case.

  designcraft-cli commands frame.

In the parameters, `?` marks an optional one, a spaced ` | ` separates alternatives, `…` stands for
more, `→` shows what the command answers and ` — ` starts a remark. `designcraft-cli describe COMMAND`
spells one command out.",
    },
    Subcommand {
        name: "describe",
        usage: "describe COMMAND",
        summary: "explain one command and its parameters",
        details: "Explains one command: what it is in the app, each of its parameters, what it acts on, what it
answers, and how run calls it.

  designcraft-cli describe frame.create",
    },
    Subcommand {
        name: "script",
        usage: "script [FILE|-] [--in FILE | --sample] [--connect PORT] [--save OUT] [--export OUT]... [--keep-going]",
        summary: "run a script of commands from a file or stdin",
        details: "Runs the commands in FILE (stdin with `-` or no FILE) on one document: FILE with --in, the sample
magazine with --sample, else a new one. --connect PORT runs them in the DesignCraft app started with
`designcraft --control PORT` instead. Then --save writes the document and each --export writes it by
the extension: .pdf, .idml, .epub, else a .png or .jpg of the first page.

A script is one command per line, `COMMAND {PARAMS}`, or a JSON array of {\"command\", \"params\"}
objects, or one such object per line. Blank lines and lines starting with # or // are ignored. A
string parameter \"$N.key\" is the `key` of the answer of step N (counted from 0), \"$last.key\" the
previous step's; inside a longer string, \"${N.key}\" is replaced by the value's text.

  frame.create {\"rect\": [72, 72, 540, 300], \"content\": \"text\", \"text\": \"Results\"}
  text.select {\"story\": \"$0.story\", \"anchor\": 0, \"focus\": 0}
  footnote.insert {\"text\": \"Survey run in 2026.\"}

Prints one JSON object: {\"completed\": N, \"results\": [each step's answer]}, plus \"failedIndex\",
\"failedCommand\" and \"error\" when a step failed. A failed step stops the script and nothing is saved
or exported; with --keep-going the other steps still run and the failure is that step's result.",
    },
    Subcommand {
        name: "app",
        usage: "app [--port PORT] COMMAND [PARAMS] | --method METHOD [PARAMS]",
        summary: "run a command in the DesignCraft app started with --control",
        details: "Sends one request to the DesignCraft app started with `designcraft --control PORT` (port 7979 by
default) and prints its answer: a sentence, or JSON. COMMAND [PARAMS] runs a command, as run --cmd does; --method
METHOD [PARAMS] calls a control-channel method such as ui.screenshot (docs/control-protocol.md).
PARAMS is a JSON object.

  designcraft-cli app frame.create '{\"rect\":[72,72,300,200]}'
  designcraft-cli app --method ui.screenshot '{\"path\":\"/tmp/shot.png\"}'",
    },
    Subcommand {
        name: "mcp",
        usage: "mcp [--connect PORT] [--sample]",
        summary: "serve the commands to an agent over MCP on stdio",
        details: "Runs a Model Context Protocol server on stdin and stdout (docs/mcp.md). It works on its own document
without a window, or with --connect PORT (or host:port) on the DesignCraft app started with --control
PORT. --sample opens the sample magazine.",
    },
    Subcommand {
        name: "perf",
        usage: "perf [--pages N] [--frames N] [--chars N] [--images N] [--runs N] [--strict]",
        summary: "measure layout and rendering on a synthetic document",
        details: "Builds a document of N pages (--pages), text frames (--frames), characters (--chars) and images
(--images), measures composing and rendering it --runs times, and prints the timings against their
budgets. --strict fails when a budget is exceeded.",
    },
    Subcommand {
        name: "bench",
        usage: "bench FILE [--runs N]",
        summary: "measure layout and rendering on one document",
        details: "Opens FILE and prints how long it takes to compose and render, averaged over --runs runs.",
    },
    Subcommand {
        name: "validate-pdf",
        usage: "validate-pdf FILE.pdf",
        summary: "check a PDF against PDF/X-4",
        details: "Runs the built-in PDF/X-4 conformance checks on FILE.pdf and prints the report as JSON: {\"path\",
\"standard\": \"PDF/X-4\", \"valid\": true|false, \"issues\": [...]}. Fails when the file doesn't conform.",
    },
    Subcommand {
        name: "links",
        usage: "links",
        summary: "print the DesignCraft community and project links",
        details: "Prints the DesignCraft community and project links.",
    },
    Subcommand { name: "version", usage: "--version", summary: "print the version", details: "Prints the version of designcraft-cli." },
];

pub fn page(name: &str) -> Option<String> {
    let name = match name {
        "help" | "--help" | "-h" => return Some(overview()),
        "--version" | "-V" => "version",
        name => name,
    };
    SUBCOMMANDS.iter().find(|s| s.name == name).map(|s| format!("usage: designcraft-cli {}\n\n{}", s.usage, s.details))
}

pub fn overview() -> String {
    let first = |s: &Subcommand| s.usage.split(' ').next().unwrap_or_default();
    let width = SUBCOMMANDS.iter().map(|s| first(s).len()).max().unwrap_or_default();
    let list: Vec<String> = SUBCOMMANDS.iter().map(|s| format!("  {:<width$}  {}", first(s), s.summary)).collect();
    format!(
        "designcraft-cli works on DesignCraft documents (.designcraft, .idml) without a window.

{}

A document is made by running commands on it:

  designcraft-cli run --cmd 'frame.create={{\"rect\":[72,72,540,200],\"content\":\"text\",\"text\":\"Hello\"}}' --export hello.png

`designcraft-cli help SUBCOMMAND`, or `designcraft-cli SUBCOMMAND --help`, explains one subcommand;
`designcraft-cli help run` explains commands.",
        list.join("\n")
    )
}
