//! User settings persisted with `cosmic-config` under the app id.

use cosmic::cosmic_config::{self, ConfigGet, ConfigSet};
use serde::{Deserialize, Serialize};

use crate::app::APP_ID;

pub const CONFIG_VERSION: u64 = 1;

const KEY_APPEARANCE: &str = "appearance";
const KEY_PREVIEWS: &str = "notification_previews";

/// Colour scheme preference. Dark is the default, as the spec requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Appearance {
    #[default]
    Dark,
    Light,
    System,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Appearance::Dark, Appearance::Light, Appearance::System];

    pub const fn label(self) -> &'static str {
        match self {
            Appearance::Dark => "Dark",
            Appearance::Light => "Light",
            Appearance::System => "Match system",
        }
    }

    /// The libcosmic theme for this preference. Dark/Light pin the preference
    /// so system mode changes do not override it; System follows the desktop.
    pub fn theme(self) -> cosmic::Theme {
        match self {
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            appearance: Appearance::default(),
            notification_previews: true,
        }
    }
}

impl Config {
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
        if let Ok(previews) = handler.get::<bool>(KEY_PREVIEWS) {
            config.notification_previews = previews;
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_default_is_dark() {
        assert_eq!(Appearance::default(), Appearance::Dark);
        assert_eq!(Config::default().appearance, Appearance::Dark);
        assert!(Config::default().notification_previews);
    }

    #[test]
    fn load_without_handler_gives_defaults() {
        assert_eq!(Config::load(None), Config::default());
    }
}
