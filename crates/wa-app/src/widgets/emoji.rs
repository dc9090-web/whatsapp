//! The emoji set and the round emoji buttons shared by the composer's emoji
//! panel and the reaction picker.

use cosmic::iced::{Alignment, Background, Length};
use cosmic::widget::{self, button, container};
use cosmic::{Element, theme};

use super::dim_text;

/// Emoji grouped the way the picker shows them.
pub const CATEGORIES: &[(&str, &[&str])] = &[
    (
        "Smileys",
        &[
            "😀", "😃", "😄", "😁", "😆", "😅", "🤣", "😂", "🙂", "🙃", "😉", "😊", "😇", "🥰",
            "😍", "🤩", "😘", "😗", "😚", "😙", "🥲", "😋", "😛", "😜", "🤪", "😝", "🤑", "🤗",
            "🤭", "🤫", "🤔", "🤐", "🤨", "😐", "😑", "😶", "😏", "😒", "🙄", "😬", "🤥", "😌",
            "😔", "😪", "🤤", "😴", "😷", "🤒", "🤕", "🤢", "🤮", "🤧", "🥵", "🥶", "🥴", "😵",
            "🤯", "🤠", "🥳", "😎", "🤓", "🧐", "😕", "😟", "🙁", "😮", "😯", "😲", "😳", "🥺",
            "😦", "😧", "😨", "😰", "😥", "😢", "😭", "😱", "😖", "😣", "😞", "😓", "😩", "😫",
            "🥱", "😤", "😡", "😠", "🤬", "😈", "👿", "💀", "💩", "🤡", "👻", "👽", "🤖", "😺",
            "😸", "😹", "😻", "🙈", "🙉", "🙊",
        ],
    ),
    (
        "Gestures & people",
        &[
            "👍", "👎", "👌", "🤌", "🤏", "✌️", "🤞", "🤟", "🤘", "🤙", "👈", "👉", "👆", "👇",
            "☝️", "✋", "🤚", "🖐️", "🖖", "👋", "🤝", "👏", "🙌", "👐", "🤲", "🙏", "✍️", "💪",
            "🫶", "👀", "🧠", "👶", "🧒", "👦", "👧", "🧑", "👨", "👩", "🧓", "👴", "👵", "🙋",
            "🤷", "🤦", "🙅", "🙆", "💁", "🏃", "🚶", "💃", "🕺", "👪",
        ],
    ),
    (
        "Hearts",
        &[
            "❤️", "🧡", "💛", "💚", "💙", "💜", "🖤", "🤍", "🤎", "💔", "❣️", "💕", "💞", "💓",
            "💗", "💖", "💘", "💝", "💋", "💯", "💢", "💥", "💫", "💦", "💤",
        ],
    ),
    (
        "Animals & nature",
        &[
            "🐶", "🐱", "🐭", "🐹", "🐰", "🦊", "🐻", "🐼", "🐨", "🐯", "🦁", "🐮", "🐷", "🐸",
            "🐵", "🐔", "🐧", "🐦", "🦆", "🦉", "🐴", "🦄", "🐝", "🦋", "🐌", "🐞", "🐢", "🐍",
            "🐙", "🐬", "🐳", "🦈", "🐘", "🦒", "🐑", "🐕", "🐈", "🌵", "🎄", "🌲", "🌴", "🌱",
            "🍀", "🍁", "🍂", "🌹", "🌷", "🌸", "🌻", "🌞", "🌙", "⭐", "🌟", "✨", "⚡", "🔥",
            "🌈", "☀️", "⛅", "☁️", "🌧️", "❄️", "☃️", "🌊",
        ],
    ),
    (
        "Food & drink",
        &[
            "🍏", "🍎", "🍐", "🍊", "🍋", "🍌", "🍉", "🍇", "🍓", "🍒", "🍑", "🥭", "🍍", "🥥",
            "🥝", "🍅", "🥑", "🥦", "🌽", "🥕", "🥔", "🍞", "🥐", "🧀", "🥚", "🍳", "🥓", "🍔",
            "🍟", "🍕", "🌭", "🥪", "🌮", "🌯", "🍝", "🍜", "🍲", "🍛", "🍣", "🍱", "🍤", "🍚",
            "🍦", "🍰", "🎂", "🍪", "🍩", "🍫", "🍬", "🍿", "☕", "🍵", "🥤", "🍺", "🍻", "🥂",
            "🍷", "🥃", "🍸", "🍹", "🍾",
        ],
    ),
    (
        "Activities & travel",
        &[
            "⚽", "🏀", "🏈", "⚾", "🎾", "🏐", "🏉", "🎱", "🏓", "🏸", "🥊", "⛳", "🏊", "🚴",
            "🏆", "🥇", "🥈", "🥉", "🏅", "🎯", "🎮", "🎲", "🎸", "🎹", "🎤", "🎧", "🎬", "🎨",
            "🎉", "🎊", "🎈", "🎁", "🚗", "🚕", "🚌", "🚓", "🚑", "🚒", "🚚", "🚲", "🛵", "✈️",
            "🚀", "🚁", "⛵", "🚢", "🚆", "🏠", "🏢", "🏥", "🏖️", "🏔️", "🗺️", "🌍",
        ],
    ),
    (
        "Objects & symbols",
        &[
            "📱", "💻", "⌨️", "🖥️", "📷", "📺", "⏰", "⌛", "💡", "🔋", "🔌", "💰", "💳", "💎",
            "🔧", "🔨", "🔑", "🔒", "📦", "📌", "📎", "✏️", "📝", "📅", "📚", "📖", "✉️", "📞",
            "🎵", "🎶", "🔔", "🔕", "📣", "💬", "💭", "✅", "❌", "❓", "❗", "⚠️", "🚫", "♻️",
            "➕", "➖", "➡️", "⬅️", "⬆️", "⬇️", "🔴", "🟠", "🟡", "🟢", "🔵", "🟣", "⚫", "⚪",
        ],
    ),
];

