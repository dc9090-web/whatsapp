//! Colour emoji everywhere.
//!
//! cosmic-text's Unix fallback list tries "DejaVu Sans" before "Noto Color
//! Emoji", and DejaVu ships monochrome glyphs for ❤ and most of the 😀–🙏
//! block. Any emoji the interface font lacks therefore rendered as a tinted
//! stencil. We rebuild the global font system with the colour emoji font
//! moved ahead of the monochrome fallbacks.

use cosmic::iced::advanced::graphics::text::{cosmic_text, font_system};
use cosmic_text::{Fallback, PlatformFallback};
use unicode_script::Script;

const EMOJI_FONT: &str = "Noto Color Emoji";

/// [`PlatformFallback`] with [`EMOJI_FONT`] promoted to right after the
/// primary sans fallback.
#[derive(Debug)]
struct EmojiFirst;

/// `cosmic_text`'s list wants `&'static`, so the reordered list is built once.
fn common() -> &'static [&'static str] {
    static LIST: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    LIST.get_or_init(|| reorder(PlatformFallback.common_fallback()))
}

/// Moves [`EMOJI_FONT`] to just after the first entry (the primary sans
/// fallback, which has no emoji), adding it if missing.
fn reorder(list: &[&'static str]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = list.iter().copied().filter(|f| *f != EMOJI_FONT).collect();
    out.insert(out.len().min(1), EMOJI_FONT);
    out
}

impl Fallback for EmojiFirst {
    fn common_fallback(&self) -> &'static [&'static str] {
        common()
    }

    fn forbidden_fallback(&self) -> &'static [&'static str] {
        PlatformFallback.forbidden_fallback()
    }

    fn script_fallback(&self, script: Script, locale: &str) -> &'static [&'static str] {
        PlatformFallback.script_fallback(script, locale)
    }
}

/// Swaps the global font system's fallback list. Call once at startup,
/// before any text is shaped.
pub fn prefer_colour_emoji() {
    let mut system = match font_system().write() {
        Ok(system) => system,
        Err(poisoned) => poisoned.into_inner(),
    };
    let raw = system.raw();
    // A throwaway system with an empty database is cheap to build.
    let empty = cosmic_text::FontSystem::new_with_locale_and_db(
        String::new(),
        cosmic_text::fontdb::Database::new(),
    );
    let (locale, db) = std::mem::replace(raw, empty).into_locale_and_db();
    *raw = cosmic_text::FontSystem::new_with_locale_and_db_and_fallback(locale, db, EmojiFirst);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_font_comes_before_dejavu() {
        let list = reorder(&["Noto Sans", "DejaVu Sans", "Noto Sans Symbols2", EMOJI_FONT]);
        assert_eq!(
            list,
            ["Noto Sans", EMOJI_FONT, "DejaVu Sans", "Noto Sans Symbols2"]
        );
    }

    #[test]
    fn emoji_font_is_added_when_missing() {
        assert_eq!(
            reorder(&["Noto Sans", "DejaVu Sans"]),
            ["Noto Sans", EMOJI_FONT, "DejaVu Sans"]
        );
        assert_eq!(reorder(&[]), [EMOJI_FONT]);
    }

    #[test]
    fn platform_list_is_reordered() {
        let list = common();
        let emoji = list.iter().position(|f| *f == EMOJI_FONT).unwrap();
        if let Some(dejavu) = list.iter().position(|f| *f == "DejaVu Sans") {
            assert!(emoji < dejavu, "{list:?}");
        }
    }
}
