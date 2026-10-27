//! Reaction chips under a bubble and the quick-reaction set.

use cosmic::iced::Length;
use cosmic::widget::{self, button};
use cosmic::{Element, theme};
use wa_core::{MessageId, Reaction};

use crate::app::Message;

/// The six quick reactions offered in the message menu.
pub const QUICK_EMOJIS: [&str; 6] = ["👍", "❤️", "😂", "😮", "😢", "🙏"];

/// The "More…" emoji grid.
pub const ALL_EMOJIS: &[&str] = &[
    "👍", "👎", "❤️", "🧡", "💛", "💚", "💙", "💜", "🖤", "🤍", "💯", "🔥", "✨", "🎉", "🙏", "👏",
    "🙌", "👌", "✌️", "🤞", "💪", "🤝", "👀", "🫶", "😀", "😃", "😄", "😁", "😆", "😅", "🤣", "😂",
    "🙂", "😉", "😊", "😇", "🥰", "😍", "🤩", "😘", "😋", "😛", "😜", "🤪", "🤗", "🤔", "🤨", "😐",
    "😑", "😶", "🙄", "😏", "😬", "😌", "😔", "😪", "😴", "😷", "🤒", "🤯", "🥳", "😎", "🤓", "😕",
    "😟", "😮", "😯", "😲", "😳", "🥺", "😦", "😧", "😨", "😰", "😥", "😢", "😭", "😱", "😖", "😣",
    "😞", "😓", "😩", "😫", "😤", "😡", "😠", "🤬", "💀", "💩", "🤡", "👻", "👽", "🤖", "🎂", "🍕",
    "☕", "🍺", "🥂", "⚽", "🏆", "🎵", "📌", "✅", "❌", "❓", "❗", "⭐", "🌹", "🌞", "🌈", "🐶",
];

/// Groups reactions into `(emoji, count, includes_mine)`, most popular first
/// (ties broken by emoji for a stable order). Empty emojis are removals and
/// are ignored.
pub fn group_reactions(
    reactions: &[Reaction],
    is_mine: impl Fn(&Reaction) -> bool,
) -> Vec<(String, usize, bool)> {
    let mut groups: Vec<(String, usize, bool)> = Vec::new();
    for reaction in reactions.iter().filter(|r| !r.emoji.is_empty()) {
        let mine = is_mine(reaction);
        match groups
            .iter_mut()
            .find(|(emoji, ..)| *emoji == reaction.emoji)
        {
            Some(group) => {
                group.1 += 1;
                group.2 |= mine;
            }
            None => groups.push((reaction.emoji.clone(), 1, mine)),
        }
    }
    groups.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    groups
}

/// Chips for each grouped reaction; clicking toggles our own reaction.
pub fn chips<'a>(target: &MessageId, groups: Vec<(String, usize, bool)>) -> Element<'a, Message> {
    let space = theme::spacing();
    let mut row = widget::row::with_capacity(groups.len()).spacing(space.space_xxxs);
    for (emoji, count, mine) in groups {
        let label = if count > 1 {
            format!("{emoji} {count}")
        } else {
            emoji.clone()
        };
        let next = if mine { String::new() } else { emoji };
        row = row.push(
            button::custom(widget::text::caption(label))
                .padding([2, 8])
                .class(super::chip_class(mine))
                .on_press(Message::React(target.clone(), next)),
        );
    }
    row.width(Length::Shrink).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wa_core::ChatId;

    fn r(sender: &str, emoji: &str) -> Reaction {
        Reaction {
            chat: ChatId("c@g.us".into()),
            target: MessageId("m".into()),
            sender: sender.into(),
            emoji: emoji.into(),
            timestamp: 0,
        }
    }

    #[test]
    fn groups_by_count_desc_and_flags_mine() {
        let reactions = vec![
            r("a", "❤️"),
            r("me", "👍"),
            r("b", "👍"),
            r("c", "😂"),
            r("d", "👍"),
            r("e", ""),
        ];
        let groups = group_reactions(&reactions, |r| r.sender == "me");
        assert_eq!(
            groups,
            vec![
                ("👍".to_owned(), 3, true),
                ("❤️".to_owned(), 1, false),
                ("😂".to_owned(), 1, false),
            ]
        );
    }

    #[test]
    fn empty_input_gives_no_groups() {
        assert!(group_reactions(&[], |_| false).is_empty());
        assert!(group_reactions(&[r("a", "")], |_| false).is_empty());
    }

    #[test]
    fn quick_emojis_are_in_full_grid() {
        for emoji in QUICK_EMOJIS {
            assert!(ALL_EMOJIS.contains(&emoji), "{emoji} missing from grid");
        }
    }
}
