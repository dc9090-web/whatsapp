//! Group info (shown in the context drawer) and the "New group" dialog.

use std::path::PathBuf;

use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, container, icon};
use cosmic::{Element, theme};
use wa_core::{Chat, ChatId, ChatKind, GroupInfo, GroupMember};

use crate::app::{App, Message, NewGroupDraft};
use crate::pages::chats::chat_matches;
use crate::widgets;

/// A phone-number JID from whatever the user typed, if it has enough digits.
pub fn phone_to_jid(input: &str) -> Option<String> {
    let digits: String = input.chars().filter(char::is_ascii_digit).collect();
    (digits.len() >= 7).then(|| format!("{digits}@s.whatsapp.net"))
}

/// A new group needs a name and at least one other member.
pub fn can_create(draft: &NewGroupDraft) -> bool {
    !draft.creating && !draft.name.trim().is_empty() && !draft.members.is_empty()
}

/// Display name for a member: the server's, else the matching chat's, else
/// the phone number.
pub fn member_name(app: &App, member: &GroupMember) -> String {
    member
        .name
        .clone()
        .or_else(|| {
            app.chats
                .iter()
                .find(|c| c.id.0 == member.jid)
                .map(|c| c.name.clone())
        })
        .unwrap_or_else(|| format!("+{}", ChatId(member.jid.clone()).user()))
}

fn is_me(app: &App, member: &GroupMember) -> bool {
    member.name.as_deref() == Some("You")
        || app
            .own_jid
            .as_deref()
            .is_some_and(|own| ChatId(own.to_owned()).user() == ChatId(member.jid.clone()).user())
}

fn heading(label: impl Into<String>) -> Element<'static, Message> {
    widget::text::caption_heading(label.into())
        .class(theme::Text::Custom(widgets::dim_text))
        .into()
}

pub fn view(app: &App) -> Element<'_, Message> {
    let space = theme::spacing();
    let Some(info) = &app.group_info else {
        return container(
            widget::text::body("Loading group…").class(theme::Text::Custom(widgets::dim_text)),
        )
        .padding(space.space_m)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .into();
    };

    let mut column = widget::column::with_capacity(6)
        .spacing(space.space_l)
        .padding([0, space.space_s, space.space_m, space.space_s]);

    column = column.push(about(app, info));
    column = column.push(members(app, info));
    if info.me_admin {
        column = column.push(add_members(app, info));
    }
    column = column.push(invite(app));
    column = column.push(button::destructive("Leave group").on_press(Message::AskLeaveGroup));
    widget::scrollable(column).into()
}

fn about<'a>(app: &'a App, info: &'a GroupInfo) -> Element<'a, Message> {
    let space = theme::spacing();
    let mut section = widget::column::with_capacity(4).spacing(space.space_xs);
    if info.me_admin {
        let subject_changed = app.group_subject_draft.trim() != info.subject
            && !app.group_subject_draft.trim().is_empty();
        let desc_changed = app.group_desc_draft.trim() != info.description.as_deref().unwrap_or("");
        section = section
            .push(heading("Group name"))
            .push(
                widget::row::with_capacity(2)
                    .spacing(space.space_xs)
                    .align_y(Alignment::Center)
                    .push(
                        widget::text_input("Group name", app.group_subject_draft.as_str())
                            .on_input(Message::GroupSubjectDraft)
                            .on_submit(|_| Message::SaveGroupSubject)
                            .width(Length::Fill),
                    )
                    .push(
                        button::text("Save")
                            .on_press_maybe(subject_changed.then_some(Message::SaveGroupSubject)),
                    ),
            )
            .push(heading("Description"))
            .push(
                widget::row::with_capacity(2)
                    .spacing(space.space_xs)
                    .align_y(Alignment::Center)
                    .push(
                        widget::text_input("Add a description", app.group_desc_draft.as_str())
                            .on_input(Message::GroupDescDraft)
                            .on_submit(|_| Message::SaveGroupDesc)
                            .width(Length::Fill),
                    )
                    .push(
                        button::text("Save")
                            .on_press_maybe(desc_changed.then_some(Message::SaveGroupDesc)),
                    ),
            );
    } else {
        section = section
            .push(widget::text::title4(info.subject.as_str()))
            .push(
                widget::text::body(
                    info.description
                        .clone()
                        .unwrap_or_else(|| "No description".to_owned()),
                )
                .class(theme::Text::Custom(widgets::dim_text)),
            );
    }
    section.into()
}