/// Diameter of an emoji button in the pickers.
pub const BUTTON_SIZE: f32 = 44.0;
/// Glyph size inside a picker button.
pub const GLYPH_SIZE: u16 = 28;

/// A round button showing one emoji, `diameter` across.
pub fn round_button<'a, M: Clone + 'a>(
    emoji: &'a str,
    glyph: u16,
    diameter: f32,
    on_press: M,
) -> Element<'a, M> {
    button::custom(
        container(widget::text(emoji).size(glyph))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
    )
    .width(Length::Fixed(diameter))
    .height(Length::Fixed(diameter))
    .padding(0)
    .class(round_class(diameter / 2.0))
    .on_press(on_press)
    .into()
}

/// Transparent round button that fills with a circle on hover.
pub fn round_class(radius: f32) -> theme::Button {
    fn style(theme: &cosmic::Theme, radius: f32, hovered: bool) -> button::Style {
        let cosmic = theme.cosmic();
        let mut style = button::Style::new();
        style.border_radius = radius.into();
        style.background =
            hovered.then(|| Background::Color(cosmic.background(false).component.hover.into()));
        style.text_color = Some(cosmic.on_bg_color().into());
        style
    }
    theme::Button::Custom {
        active: Box::new(move |_, t| style(t, radius, false)),
        disabled: Box::new(move |t| style(t, radius, false)),
        hovered: Box::new(move |_, t| style(t, radius, true)),
        pressed: Box::new(move |_, t| style(t, radius, true)),
    }
}

/// Width of a grid with `per_row` buttons.
pub fn grid_width(per_row: usize) -> f32 {
    BUTTON_SIZE * per_row as f32
}

/// Most search results shown.
pub const MAX_RESULTS: usize = 96;

