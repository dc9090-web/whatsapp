//! User settings persisted with `cosmic-config` under the app id.

use cosmic::cosmic_config::{self, ConfigGet, ConfigSet};
use serde::{Deserialize, Serialize};

use crate::app::APP_ID;

pub const CONFIG_VERSION: u64 = 1;

const KEY_APPEARANCE: &str = "appearance";
const KEY_PREVIEWS: &str = "notification_previews";
const KEY_SOUND: &str = "notification_sound";
const KEY_GIPHY: &str = "giphy_api_key";
const KEY_DESIGN: &str = "design";
/// Bumped when a release should move everyone onto its new default look once.
const DESIGN: u32 = 2;
const KEY_VOLUME: &str = "notification_volume";
const KEY_SOUND_FILE: &str = "notification_sound_file";

/// Colour scheme preference. Tokyo Night (a dark palette) is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Appearance {
    #[default]
    TokyoNight,
    Dark,
    Light,
    System,
}

impl Appearance {
    pub const ALL: [Appearance; 4] = [
        Appearance::TokyoNight,
        Appearance::Dark,
        Appearance::Light,
        Appearance::System,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Appearance::TokyoNight => "Tokyo Night",
            Appearance::Dark => "Dark",
            Appearance::Light => "Light",
            Appearance::System => "Match system",
        }
    }

    /// The libcosmic theme for this preference. Dark/Light pin the preference
    /// so system mode changes do not override it; System follows the desktop.
    pub fn theme(self) -> cosmic::Theme {
        match self {
            Appearance::TokyoNight => crate::style::tokyo_night_theme(),
            Appearance::Dark => {
                let mut theme = cosmic::theme::system_dark();
                theme.theme_type.prefer_dark(Some(true));
                theme
            }
            Appearance::Light => {
                let mut theme = cosmic::theme::system_light();
                theme.theme_type.prefer_dark(Some(false));
                theme
            }
            Appearance::System => cosmic::theme::system_preference(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub appearance: Appearance,
    pub notification_previews: bool,
    pub notification_sound: bool,
    /// Loudness of the notification sound, 0–100.
    pub notification_volume: u8,
    /// Audio file played instead of the built-in chime; empty for the default.
    pub notification_sound_file: String,
    /// The user's GIPHY API key; empty until they add one in Settings.
    pub giphy_api_key: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            appearance: Appearance::default(),
            notification_previews: true,
            notification_sound: true,
            notification_volume: 100,
            notification_sound_file: String::new(),
            giphy_api_key: String::new(),
        }
    }
}

impl Config {
    /// The custom notification sound, if one is set.
    pub fn sound_file(&self) -> Option<std::path::PathBuf> {
        (!self.notification_sound_file.is_empty())
            .then(|| std::path::PathBuf::from(&self.notification_sound_file))
    }

    /// Opens the cosmic-config handle for this app.
    pub fn handler() -> Option<cosmic_config::Config> {
        match cosmic_config::Config::new(APP_ID, CONFIG_VERSION) {
            Ok(handler) => Some(handler),
            Err(err) => {
                tracing::warn!(?err, "cosmic-config unavailable; settings will not persist");
                None
            }
        }
    }

    /// Loads settings, falling back to defaults for missing or invalid keys.
    pub fn load(handler: Option<&cosmic_config::Config>) -> Self {
        let mut config = Self::default();
        let Some(handler) = handler else {
            return config;
        };
        if let Ok(appearance) = handler.get::<Appearance>(KEY_APPEARANCE) {
            config.appearance = appearance;
        }
        // The redesign makes Tokyo Night the default: switch once, then
        // respect whatever the user picks afterwards.
        if handler.get::<u32>(KEY_DESIGN).unwrap_or(0) < DESIGN {
            config.appearance = Appearance::TokyoNight;
            if let Err(err) = handler.set(KEY_DESIGN, DESIGN) {
                tracing::error!(?err, "failed to save the design version");
            }
            if let Err(err) = handler.set(KEY_APPEARANCE, config.appearance) {
                tracing::error!(?err, "failed to save appearance");
            }
        }
        if let Ok(previews) = handler.get::<bool>(KEY_PREVIEWS) {
            config.notification_previews = previews;
        }
        if let Ok(sound) = handler.get::<bool>(KEY_SOUND) {
            config.notification_sound = sound;
        }
        if let Ok(volume) = handler.get::<u8>(KEY_VOLUME) {
            config.notification_volume = volume.min(100);
        }
        if let Ok(file) = handler.get::<String>(KEY_SOUND_FILE) {
            config.notification_sound_file = file;
        }
        if let Ok(key) = handler.get::<String>(KEY_GIPHY) {
            config.giphy_api_key = key;
        }
        config
    }

    pub fn save(&self, handler: Option<&cosmic_config::Config>) {
        let Some(handler) = handler else {
            return;
        };
        if let Err(err) = handler.set(KEY_APPEARANCE, self.appearance) {
            tracing::error!(?err, "failed to save appearance");
        }
        if let Err(err) = handler.set(KEY_PREVIEWS, self.notification_previews) {
            tracing::error!(?err, "failed to save notification previews");
        }
        if let Err(err) = handler.set(KEY_SOUND, self.notification_sound) {
            tracing::error!(?err, "failed to save notification sound");
        }
        if let Err(err) = handler.set(KEY_VOLUME, self.notification_volume) {
            tracing::error!(?err, "failed to save notification volume");
        }
        if let Err(err) = handler.set(KEY_SOUND_FILE, self.notification_sound_file.clone()) {
            tracing::error!(?err, "failed to save notification sound file");
        }
        if let Err(err) = handler.set(KEY_GIPHY, self.giphy_api_key.clone()) {
            tracing::error!(?err, "failed to save the GIPHY API key");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_default_is_dark() {
        assert_eq!(Appearance::default(), Appearance::TokyoNight);
        assert_eq!(Config::default().appearance, Appearance::TokyoNight);
        assert!(Config::default().notification_previews);
        assert!(Config::default().notification_sound);
    }

    #[test]
    fn load_without_handler_gives_defaults() {
        assert_eq!(Config::load(None), Config::default());
    }
}
