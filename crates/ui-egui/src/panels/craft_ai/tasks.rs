//! What Craft AI does inside DesignCraft. Craft AI is one assistant across the ArtCraft family
//! — the model, the runtime and the shared folder belong to the family — but its *work* is the
//! app's own: the system prompt tells the model which app it is inside and what that app does,
//! and every ready-made task is framed for that app's world, so the answer that comes back is
//! about pages, frames and paragraph styles, not generic advice.
//!
//! Each app of the family carries its own `tasks.rs` — WordCraft's asks about documents and
//! styles, SoundCraft's about tracks and buses, and so on — same model, same shared folder,
//! different craft. The labels and hints are catalog keys: the panel shows them translated.

/// The app Craft AI is embedded in, as the shared folder's records name it.
pub const APP: &str = "DesignCraft";

/// One ready-made task of the Assistant tab.
pub struct Task {
    /// The label the picker shows (a catalog key).
    pub label: &'static str,
    /// The input field's hint (a catalog key).
    pub hint: &'static str,
}

/// The tasks, in the picker's order.
pub const TASKS: &[Task] = &[
    Task { label: "Ask anything", hint: "Ask in any language" },
    Task { label: "Explain a layout step", hint: "Paste or describe the frames, guides or Pathfinder steps" },
    Task { label: "Describe a page (alt text)", hint: "Describe the page or spread for someone who cannot see it" },
    Task { label: "Name this document", hint: "What's in the document, for names" },
    Task { label: "How do I…", hint: "What you want to achieve" },
    Task { label: "Print or export advice", hint: "Where the document is going (print, PDF, web)" },
];

/// Tell the model where it is and how to answer: briefly, practically, in the language the user
/// wrote in, about this app's own world — the words of its interface, the files it opens and
/// writes. The last line is the honesty clause: a local model can be wrong, and it is told to
/// say so rather than invent.
pub fn system_prompt() -> String {
    "You are Craft AI, the assistant built into DesignCraft, a desktop page-layout and \
     publishing editor. DesignCraft's world, in its own words: documents of pages, spreads \
     and parent pages; text frames and graphics frames on layers; stories threaded between \
     frames; the Type menu's fonts and OpenType features, the Character, Paragraph, \
     Typography and Glyphs panels, and character and paragraph styles; tables; swatches, \
     gradients, strokes, effects and the Pathfinder panel; margins and columns, guides and \
     the baseline grid; the Pages, Layers and Links panels; anchored objects and captions; \
     the Story Editor; the editorial panels — Notes, Track Changes, Conditional Text — and \
     books, tables of contents, indexes, cross-references and endnotes; data merge; \
     interactive documents — hyperlinks, bookmarks, buttons and forms, media and page \
     transitions; Preflight, Package and Print; export to PDF with its presets, marks and \
     bleed, and to EPUB; IDML files opened and written beside the native .designcraft \
     documents. Answer in the language the user wrote in, Arabic included. Be brief and \
     practical: name the menu or the panel, give the steps as a short list, say what to watch \
     out for, and when a question is not about DesignCraft or publishing, answer it anyway, \
     briefly. Never invent menu entries or panel names: if you are not sure DesignCraft has a \
     feature, say so plainly."
        .to_string()
}

/// Frame the user's input for the task they picked.
pub fn user_prompt(task: usize, input: &str) -> String {
    let input = input.trim();
    match TASKS[task.min(TASKS.len() - 1)].label {
        "Explain a layout step" => format!(
            "In DesignCraft, explain in practical steps how to build or fix this part of the \
             layout — frames, guides, Pathfinder steps or all of them: {input}"
        ),
        "Describe a page (alt text)" => format!(
            "Write alt text, one or two plain and concrete sentences, for this page or spread \
             of a document: {input}"
        ),
        "Name this document" => format!(
            "Suggest short, fitting file, style and layer names for the document described \
             below — a handful of options, no explanations: {input}"
        ),
        "How do I…" => format!("In DesignCraft, how do I: {input}"),
        "Print or export advice" => format!(
            "Advise on printing or exporting this document for where it is going — PDF preset, \
             marks and bleed, colour, pitfalls — in a short list: {input}"
        ),
        _ => input.to_string(),
    }
}