fn members<'a>(app: &'a App, info: &'a GroupInfo) -> Element<'a, Message> {
    let space = theme::spacing();
    let mut section = widget::column::with_capacity(info.members.len() + 1)
        .spacing(space.space_xxs)
        .push(heading(format!("{} members", info.members.len())));
    for member in &info.members {
        section = section.push(member_row(app, info, member));
    }
    section.into()
}

fn member_row<'a>(
    app: &'a App,
    info: &'a GroupInfo,
    member: &'a GroupMember,
) -> Element<'a, Message> {
    let space = theme::spacing();
    let me = is_me(app, member);
    let mut name = member_name(app, member);
    if me && !name.ends_with("(you)") && name != "You" {
        name.push_str(" (you)");
    }
    let role = if info.owner.as_deref() == Some(member.jid.as_str()) || member.superadmin {
        "Owner"
    } else if member.admin {
        "Admin"
    } else {
        "Member"
    };
    let picture = app
        .avatars
        .get(&ChatId(member.jid.clone()))
        .map(PathBuf::as_path);
    let mut row = widget::row::with_capacity(3)
        .spacing(space.space_s)
        .align_y(Alignment::Center)
        .push(widgets::avatar(&name, &member.jid, 36.0, picture))
        .push(
            widget::column::with_capacity(2)
                .width(Length::Fill)
                .push(widget::text::body(name).wrapping(Wrapping::None))
                .push(widget::text::caption(role).class(theme::Text::Custom(widgets::dim_text))),
        );
    if info.me_admin && !me {
        let jid = member.jid.clone();
        let toggle = if member.admin {
            button::text("Dismiss admin").on_press(Message::GroupDemote(jid.clone()))
        } else {
            button::text("Make admin").on_press(Message::GroupPromote(jid.clone()))
        };
        let remove = widget::tooltip(
            button::icon(icon::from_name("edit-delete-symbolic"))
                .extra_small()
                .on_press(Message::GroupRemoveMember(jid)),
            widget::text::caption("Remove from group"),
            widget::tooltip::Position::Top,
        );
        row = row.push(
            widget::row::with_capacity(2)
                .spacing(space.space_xxxs)
                .push(toggle)
                .push(remove),
        );
    }
    row.into()
}

fn add_members<'a>(app: &'a App, info: &'a GroupInfo) -> Element<'a, Message> {
    let space = theme::spacing();
    let draft = app.group_add_draft.trim();
    let phone = phone_to_jid(draft);
    let mut section = widget::column::with_capacity(4)
        .spacing(space.space_xs)
        .push(heading("Add members"))
        .push(
            widget::row::with_capacity(2)
                .spacing(space.space_xs)
                .align_y(Alignment::Center)
                .push(
                    widget::text_input("Name or phone number", app.group_add_draft.as_str())
                        .on_input(Message::GroupAddDraft)
                        .width(Length::Fill),
                )
                .push(
                    button::text("Add").on_press_maybe(phone.clone().map(Message::GroupAddMember)),
                ),
        );
    let in_group = |chat: &Chat| info.members.iter().any(|m| m.jid == chat.id.0);
    let suggestions: Vec<&Chat> = app
        .chats
        .iter()
        .filter(|c| c.kind == ChatKind::Direct && !in_group(c))
        .filter(|c| draft.is_empty() || chat_matches(&c.name, draft))
        .take(8)
        .collect();
    for chat in suggestions {
        let picture = app.avatars.get(&chat.id).map(PathBuf::as_path);
        section = section.push(
            widget::row::with_capacity(3)
                .spacing(space.space_s)
                .align_y(Alignment::Center)
                .push(widgets::avatar(&chat.name, &chat.id.0, 32.0, picture))
                .push(
                    widget::text::body(chat.name.as_str())
                        .wrapping(Wrapping::None)
                        .width(Length::Fill),
                )
                .push(button::text("Add").on_press(Message::GroupAddMember(chat.id.0.clone()))),
        );
    }
    section.into()
}

