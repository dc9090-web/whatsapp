//! Settings, shown in the COSMIC context drawer.

use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, settings};
use cosmic::{Element, theme};

use crate::app::{App, Message};
use crate::config::Appearance;
use crate::widgets;

const APPEARANCE_LABELS: [&str; 3] = [
    Appearance::Dark.label(),
    Appearance::Light.label(),
    Appearance::System.label(),
];

pub fn view(app: &App) -> Element<'_, Message> {
    let space = theme::spacing();
    let selected = Appearance::ALL
        .iter()
        .position(|a| *a == app.config.appearance);

    let appearance = settings::section().title("Appearance").add(settings::item(
        "Theme",
        widget::dropdown(&APPEARANCE_LABELS[..], selected, |i| {
            Message::SetAppearance(Appearance::ALL.get(i).copied().unwrap_or_default())
        }),
    ));

    let notifications = settings::section().title("Notifications").add(
        settings::item::builder("Message previews")
            .description("Show the message text in notifications")
            .toggler(app.config.notification_previews, Message::SetPreviews),
    );

    let startup = settings::section().title("Startup");
    let startup = match app.autostart {
        Some(enabled) => startup.add(
            settings::item::builder("Start at login")
                .description("Run in the background and keep receiving messages")
                .toggler(enabled, Message::SetAutostart),
        ),
        None => startup.add(
            settings::item::builder("Start at login")
                .description("Not available in this build")
                .control(widget::toggler(false)),
        ),
    };

    let account = settings::section().title("Account").add(
        settings::item::builder("Log out")
            .description("Unlink this computer and delete local messages")
            .control(button::destructive("Log out").on_press(Message::RequestLogout)),
    );

    let about = widget::column::with_capacity(2)
        .spacing(space.space_xxxs)
        .align_x(Alignment::Center)
        .width(Length::Fill)
        .push(widget::text::caption(format!(
            "WhatsApp for COSMIC {}",
            env!("CARGO_PKG_VERSION")
        )))
        .push(
            widget::text::caption("Unofficial client — not affiliated with WhatsApp or Meta")
                .class(theme::Text::Custom(widgets::dim_text)),
        );

    settings::view_column(vec![
        appearance.into(),
        notifications.into(),
        startup.into(),
        account.into(),
        about.into(),
    ])
    .into()
}