/// Emoji whose name or shortcode contains every word of `query` ("red
/// heart", "thumbs", "joy"), best matches first. Searches every emoji, not
/// only the ones in [`CATEGORIES`].
pub fn search(query: &str) -> Vec<&'static str> {
    let query = query.trim().trim_matches(':').to_lowercase();
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(u8, &'static str)> = emojis::iter()
        .filter_map(|emoji| {
            let name = emoji.name();
            let matches = |word: &str| {
                name.contains(word) || emoji.shortcodes().any(|code| code.contains(word))
            };
            if !words.iter().all(|word| matches(word)) {
                return None;
            }
            // A name or shortcode that *is* the query beats one that starts
            // with it, which beats one that merely contains it.
            let rank = if name == query || emoji.shortcodes().any(|code| code == query) {
                0
            } else if name
                .split([' ', '-'])
                .any(|part| part.starts_with(words[0]))
                || emoji
                    .shortcodes()
                    .any(|code| code.split('_').any(|part| part.starts_with(words[0])))
            {
                1
            } else {
                2
            };
            Some((rank, emoji.as_str()))
        })
        .collect();
    hits.sort_by_key(|(rank, _)| *rank);
    hits.truncate(MAX_RESULTS);
    hits.into_iter().map(|(_, emoji)| emoji).collect()
}

/// The picker's body: every category, or the emoji matching `query` when
/// one is typed.
pub fn picker<'a, M: Clone + 'a>(
    query: &str,
    per_row: usize,
    on_pick: impl Fn(&'static str) -> M,
) -> Element<'a, M> {
    if query.trim().is_empty() {
        return grid(per_row, on_pick);
    }
    let found = search(query);
    if found.is_empty() {
        return container(
            widget::text::body("No emoji found").class(theme::Text::Custom(dim_text)),
        )
        .width(Length::Fixed(grid_width(per_row)))
        .padding(theme::spacing().space_m)
        .align_x(Alignment::Center)
        .into();
    }
    let mut column = widget::column::with_capacity(found.len() / per_row + 1)
        .width(Length::Fixed(grid_width(per_row)));
    for chunk in found.chunks(per_row) {
        let mut row = widget::row::with_capacity(per_row);
        for emoji in chunk {
            row = row.push(round_button(emoji, GLYPH_SIZE, BUTTON_SIZE, on_pick(emoji)));
        }
        column = column.push(row);
    }
    column.into()
}

/// Every category under its heading, `per_row` round buttons to a line.
pub fn grid<'a, M: Clone + 'a>(
    per_row: usize,
    on_pick: impl Fn(&'static str) -> M,
) -> Element<'a, M> {
    let space = theme::spacing();
    let mut column = widget::column::with_capacity(CATEGORIES.len() * 8)
        .width(Length::Fixed(grid_width(per_row)));
    for (i, (name, emojis)) in CATEGORIES.iter().enumerate() {
        let top = if i == 0 { 0 } else { space.space_xs };
        column = column.push(
            container(widget::text::caption_heading(*name).class(theme::Text::Custom(dim_text)))
                .padding([top, space.space_xs, space.space_xxs, space.space_xs]),
        );
        for chunk in emojis.chunks(per_row) {
            let mut row = widget::row::with_capacity(per_row);
            for emoji in chunk {
                row = row.push(round_button(emoji, GLYPH_SIZE, BUTTON_SIZE, on_pick(emoji)));
            }
            column = column.push(row);
        }
    }
    column.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_emoji_is_listed_twice() {
        let mut all: Vec<&str> = CATEGORIES
            .iter()
            .flat_map(|(_, e)| e.iter().copied())
            .collect();
        let total = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), total);
    }

    #[test]
    fn search_matches_names_and_shortcodes() {
        assert!(search("").is_empty());
        assert!(search("   ").is_empty());
        assert!(search("zzzzqq").is_empty());
        assert!(search("heart").contains(&"❤️"));
        assert!(search("red heart").contains(&"❤️"));
        assert!(search("Thumbs").contains(&"👍"));
        assert!(search("laugh").contains(&"😆"));
        // Shortcodes work with or without their colons.
        assert_eq!(search(":joy:").first(), Some(&"😂"));
        assert_eq!(search("fire").first(), Some(&"🔥"));
        assert!(search("a").len() <= MAX_RESULTS);
    }

    #[test]
    fn quick_reactions_are_in_the_picker() {
        for emoji in super::super::reactions::QUICK_EMOJIS {
            assert!(
                CATEGORIES.iter().any(|(_, e)| e.contains(&emoji)),
                "{emoji} missing from picker"
            );
        }
    }
}