fn invite(app: &App) -> Element<'_, Message> {
    let space = theme::spacing();
    let mut section = widget::column::with_capacity(3)
        .spacing(space.space_xs)
        .push(heading("Invite link"))
        .push(button::standard("Copy invite link").on_press(Message::CopyInviteLink));
    if let Some(link) = &app.invite_link {
        section = section.push(
            widget::text::caption(link.as_str()).class(theme::Text::Custom(widgets::dim_text)),
        );
    }
    section.into()
}

/// The "New group" dialog: name, a member picker over direct chats, and a
/// phone-number entry for people not in the chat list.
pub fn new_group_dialog(app: &App) -> Element<'_, Message> {
    let space = theme::spacing();
    let draft = &app.new_group;
    let mut list = widget::column::with_capacity(32).spacing(space.space_xxxs);
    let people = app
        .chats
        .iter()
        .filter(|c| c.kind == ChatKind::Direct)
        .filter(|c| draft.filter.trim().is_empty() || chat_matches(&c.name, &draft.filter))
        .take(50);
    for chat in people {
        let id = chat.id.clone();
        list = list.push(
            widget::checkbox(draft.members.contains(&chat.id))
                .label(chat.name.clone())
                .on_toggle(move |_| Message::NewGroupToggle(id.clone())),
        );
    }
    let extra: Vec<&ChatId> = draft
        .members
        .iter()
        .filter(|id| !app.chats.iter().any(|c| &c.id == *id))
        .collect();
    for id in extra {
        let id2 = id.clone();
        list = list.push(
            widget::checkbox(true)
                .label(format!("+{}", id.user()))
                .on_toggle(move |_| Message::NewGroupToggle(id2.clone())),
        );
    }
    let phone_ok = phone_to_jid(&draft.phone).is_some();
    let content = widget::column::with_capacity(5)
        .spacing(space.space_s)
        .push(
            widget::text_input("Group name", draft.name.as_str())
                .on_input(Message::NewGroupName)
                .width(Length::Fill),
        )
        .push(
            widget::search_input("Find people", draft.filter.as_str())
                .on_input(Message::NewGroupFilter)
                .on_clear(Message::NewGroupFilter(String::new())),
        )
        .push(widget::scrollable(list).height(Length::Fixed(240.0)))
        .push(
            widget::row::with_capacity(2)
                .spacing(space.space_xs)
                .align_y(Alignment::Center)
                .push(
                    widget::text_input("Add by phone number", draft.phone.as_str())
                        .on_input(Message::NewGroupPhone)
                        .on_submit(|_| Message::NewGroupAddPhone)
                        .width(Length::Fill),
                )
                .push(
                    button::text("Add")
                        .on_press_maybe(phone_ok.then_some(Message::NewGroupAddPhone)),
                ),
        )
        .push(
            widget::text::caption(if draft.creating {
                "Creating…".to_owned()
            } else {
                format!("{} selected", draft.members.len())
            })
            .class(theme::Text::Custom(widgets::dim_text)),
        );
    widget::dialog()
        .title("New group")
        .control(content)
        .primary_action(
            button::suggested("Create")
                .on_press_maybe(can_create(draft).then_some(Message::CreateGroup)),
        )
        .secondary_action(button::standard("Cancel").on_press(Message::CloseDialog))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phone_numbers_become_jids() {
        assert_eq!(
            phone_to_jid("+1 (555) 010-1001").as_deref(),
            Some("15550101001@s.whatsapp.net")
        );
        assert_eq!(phone_to_jid("alice"), None);
        assert_eq!(phone_to_jid("12345"), None);
    }

    #[test]
    fn new_group_needs_name_and_members() {
        let mut draft = NewGroupDraft::default();
        assert!(!can_create(&draft));
        draft.name = "Trip".into();
        assert!(!can_create(&draft));
        draft.members.insert(ChatId("1@s.whatsapp.net".into()));
        assert!(can_create(&draft));
        draft.creating = true;
        assert!(!can_create(&draft));
    }
}
