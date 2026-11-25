//! Design tokens for the COSMIC WhatsApp client redesign (direction A, "Polish").
//!
//! Pure Rust, no dependencies, so it can be dropped into the app unchanged.
//! Convert to the toolkit colour type at the call site:
//!
//! ```ignore
//! let [r, g, b] = TOKYO_NIGHT.accent.to_f32();
//! let c = cosmic::iced::Color::from_rgb(r, g, b);
//! ```

#![allow(dead_code)] // shared token set; not every token is used by the reference window

/// 8-bit sRGB colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Build from `0xRRGGBB`.
    pub const fn hex(v: u32) -> Self {
        Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    /// Components as `0.0..=1.0` floats, the form iced's `Color::from_rgb` takes.
    pub fn to_f32(self) -> [f32; 3] {
        [
            self.0 as f32 / 255.0,
            self.1 as f32 / 255.0,
            self.2 as f32 / 255.0,
        ]
    }

    /// `percent`% of `self` over `base`, per channel in sRGB space.
    /// Same result as CSS `color-mix(in srgb, self percent%, base)`.
    pub const fn mix_over(self, base: Rgb, percent: u32) -> Rgb {
        const fn ch(a: u8, b: u8, p: u32) -> u8 {
            ((a as u32 * p + b as u32 * (100 - p) + 50) / 100) as u8
        }
        Rgb(
            ch(self.0, base.0, percent),
            ch(self.1, base.1, percent),
            ch(self.2, base.2, percent),
        )
    }

    /// WCAG relative luminance.
    pub fn luminance(self) -> f64 {
        fn lin(c: u8) -> f64 {
            let c = c as f64 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * lin(self.0) + 0.7152 * lin(self.1) + 0.0722 * lin(self.2)
    }

    /// WCAG contrast ratio between two colours (1.0..=21.0).
    pub fn contrast(self, other: Rgb) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }
}

/// Every colour the main window uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// Window and conversation pane background.
    pub window_bg: Rgb,
    /// Chat list pane background.
    pub sidebar_bg: Rgb,
    /// Search field, unselected filter chips, incoming bubbles.
    pub component: Rgb,
    /// Selected chat row; also pane and header dividers.
    pub selected: Rgb,
    /// Date separator chip background.
    pub date_chip: Rgb,
    /// Message field border.
    pub input_border: Rgb,
    /// Border of the "Ctrl K" hint.
    pub key_border: Rgb,
    /// Primary text.
    pub text: Rgb,
    /// Timestamps, previews, status lines.
    pub text_secondary: Rgb,
    /// Timestamps inside outgoing bubbles.
    pub text_on_tint: Rgb,
    /// Accent: selected chip, unread badge, read ticks, send/mic button.
    pub accent: Rgb,
    /// Text and icons drawn on top of `accent`.
    pub on_accent: Rgb,
    /// Outgoing bubble and "your reaction" chip: `accent` at `TINT_PERCENT` over `window_bg`.
    pub tint: Rgb,
    /// Avatar fallback fills; pick with [`avatar_index`].
    pub avatars: [Rgb; 8],
}

/// Share of accent in [`Palette::tint`].
pub const TINT_PERCENT: u32 = 30;

const fn tokyo_night() -> Palette {
    let window_bg = Rgb::hex(0x1a1b26);
    let accent = Rgb::hex(0x7aa2f7);
    Palette {
        window_bg,
        sidebar_bg: Rgb::hex(0x16161e),
        component: Rgb::hex(0x24283b),
        selected: Rgb::hex(0x292e42),
        date_chip: Rgb::hex(0x1f2335),
        input_border: Rgb::hex(0x3b4261),
        key_border: Rgb::hex(0x414868),
        text: Rgb::hex(0xc0caf5),
        text_secondary: Rgb::hex(0x9aa5ce),
        text_on_tint: Rgb::hex(0xa9b1d6),
        accent,
        on_accent: Rgb::hex(0x16161e),
        tint: accent.mix_over(window_bg, TINT_PERCENT),
        avatars: [
            Rgb::hex(0x9ece6a),
            Rgb::hex(0xe0af68),
            Rgb::hex(0xbb9af7),
            Rgb::hex(0x7dcfff),
            Rgb::hex(0xf7768e),
            Rgb::hex(0x9aa5ce),
            Rgb::hex(0xff9e64),
            Rgb::hex(0x73daca),
        ],
    }
}

/// Tokyo Night ("Night" variant). The approved look.
pub const TOKYO_NIGHT: Palette = tokyo_night();

impl Palette {
    /// Same palette with a different accent (e.g. the COSMIC system accent).
    /// Recomputes `tint`; `on_accent` stays dark, so pass a light accent.
    pub const fn with_accent(mut self, accent: Rgb) -> Palette {
        self.accent = accent;
        self.tint = accent.mix_over(self.window_bg, TINT_PERCENT);
        self
    }
}

/// Stable avatar colour slot for a chat, from its id (e.g. the JID).
/// FNV-1a, so the same chat keeps its colour across runs and machines.
pub fn avatar_index(chat_id: &str) -> usize {
    let mut h: u32 = 0x811c9dc5;
    for b in chat_id.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    (h % 8) as usize
}

/// Up to two initials for the avatar fallback: first letters of the first two words.
pub fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|w| w.chars().find(|c| c.is_alphanumeric()))
        .take(2)
        .flat_map(char::to_uppercase)
        .collect()
}

/// Unread badge text: the count, capped at "99+". `None` when there is nothing unread.
pub fn unread_label(count: u32) -> Option<String> {
    match count {
        0 => None,
        1..=99 => Some(count.to_string()),
        _ => Some("99+".to_string()),
    }
}

