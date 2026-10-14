//! Southeast Asian scripts written without spaces between words (Thai, Lao, Khmer, Myanmar): where
//! their words end comes from a segmenter, and lines break and justify there.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use icu_segmenter::{WordSegmenter, WordSegmenterBorrowed};

use crate::shape::Glyph;

/// A letter, vowel sign or mark of these scripts: the characters Unicode line breaking (UAX #14,
/// class SA) leaves to a segmenter. The scripts' digits and punctuation break like Latin ones.
pub(crate) fn is_sea(c: char) -> bool {
    matches!(c as u32,
        0x0E01..=0x0E3A | 0x0E40..=0x0E4E // Thai
        | 0x0E81..=0x0ECE | 0x0EDC..=0x0EDF // Lao
        | 0x1000..=0x103F | 0x1050..=0x108F | 0x109A..=0x109F // Myanmar
        | 0x1780..=0x17D3 | 0x17D7 | 0x17DC..=0x17DD) // Khmer
}

/// A combining mark of these scripts, set above, below or around its base letter.
pub(crate) fn is_sea_mark(c: char) -> bool {
    match c as u32 {
        0x0E31 | 0x0E34..=0x0E3A | 0x0E47..=0x0E4E | 0x0EB1 | 0x0EB4..=0x0EBC | 0x0EC8..=0x0ECE => true,
        // Myanmar and Khmer: the nonspacing marks.
        // TODO: their spacing marks (category Mc); the shaper keeps those in their base's cluster.
        0x1000..=0x109F | 0x1780..=0x17FF => unicode_bidi::bidi_class(c) == unicode_bidi::BidiClass::NSM,
        _ => false,
    }
}

/// Is glyph `i + 1` of `line` attached to glyph `i`, so that nothing may come between them? It is
/// when it is a combining mark of these scripts, or a later glyph of the same cluster: the shaper
/// keeps a base and its marks in one cluster, whose glyphs share `byte` and `ch` (the base's).
pub(crate) fn attached(line: &[Glyph], i: usize) -> bool {
    let Some(next) = line.get(i + 1) else { return false };
    if !is_sea(next.ch) {
        return false;
    }
    // Generated glyphs (a list label) have no length either, but no glyph with one before them.
    let head = || line.get(..=i).and_then(|l| l.iter().rev().find(|g| g.len > 0));
    is_sea_mark(next.ch) || (next.len == 0 && head().is_some_and(|h| h.byte == next.byte))
}

/// No line starts with `c`: a vowel written after its consonant, or a repetition or abbreviation
/// sign, which belongs to the word before it.
fn stays_with_previous(c: char) -> bool {
    matches!(c, 'ะ' | 'า' | 'ำ' | 'ๅ' | 'ๆ' | 'ฯ' | 'ະ' | 'າ' | 'ຳ' | 'ໆ' | 'ຯ' | 'ៗ')
}

/// No line ends with `c`: a vowel written before its consonant (Thai เ แ โ ใ ไ, Lao ເ ແ ໂ ໃ ໄ).
fn stays_with_next(c: char) -> bool {
    matches!(c as u32, 0x0E40..=0x0E44 | 0x0EC0..=0x0EC4)
}

static DICTIONARY: LazyLock<WordSegmenterBorrowed<'static>> = LazyLock::new(|| WordSegmenter::new_dictionary(Default::default()));
static MODEL: LazyLock<WordSegmenterBorrowed<'static>> = LazyLock::new(|| WordSegmenter::new_lstm(Default::default()));

/// Paragraph text remembered by [`word_boundaries`] before it starts over (bytes).
const CACHE_BYTES: usize = 4 << 20;

/// Word boundaries of `span` as sorted byte offsets: the ones ICU's dictionary and its LSTM model
/// agree on. The dictionary takes the longest match and loses its place after a wrong one (ผลการ
/// comes out as ผลก|า|ร…), and the model splits some compounds; a boundary both report is a word end.
pub(crate) fn word_boundaries(span: &str) -> Arc<[usize]> {
    // A story is recomposed whole after every edit, mostly from unchanged paragraphs: remember them.
    static CACHE: LazyLock<Mutex<(HashMap<Box<str>, Arc<[usize]>>, usize)>> = LazyLock::new(Default::default);
    if let Some(known) = CACHE.lock().unwrap_or_else(PoisonError::into_inner).0.get(span) {
        return known.clone();
    }
    let model: Vec<usize> = MODEL.segment_str(span).collect();
    let out: Arc<[usize]> = DICTIONARY.segment_str(span).filter(|b| model.binary_search(b).is_ok()).collect();
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if cache.1 > CACHE_BYTES {
        *cache = Default::default();
    }
    cache.1 = cache.1.saturating_add(span.len());
    cache.0.insert(span.into(), out.clone());
    out
}

/// Mark where the paragraph's Thai, Lao, Khmer and Myanmar text may break. Between two glyphs of
/// these scripts, `break_after` becomes whether a word ends there (never inside a cluster, and
/// never where No Break or kinsoku already forbid a break), and `word_gap` marks the word ends for
/// justification. `text` is the story text the glyphs' bytes index. Other text is left as it is.
pub(crate) fn apply_word_breaks(glyphs: &mut [Glyph], text: &str) {
    if !glyphs.iter().any(|g| is_sea(g.ch)) {
        return;
    }
    // The text the paragraph's own glyphs cover (generated glyphs have no length).
    let own = || glyphs.iter().filter(|g| g.len > 0);
    let (Some(lo), Some(hi)) = (own().map(|g| g.byte).min(), own().map(|g| g.byte.saturating_add(g.len)).max()) else { return };
    let Some(span) = text.get(lo..hi) else { return };
    let words = word_boundaries(span);
    // Byte of the last glyph with a length: the glyphs after it at that byte are the rest of its cluster.
    let mut head = None;
    for i in 1..glyphs.len() {
        let (left, right) = glyphs.split_at_mut(i);
        let (Some(g), Some(next)) = (left.last_mut(), right.first()) else { continue };
        if g.len > 0 {
            head = Some(g.byte);
        }
        if g.no_break || g.break_after == Some(false) || !is_sea(g.ch) || !is_sea(next.ch) {
            continue;
        }
        if next.len == 0 {
            if head == Some(next.byte) {
                g.break_after = Some(false);
            }
            continue;
        }
        // A generated glyph (a list label) is not part of the text.
        if head != Some(g.byte) {
            continue;
        }
        let word_end = next.byte.checked_sub(lo).is_some_and(|b| words.binary_search(&b).is_ok());
        g.word_gap = word_end && !stays_with_previous(next.ch) && !stays_with_next(g.ch);
        g.break_after = Some(g.word_gap);
    }
}
