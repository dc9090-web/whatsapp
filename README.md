# WhatsApp for COSMIC

An unofficial, native WhatsApp client for the [COSMIC](https://github.com/pop-os/cosmic-epoch)
desktop, written in Rust with libcosmic. It pairs with your phone as a linked
device (QR code) using the [`whatsapp-rust`](https://crates.io/crates/whatsapp-rust) library.

![Fake-mode screenshot](docs/screenshots/fake-mode.png)

> **Disclaimer.** This project is not affiliated with, authorised or endorsed by
> WhatsApp or Meta. Unofficial clients violate the WhatsApp Terms of Service and
> your account could be banned. Use it at your own risk.

## Features

- QR-code login as a linked device; the session survives restarts
- Chat list (direct chats and groups) with unread counts
- Conversations with replies, delivery/read ticks, reactions (view and send)
- Inline images and stickers, voice-note playback (OGG/Opus), documents
- Desktop notifications, tray icon with unread badge, close-to-tray, optional start at login
- Local history in SQLite; dark theme by default (Dark / Light / Match system)

## Build and run

Requires stable Rust (1.94+), `cmake` (for the bundled libopus build), and the
usual Wayland development libraries (`libxkbcommon`, `wayland`).

```sh
just build          # cargo build --release --locked -p wa-app
just run            # real backend (debug build)
just run-fake       # scripted demo backend, throwaway database, no network
just check          # tests + clippy
sudo just install   # installs to /usr (respects DESTDIR)
```

Without `just`: `cargo run -p wa-app -- --fake`.

Command-line flags of `whatsapp-cosmic`:

- `--fake` — run against the built-in demo backend (no network, temporary database)
- `--background` — start hidden with only the tray icon (used by start-at-login)

Data locations: `~/.local/share/whatsapp-cosmic/` (session and messages),
`~/.cache/whatsapp-cosmic/media/` (downloaded media), `~/.local/state/whatsapp-cosmic/log`.

A command-line pairing tool for testing the backend on its own is in
`crates/wa-core/examples/wa_cli.rs` (`cargo run -p wa-core --example wa_cli`).

### Arch / CachyOS package

```sh
cd packaging && makepkg -si
```

The PKGBUILD builds `whatsapp-cosmic-git` from this repository.

## Layout

- `crates/wa-core` — model, SQLite store, runtime, `whatsapp-rust` backend and the fake backend
- `crates/wa-desktop` — notifications, StatusNotifierItem tray, autostart (no UI toolkit)
- `crates/wa-app` — the libcosmic application (`whatsapp-cosmic` binary)

## Manual end-to-end checklist (real account)

Automated tests never talk to WhatsApp servers, so check these by hand:

- [ ] **Pair:** start the app, scan the QR code from WhatsApp → Linked devices; history sync completes and the chat list appears
- [ ] **Restart:** quit and start again; no QR code, chats load from the local database
- [ ] **Receive:** a message from another phone appears in the list and the open conversation; a notification shows when the window is not focused; the tray badge updates
- [ ] **Send:** send a text; ticks go 🕓 → ✓ → ✓✓ → read; reply to a message and check the quote on the phone
- [ ] **React:** react from the app and from the phone; both show, and your own reaction is highlighted
- [ ] **Media:** an incoming image and sticker display; a voice note plays; a document downloads and opens
- [ ] **Reconnect:** disable networking for a minute, re-enable; the app reconnects and catches up
- [ ] **Background:** close the window; the process and tray icon stay; "Show" from the tray brings it back
- [ ] **Remote logout:** remove the linked device on the phone; the app returns to the QR screen
- [ ] **Log out** from the app's menu wipes the local data

## License

GPL-3.0-or-later.