/// Sizes in logical pixels. Names match `design/SPEC.md`.
pub mod size {
    pub const WINDOW_RADIUS: f32 = 12.0;
    pub const SIDEBAR_WIDTH: f32 = 344.0;
    pub const HEADER_HEIGHT: f32 = 56.0;

    pub const ICON_BUTTON: f32 = 40.0;
    pub const ICON: f32 = 20.0;
    pub const WINDOW_BUTTON: f32 = 32.0;
    pub const WINDOW_ICON: f32 = 16.0;

    pub const SEARCH_HEIGHT: f32 = 40.0;
    pub const CHIP_HEIGHT: f32 = 32.0;
    pub const CHIP_PAD_X: f32 = 14.0;
    pub const CHIP_GAP: f32 = 6.0;

    pub const ROW_HEIGHT: f32 = 68.0;
    pub const ROW_RADIUS: f32 = 8.0;
    pub const ROW_PAD_X: f32 = 12.0;
    pub const ROW_GAP: f32 = 2.0;
    pub const AVATAR_ROW: f32 = 44.0;
    pub const AVATAR_HEADER: f32 = 36.0;
    pub const BADGE: f32 = 20.0;

    pub const MESSAGES_PAD_X: f32 = 28.0;
    pub const MESSAGES_PAD_Y: f32 = 20.0;
    pub const BUBBLE_MAX_WIDTH: f32 = 520.0;
    pub const BUBBLE_PAD_X: f32 = 14.0;
    pub const BUBBLE_PAD_Y: f32 = 10.0;
    pub const BUBBLE_RADIUS: f32 = 16.0;
    /// Corner nearest the sender's edge (bottom-right outgoing, bottom-left incoming).
    pub const BUBBLE_TAIL_RADIUS: f32 = 4.0;
    /// Gap between bubbles from the same sender / from different senders.
    pub const BUBBLE_GAP_SAME: f32 = 4.0;
    pub const BUBBLE_GAP_OTHER: f32 = 12.0;

    pub const REACTION_ICON: f32 = 20.0;
    pub const REACTION_RADIUS: f32 = 16.0;
    /// Reaction chip overlaps the bubble above it by this much.
    pub const REACTION_OVERLAP: f32 = 10.0;

    pub const COMPOSER_BUTTON: f32 = 44.0;
    pub const COMPOSER_ICON: f32 = 22.0;
    pub const COMPOSER_FIELD_HEIGHT: f32 = 44.0;

    pub const TEXT_TITLE: f32 = 18.0;
    pub const TEXT_NAME: f32 = 14.5;
    pub const TEXT_BODY: f32 = 14.0;
    pub const TEXT_PREVIEW: f32 = 13.0;
    pub const TEXT_META: f32 = 12.0;
    pub const TEXT_BUBBLE_TIME: f32 = 11.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        assert_eq!(Rgb::hex(0x7aa2f7), Rgb(0x7a, 0xa2, 0xf7));
    }

    #[test]
    fn tint_matches_css_color_mix() {
        // color-mix(in srgb, #7aa2f7 30%, #1a1b26)
        assert_eq!(TOKYO_NIGHT.tint, Rgb(0x37, 0x44, 0x65));
    }

    #[test]
    fn text_meets_wcag_aa() {
        let p = TOKYO_NIGHT;
        let pairs = [
            ("text / window", p.text, p.window_bg),
            ("text / sidebar", p.text, p.sidebar_bg),
            ("text / component", p.text, p.component),
            ("text / selected", p.text, p.selected),
            ("text / tint", p.text, p.tint),
            ("secondary / window", p.text_secondary, p.window_bg),
            ("secondary / sidebar", p.text_secondary, p.sidebar_bg),
            ("secondary / component", p.text_secondary, p.component),
            ("secondary / selected", p.text_secondary, p.selected),
            ("secondary / date chip", p.text_secondary, p.date_chip),
            ("on-tint time / tint", p.text_on_tint, p.tint),
            ("on-accent / accent", p.on_accent, p.accent),
            ("accent / sidebar", p.accent, p.sidebar_bg),
        ];
        for (name, fg, bg) in pairs {
            let r = fg.contrast(bg);
            assert!(r >= 4.5, "{name}: {r:.2}");
        }
        for (i, a) in p.avatars.iter().enumerate() {
            let r = p.window_bg.contrast(*a);
            assert!(r >= 4.5, "avatar {i}: {r:.2}");
        }
    }

    #[test]
    fn with_accent_recomputes_tint() {
        let p = TOKYO_NIGHT.with_accent(Rgb::hex(0xbb9af7));
        assert_eq!(
            p.tint,
            Rgb::hex(0xbb9af7).mix_over(p.window_bg, TINT_PERCENT)
        );
        assert_ne!(p.tint, TOKYO_NIGHT.tint);
    }

    #[test]
    fn avatar_index_is_stable_and_in_range() {
        assert_eq!(
            avatar_index("61400000000@s.whatsapp.net"),
            avatar_index("61400000000@s.whatsapp.net")
        );
        for id in ["", "a", "group-123@g.us", "ünïcödé"] {
            assert!(avatar_index(id) < 8);
        }
    }

    #[test]
    fn initials_cases() {
        assert_eq!(initials("Sam Rivera"), "SR");
        assert_eq!(initials("family"), "F");
        assert_eq!(initials("  Design   team chat "), "DT");
        assert_eq!(initials("+61 400 000 000"), "64");
        assert_eq!(initials(""), "");
    }

    #[test]
    fn unread_label_cases() {
        assert_eq!(unread_label(0), None);
        assert_eq!(unread_label(4).as_deref(), Some("4"));
        assert_eq!(unread_label(99).as_deref(), Some("99"));
        assert_eq!(unread_label(100).as_deref(), Some("99+"));
    }
}
