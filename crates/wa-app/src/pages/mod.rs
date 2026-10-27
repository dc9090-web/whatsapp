//! Top-level screens. Each module renders from `&App` and holds the pure
//! helpers its view needs.

pub mod chats;
pub mod conversation;
pub mod group;
pub mod login;
pub mod settings;
pub mod sync;

/// Which screen the main window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// QR pairing (or waiting for the first QR / connection).
    Login,
    /// Initial history sync after pairing.
    Sync,
    /// Two-pane chat list + conversation.
    Main,
}
