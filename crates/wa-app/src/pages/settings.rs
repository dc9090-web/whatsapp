//! Settings, shown in the COSMIC context drawer.

use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, button, settings};
use cosmic::{Element, theme};

use crate::app::{App, Message};
use crate::config::Appearance;
use crate::widgets;

const APPEARANCE_LABELS: [&str; 4] = [
    Appearance::TokyoNight.label(),
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

    let custom_sound = app.config.sound_file();
    let sound_name = match &custom_sound {
        Some(path) => path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Custom sound".to_owned()),
        None => "Default chime".to_owned(),
    };
    let sound_buttons = widget::row::with_capacity(3)
        .spacing(space.space_xxs)
        .align_y(Alignment::Center)
        .push(button::standard("Test").on_press(Message::TestSound))
        .push(button::standard("Choose…").on_press(Message::PickSoundFile))
        .push_maybe(
            custom_sound
                .is_some()
                .then(|| button::standard("Reset").on_press(Message::ResetSoundFile)),
        );

    let notifications = settings::section()
        .title("Notifications")
        .add(
            settings::item::builder("Message previews")
                .description("Show the message text in notifications")
                .toggler(app.config.notification_previews, Message::SetPreviews),
        )
        .add(
            settings::item::builder("Notification sound")
                .description("Play a chime when a message arrives")
                .toggler(app.config.notification_sound, Message::SetSound),
        )
        .add(
            settings::item::builder("Volume")
                .description(format!("{}%", app.config.notification_volume))
                .control(
                    widget::slider(
                        0..=100u8,
                        app.config.notification_volume,
                        Message::SetSoundVolume,
                    )
                    .step(5u8)
                    .on_release(Message::TestSound)
                    .width(Length::Fixed(180.0)),
                ),
        )
        .add(
            settings::item::builder("Sound")
                .description(sound_name)
                .control(sound_buttons),
        );

    let gifs = settings::section().title("GIFs").add(
        settings::item::builder("GIPHY API key")
            .description("Needed to search GIFs. Free from developers.giphy.com")
            .control(
                widget::text_input("Paste your key", app.config.giphy_api_key.as_str())
                    .on_input(Message::SetGiphyKey)
                    .width(Length::Fixed(220.0)),
            ),
    );

    let startup = settings::section().title("Startup");
    let startup = match app.autostart {
        Some(enabled) => startup.add(
            settings::item::builder("Start at login")
                .description("Open WhatsApp when you log in")
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
        gifs.into(),
        startup.into(),
        account.into(),
        about.into(),
    ])
    .into()
}
