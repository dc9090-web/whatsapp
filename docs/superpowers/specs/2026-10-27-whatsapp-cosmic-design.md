# WhatsApp for COSMIC — Design Spec

Date: 2026-10-27
Status: Approved (brainstorming)

## Goal

A native Rust WhatsApp desktop client for the COSMIC desktop (Wayland, CachyOS), built with libcosmic. It links to the user's personal WhatsApp account as a "linked device" via QR code, using the unofficial pure-Rust `whatsapp-rust` protocol library.

**Known risk:** unofficial clients violate WhatsApp ToS; account bans are possible (rare for normal personal use). Accepted by the user.

## Scope (v1)

In:
- QR-code login as linked device; session persists across restarts
- Chat list (1:1 + groups), unread counts, sorted by last activity
- Conversation view: text, replies/quotes, timestamps, delivery/read ticks
- Send text messages (incl. replying to a message)
- Receive media: inline images & stickers, voice-note playback, document download/open
- Reactions: display and send
- Native desktop notifications (freedesktop, via `notify-rust`)
- Background mode: closing the window hides it; StatusNotifierItem tray icon with unread badge; optional start-at-login
- Local message history in SQLite
- Dark mode by default; setting: Dark (default) / Light / Match system

Out (v1): calls, status/stories, sending media (v2), search (v2), group creation/admin, multi-account, Flatpak.

## Architecture

Single process. A Cargo workspace with three crates:

```
whatsapp-cosmic/
├── Cargo.toml                 # workspace
├── crates/
│   ├── wa-core/               # protocol + storage, NO UI deps
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── model.rs       # ChatId, MessageId, Chat, Message, MessageContent, Receipt, Reaction, MediaRef
│   │       ├── events.rs      # BackendEvent, BackendCommand
│   │       ├── backend.rs     # WhatsAppBackend trait + spawn()
│   │       ├── backend/whatsapp_rust.rs  # real impl over whatsapp-rust 0.7
│   │       ├── backend/fake.rs           # FakeBackend: scripted events, records commands
│   │       ├── store.rs       # our SQLite store (rusqlite), chats/messages/reactions/media index
│   │       ├── media.rs       # download/decrypt/cache media under XDG cache
│   │       └── paths.rs       # XDG data/cache/state dirs
│   ├── wa-desktop/            # desktop integration, NO UI toolkit deps
│   │   └── src/
│   │       ├── notify.rs      # notifications, click → open chat
│   │       ├── tray.rs        # ksni StatusNotifierItem, unread badge, Show/Quit menu
│   │       └── autostart.rs   # ~/.config/autostart/*.desktop toggle
│   └── wa-app/                # libcosmic binary `whatsapp-cosmic`
│       └── src/
│           ├── main.rs        # args: --background, --fake
│           ├── app.rs         # cosmic::Application, top-level Message, state
│           ├── config.rs      # settings (theme, autostart, notification previews) via cosmic-config
│           ├── audio.rs       # voice-note playback (rodio + opus decode)
│           ├── pages/{login.rs, sync.rs, chats.rs, conversation.rs, settings.rs}
│           └── widgets/{bubble.rs, media.rs, reactions.rs}
├── resources/                 # .desktop, icons, metainfo
└── packaging/PKGBUILD
```

### Communication

- `wa-core` runs one long-lived tokio task owning the protocol client.
- UI → backend: `BackendCommand` over `tokio::sync::mpsc` (SendText{chat, text, reply_to}, React{chat, msg, emoji}, MarkRead{chat, up_to}, FetchMedia{msg}, Logout).
- Backend → UI: `BackendEvent` over mpsc, surfaced to iced as a `Subscription` (Qr(String), Paired, Connecting, Connected, Disconnected{reason}, LoggedOut, SyncProgress(f32), SyncDone, ChatUpserted(Chat), Message(Message), Receipt{..}, Reaction{..}, MediaReady{msg, path}, SendFailed{..}, Error(String)).
- The UI never touches the protocol. It renders state from events plus queries on `store`.
- `WhatsAppBackend` trait lets `FakeBackend` replace the real impl (tests and `--fake`), and allows swapping in a whatsmeow sidecar later.

### Storage

- Protocol/crypto state: `whatsapp-rust-sqlite-storage` at `~/.local/share/whatsapp-cosmic/session.db`.
- App data: our own `~/.local/share/whatsapp-cosmic/app.db` (rusqlite, bundled), tables `chats`, `messages`, `reactions`, `media`. Messages upserted by (chat_id, message_id) so history-sync duplicates are harmless.
- Media cache: `~/.cache/whatsapp-cosmic/media/`.
- Logs: `~/.local/state/whatsapp-cosmic/log` via `tracing` + `tracing-appender`.

## UI

COSMIC two-pane layout: chat list (left), conversation (right), COSMIC header bar.

Flows:
1. **First run**: Login page shows QR (`qrcode` crate → image) with instructions; auto-refreshes on new `Qr` events. After pairing: "Syncing history…" progress page until `SyncDone`, then chat list.
2. **Later starts**: open straight into chat list from `app.db`; reconnect in background with a "Connecting…" banner.
3. **Conversation**: paginated 50 messages, older on scroll-up. Replies show a quoted snippet. Hover/right-click a message → Reply / React (6 quick emojis + full picker). Images/stickers as inline thumbnails, click to open full size. Voice notes: play/pause + duration. Documents: name + size + Download/Open (`xdg-open`). Outgoing bubbles use the accent colour.
4. **Close**: hides window; process stays connected. Tray menu: Show / Quit. Tray icon shows unread count.
5. **Notifications**: on incoming message when the window is unfocused or that chat isn't open; respect WhatsApp mute state; clicking opens that chat. Preview text can be disabled.

Settings: Appearance (Dark default / Light / Match system — applied live via `cosmic::theme::system_dark()`/`light()`), Start at login, Notification previews, Log out (unlinks and wipes local data).

## Error handling

- Connection drop: auto-reconnect with exponential backoff (1s → 60s cap); non-blocking "Reconnecting…" banner.
- Remote logout: clear session, return to QR login; keep `app.db` until explicit Log out.
- Library errors: logged; user sees actionable message; no panics on the UI path.
- Media download failure: retry button on the media widget.
- Outgoing message states: Pending 🕓 → Sent ✓ → Delivered ✓✓ → Read (accent ✓✓); Failed ⚠ with Retry.
- Unsupported message types: grey placeholder "Unsupported message — open WhatsApp on your phone", never silently dropped.

## Testing

- `wa-core`: unit tests for `store` (schema, upsert idempotence, pagination, unread counts), protocol→model mapping, backoff logic. `FakeBackend` drives scripted event sequences.
- `wa-app`: update-logic tests against `FakeBackend`, no network. `--fake` flag runs the full UI against fake data.
- End-to-end: manual checklist with a real account (pair, receive, send, react, media, reconnect, remote logout). No automated tests against real WhatsApp servers.

## Packaging

`cargo build --release`; `resources/` has `.desktop` file, icon, metainfo; `packaging/PKGBUILD` for pacman install on CachyOS. Flatpak later.
