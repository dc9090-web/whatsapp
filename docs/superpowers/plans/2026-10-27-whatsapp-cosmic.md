# WhatsApp for COSMIC Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A native libcosmic WhatsApp client that links as a device via QR, shows chats/messages with media and reactions, notifies, and lives in the tray.

**Architecture:** Cargo workspace, single process. `wa-core` (protocol backend behind the `WhatsAppBackend` trait + SQLite store + persisting runtime), `wa-desktop` (notifications, StatusNotifierItem tray, autostart), `wa-app` (libcosmic UI). UI ↔ core via `BackendCommand`/`BackendEvent` channels from `wa_core::start`.

**Tech Stack:** Rust 1.98 stable (MSRV 1.94), edition 2024, libcosmic @ `8990a058`, whatsapp-rust 0.7 (defaults off, no `simd`), rusqlite 0.39 bundled, tokio, notify-rust, ksni, qrcode, image, rodio + opus decode, tracing.

Spec: `docs/superpowers/specs/2026-10-27-whatsapp-cosmic-design.md`

## Global Constraints

- Stable toolchain only. whatsapp-rust MUST keep `default-features = false` (its default `simd` feature requires nightly). Do not add `simd`.
- Shared contracts are fixed: `crates/wa-core/src/{model.rs,events.rs,backend.rs}`, the public signatures in `store.rs`, `runtime.rs`, `backend/fake.rs`, `backend/whatsapp_rust.rs`, and the API documented in `crates/wa-desktop/src/lib.rs`. Implement bodies; do not change signatures. If a contract is genuinely wrong, make the smallest additive change and report it.
- Do NOT add dependencies to `wa-core` that pull UI toolkits. `wa-desktop` must not depend on libcosmic/iced.
- No `unwrap()`/`expect()`/panics on runtime paths (tests are fine). Errors → `tracing` + user-visible message.
- App id: `com.github.whatsapp-cosmic`. Binary: `whatsapp-cosmic`. Data under XDG dirs via `wa_core::paths`.
- Dark theme is the default; Settings offers Dark / Light / Match system.
- Never connect to real WhatsApp servers in automated tests.
- Shared build cache: `export CARGO_TARGET_DIR=$HOME/.cache/whatsapp-cosmic-target` before any cargo command (libcosmic is huge; this avoids N cold builds).
- whatsapp-rust reference: the published source at `~/.cargo/registry/src/index.crates.io-*/whatsapp-rust-0.7.0/` (and sibling `wacore-0.7.0`, `waproto-0.7.0`) is the source of truth; `docs/reference/whatsapp-rust-llms-full.txt` (grep it) documents the API but may describe newer git HEAD.
- libcosmic reference: source in `~/.cargo/git/checkouts/libcosmic-*/8990a05*/` — see its `examples/` (e.g. `application`, `nav-context`) and `cosmic::Application` trait.
- Commit after each task with a conventional message ending in `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

---

### Task 1: Workspace and contracts — DONE (commit b795121)

Workspace, manifests, `model.rs`, `events.rs`, `backend.rs`, `paths.rs`, stubbed `store.rs`/`runtime.rs`/`fake.rs`/`whatsapp_rust.rs`, `wa-desktop` API doc, empty `wa-app` main. `cargo build` passes.

---

### Task 2: Store, runtime, FakeBackend (`wa-core`)

**Files:**
- Modify: `crates/wa-core/src/store.rs`, `crates/wa-core/src/runtime.rs`, `crates/wa-core/src/backend/fake.rs`
- Create: `crates/wa-core/src/store/schema.sql` (embedded with `include_str!`), `crates/wa-core/assets/sample.jpg`, `sample.ogg`, `sample.pdf` (tiny generated files for `--fake` media; may be created by a test helper or committed small binaries)
- Test: `#[cfg(test)] mod tests` in each file; `crates/wa-core/tests/runtime.rs`

**Interfaces:**
- Consumes: `model.rs`, `events.rs`, `backend.rs` (Task 1).
- Produces: everything in the store/runtime/fake contracts, exactly as documented in their doc comments.

Schema (exact):

```sql
CREATE TABLE IF NOT EXISTS chats (
  id TEXT PRIMARY KEY, kind TEXT NOT NULL, name TEXT NOT NULL,
  last_activity INTEGER NOT NULL DEFAULT 0, unread INTEGER NOT NULL DEFAULT 0,
  muted INTEGER NOT NULL DEFAULT 0, last_preview TEXT
);
CREATE TABLE IF NOT EXISTS messages (
  chat TEXT NOT NULL, id TEXT NOT NULL, sender TEXT NOT NULL, sender_name TEXT,
  from_me INTEGER NOT NULL, ts INTEGER NOT NULL, content TEXT NOT NULL, -- serde_json MessageContent
  reply_to TEXT, -- serde_json QuotedMessage
  status TEXT NOT NULL, -- serde_json MessageStatus
  PRIMARY KEY (chat, id)
);
CREATE INDEX IF NOT EXISTS messages_by_ts ON messages(chat, ts);
CREATE TABLE IF NOT EXISTS reactions (
  chat TEXT NOT NULL, target TEXT NOT NULL, sender TEXT NOT NULL,
  emoji TEXT NOT NULL, ts INTEGER NOT NULL, PRIMARY KEY (chat, target, sender)
);
PRAGMA user_version = 1;
```

Status ordering for "never downgrade": use `MessageStatus`'s derived `Ord` (`Failed < Pending < Sent < Delivered < Read < Received`), except `Failed` may replace `Pending`, and `Pending` may replace `Failed` (retry).

- [ ] **Step 1: Write failing store tests** (in `store.rs`), at least:
  - `upsert_message_is_idempotent` — insert same message twice → first returns `true`, second `false`, `messages()` has 1 row, chat `unread == 1`.
  - `upsert_creates_placeholder_chat` — message for unknown chat `15551234567@s.whatsapp.net` → `chat()` returns name `"15551234567"`, kind `Direct`; for `...@g.us` kind `Group`.
  - `from_me_does_not_increment_unread`.
  - `older_message_does_not_bump_last_activity` — insert ts=200 then ts=100 → `last_activity == 200`, preview of the ts=200 message.
  - `chats_sorted_by_recent_activity`.
  - `messages_pagination` — 120 messages ts 1..=120; `messages(None,50)` → ts 71..=120 ascending; `messages(Some(71),50)` → 21..=70; `messages(Some(21),50)` → 1..=20.
  - `status_never_downgrades` — Read then set Delivered → stays Read; Pending → Failed allowed; Failed → Pending allowed.
  - `replace_message_id_rekeys_message_and_reactions`.
  - `reaction_upsert_and_removal` — same sender reacts 👍 then ❤️ → one row ❤️; empty emoji → zero rows.
  - `set_media_path_updates_local_path` — round-trips through `message()`.
  - `mark_read_and_total_unread_ignores_muted`.
  - `wipe_clears_everything`.
  - `open_file_db_persists` — using `tempfile::tempdir()`, write, drop, reopen, read.
- [ ] **Step 2:** `cargo test -p wa-core store` → FAIL (todo! panics).
- [ ] **Step 3:** Implement `Store` (rusqlite, `journal_mode=WAL`, `busy_timeout=5000`, schema from `schema.sql`).
- [ ] **Step 4:** `cargo test -p wa-core store` → PASS.
- [ ] **Step 5: FakeBackend** — implement `run`: emit `script` in order (small `tokio::time::sleep` of 0 between is fine), then loop on `commands`; forward each to `received`; if `auto_reply`: `SendText` → `MessageSent{id: "fake-<n>"}` then after 300ms `MessageStatus Delivered`, after 1s `Read`; `React` → `Reaction` echo with sender `"me"`; `FetchMedia` → write the matching sample asset into a temp/media dir and emit `MediaReady`; `Logout` → `LoggedOut`; `Shutdown` → return `Ok(())`. Implement `demo()` per its doc comment with realistic names and timestamps relative to now; include at least one muted chat and one chat with unread > 0.
- [ ] **Step 6: Runtime tests** in `crates/wa-core/tests/runtime.rs` (use `#[tokio::test]`, in-memory store via a second `Store` handle on a temp file to verify persistence):
  - `send_text_is_optimistic_then_sent` — FakeBackend auto_reply; send `SendText{local_id}` → first event is `Message` with `status: Pending` and that `local_id`; then `MessageSent`; store now has message under `fake-1` with status ≥ Sent and no row under `local_id`.
  - `incoming_message_persisted_before_forwarded` — scripted `Message` → when the UI receives the event, a separate `Store::open` on the same file already returns it.
  - `logout_wipes_store` — Logout → LoggedOut forwarded → store empty.
  - `logged_out_without_request_keeps_store`.
- [ ] **Step 7:** Implement `runtime::start` per its doc comment (two tasks: command pump, event pump; store moved into the event pump task; command pump sends store work to it via an internal channel so a single connection owns writes). Run `cargo test -p wa-core` → PASS. `cargo clippy -p wa-core -- -D warnings` → clean.
- [ ] **Step 8: Commit** `feat(core): sqlite store, persisting runtime, fake backend`.

---

### Task 3: whatsapp-rust backend (`wa-core`)

**Files:**
- Modify: `crates/wa-core/src/backend/whatsapp_rust.rs`
- Create: `crates/wa-core/src/backend/whatsapp_rust/convert.rs` (protocol → model mapping, pure functions), `crates/wa-core/examples/wa_cli.rs` (headless manual harness)
- Test: unit tests in `convert.rs`

**Interfaces:**
- Consumes: `WhatsAppBackend`, `BackendCommand`, `BackendEvent`, model types.
- Produces: `WhatsAppRustBackend::new(session_db: PathBuf, media_dir: PathBuf)` implementing `WhatsAppBackend`.

Known whatsapp-rust 0.7 API (verify in source): `SqliteStore::open(path).await`, `Bot::builder().with_backend(..).on_qr_code(|code, timeout| ..).on_connected(..).on_logged_out(..).on_message(|ctx| ..).on_event(..).build().await`, `bot.client()`, `bot.spawn()` / `bot.run()`, `ctx.message.text_content()`, `ctx.info.source.sender`, `ctx.react(..)`, `ctx.reply_quoting(..)`, `Client::send_message`, `features/reaction.rs::send_reaction`, `receipt.rs::mark_as_read`, `download.rs::download(&dyn Downloadable)` / `download_from_params`, `client/lifecycle.rs::logout`. `on_event` delivers the `Event` enum (history sync, receipts, connection changes, contact/group names) — grep `pub enum Event` in wacore.

- [ ] **Step 1: Write failing `convert.rs` tests** — build waproto messages in-test and assert mapping: plain conversation text → `Text`; extended text with context info → `Text` + `reply_to` (quoted id, sender, snippet); image message → `Image{caption, media.mime, size, thumbnail = jpeg_thumbnail}` with non-empty `download_token`; sticker; audio with `ptt=true` → `voice_note: true, duration_secs`; document → `file_name`; reaction message → returns a `Reaction` (not a `Message`); unknown/poll → `Unsupported{kind}`; timestamps converted to unix ms; group vs direct `ChatId` from the chat JID (not the participant JID).
- [ ] **Step 2:** Run tests → FAIL. Implement `convert.rs`. Choose `download_token` = protobuf-encoded media sub-message (so `FetchMedia` can decode it and call `download`). Run → PASS.
- [ ] **Step 3: Implement `run`:** open `SqliteStore` at `session_db`; build bot wiring every hook to send `BackendEvent`s through a cloned `events` sender (`Qr`, `Paired`, `Connection(Connected)`, `LoggedOut`, `Message`/`Reaction` via convert, receipts → `MessageStatus`, history sync → `ChatUpserted` + `Message`s + `SyncProgress`/`SyncDone`, contact/group name updates → `ChatUpserted`, mute state if exposed). Spawn the bot; `select!` over commands: `SendText` (with quoted context when `reply_to` is set; on success `MessageSent`, on error `SendFailed`), `React`, `MarkRead`, `FetchMedia{media}` (decode `media.download_token`, download + decrypt; on error `MediaFailed`; write decrypted bytes to `media_dir/<sha-or-id>.<ext>` then `MediaReady`), `Logout` (call logout, delete `session_db`, emit `LoggedOut`), `Shutdown` (graceful shutdown, return). If the library does not auto-reconnect, implement exponential backoff 1s→60s with `Connection(Reconnecting{..})`; if it does, map its state events to `Connection`.
- [ ] **Step 4: `examples/wa_cli.rs`** — runs `wa_core::start(WhatsAppRustBackend::new(..), Store::open(..))` with paths under a `--dir` arg, prints the QR as unicode (use `qrcode` crate as a dev-dependency) and logs every event; reads stdin lines `send <jid> <text>`. This is for manual end-to-end checks by the user; do NOT run it against WhatsApp yourself.
- [ ] **Step 5:** `cargo test -p wa-core`, `cargo build -p wa-core --examples`, `cargo clippy -p wa-core -- -D warnings` → all pass.
- [ ] **Step 6: Commit** `feat(core): whatsapp-rust backend`.

Note: Task 2 and Task 3 both touch `wa-core`, but disjoint files.

---

### Task 4: Desktop integration (`wa-desktop`)

**Files:**
- Modify: `crates/wa-desktop/Cargo.toml`, `crates/wa-desktop/src/lib.rs`
- Create: `crates/wa-desktop/src/notify.rs`, `tray.rs`, `autostart.rs`, `crates/wa-desktop/examples/tray_demo.rs`

**Interfaces:** Exactly the API in `crates/wa-desktop/src/lib.rs` doc comment (keep the doc, replace the `ignore` block with real modules).

Deps: `notify-rust` (with `d` / zbus feature, async), `ksni` (tokio feature), `dirs`.

- [ ] **Step 1: Failing autostart tests** using an injectable base dir (`fn set_enabled_in(dir: &Path, ..)`, `fn is_enabled_in(dir: &Path)`, public functions call these with `dirs::config_dir()/autostart`): enable writes a file containing `Exec=whatsapp-cosmic --background`, `Type=Application`, `X-GNOME-Autostart-enabled=true`; `is_enabled_in` true; disable removes it; disable when missing is Ok.
- [ ] **Step 2:** Implement autostart → tests PASS.
- [ ] **Step 3: notify** — `notify_rust::Notification` with app name "WhatsApp", icon `com.github.whatsapp-cosmic`, action `default` → on click send `chat_id` through `on_click`. Wait for the action on a spawned tokio task (`show_async` + `wait_for_action` on a blocking thread if needed). Unit-test only the pure helper that truncates body to 200 chars on a char boundary.
- [ ] **Step 4: tray** — ksni `Tray` impl: id `com.github.whatsapp-cosmic`, title "WhatsApp", `icon_name` `com.github.whatsapp-cosmic` (fallback `mail-message-new-symbolic` if not installed), `activate` → `TrayEvent::Show`, menu: "Show WhatsApp", separator, "Quit" → `TrayEvent::Quit`. `set_unread(n)`: tooltip "N unread messages", and status `NeedsAttention` when n>0 (`Active` otherwise); also provide an `icon_pixmap` badge: render the count as a red circle with white digits onto a 32×32 ARGB32 buffer (pure function `badge_pixmap(count) -> Vec<u8>` — unit test: n=0 → None, n=5 → 32*32*4 bytes with red pixels at center, n>99 renders "99+"). Use a tiny embedded 3×5 digit bitmap font so no font dependency is needed.
- [ ] **Step 5: `examples/tray_demo.rs`** shows the tray with a counter that increments every 2s and sends a notification; Quit exits. (Manual check by user.)
- [ ] **Step 6:** `cargo test -p wa-desktop`, `cargo clippy -p wa-desktop -- -D warnings` → pass.
- [ ] **Step 7: Commit** `feat(desktop): notifications, tray, autostart`.

---

### Task 5: libcosmic UI (`wa-app`)

**Files:**
- Modify: `crates/wa-app/Cargo.toml`, `crates/wa-app/src/main.rs`
- Create: `src/app.rs`, `src/config.rs`, `src/audio.rs`, `src/backend_sub.rs`, `src/pages/{mod,login,sync,chats,conversation,settings}.rs`, `src/widgets/{mod,bubble,media,reactions}.rs`

**Interfaces:**
- Consumes: `wa_core::{start, Handle, Store, BackendCommand, BackendEvent, ConnectionState, model::*, paths, backend::fake::FakeBackend, backend::whatsapp_rust::WhatsAppRustBackend}`; `wa_desktop::{notify, tray::{Tray, TrayEvent}, autostart}`.
- Produces: binary `whatsapp-cosmic` with flags `--fake` (FakeBackend::demo, store in a temp dir) and `--background` (start with no window, tray only).

Deps: `libcosmic` (workspace), `qrcode`, `image` (png/jpeg/webp for stickers), `rodio` (default-features off + `playback`) and an Opus/Ogg decoder (`ogg` + `opus`/`audiopus`, or `symphonia` if it supports Opus at this time — pick what builds on stable and decodes WhatsApp's OGG/Opus voice notes), `open` (xdg-open), `tracing-subscriber`, `tracing-appender`, `serde`.

Behaviour (from spec):
- `cosmic::Application` with app id `com.github.whatsapp-cosmic`. Header bar title "WhatsApp"; menu/⋮ with Settings and Quit.
- Theme: on startup apply `config.appearance` (default `Dark`) via `cosmic::theme::system_dark()` / `system_light()` / system preference; change live from Settings. Persist config with `cosmic-config` (`cosmic::cosmic_config`), fields: `appearance: Appearance{Dark,Light,System}` (default Dark), `notification_previews: bool` (default true). Start-at-login reads/writes via `wa_desktop::autostart`.
- Backend events arrive through an iced `Subscription` built in `backend_sub.rs`: on first poll it calls `wa_core::start(..)` (tokio runtime is available because libcosmic `tokio` feature is on), stores the `commands` sender by emitting `Message::BackendReady(Sender)`, then streams `BackendEvent`s as `Message::Backend(ev)`.
- UI reads data through its own `Store::open(paths::app_db())` (separate connection) on events: reload chat list on `ChatUpserted`/`Message`/`MessageStatus`/`Reaction`/`SyncDone`; reload open conversation page when the event's chat matches.
- Pages: Login (QR via `qrcode` → `image::Rgba` → `cosmic::widget::image::Handle::from_rgba`; instructions text; replaced on every `Qr`); Sync (progress bar from `SyncProgress`); Main two-pane: chat list (name, preview, time, unread pill, muted icon; selection) + conversation (scrollable, bubbles, date separators, load-older on scroll top via `scrollable::on_scroll` near 0 → `store.messages(before=oldest_ts, 50)`); composer `text_input` with send button and Enter-to-send; reply banner with ✕ when replying.
- Bubble: incoming left / outgoing right with accent background; sender name in groups; quoted snippet block; text; timestamp + status glyph (🕓 ✓ ✓✓, accent ✓✓ for Read, ⚠ Failed with "Retry" button that re-sends `SendText` with a new local id); reaction chips below (emoji + count); hover/right-click context menu → Reply, React (6 quick emojis 👍 ❤️ 😂 😮 😢 🙏 + "More…" opening an emoji grid), Copy text.
- Media widget: image/sticker → thumbnail from `MediaRef.thumbnail` or `local_path`; click → `FetchMedia{chat, message, media}` if not local, then open in a full-size overlay/dialog; voice note → play/pause button + duration, fetch on first play, playback via `audio.rs` (rodio on its own thread, `Message::AudioFinished`); document/video → name, size, "Download"/"Open" (`open::that(local_path)`); `MediaFailed` → "Retry".
- Unsupported → grey italic "Unsupported message — open WhatsApp on your phone".
- Connection banner for `Connecting`/`Reconnecting{retry_in_secs}`; dismisses on `Connected`.
- `LoggedOut` → Login page (keep store unless user chose Log out).
- Mark read: when a chat is opened or a new message arrives in the open, focused chat → `MarkRead` with visible incoming ids.
- Notifications: on `BackendEvent::Message` where `!from_me`, chat not muted, and (window unfocused or chat not open) → `wa_desktop::notify::show(chat name, preview if enabled, chat id, click_tx)`. Click → show window + open chat.
- Tray: spawn at startup; `set_unread(store.total_unread())` after every reload; `TrayEvent::Show` → open/raise main window; `Quit` → send `Shutdown`, exit.
- Close window → hide (destroy main window but keep the app running; libcosmic `multi-window` + `cosmic::Application::on_close_requested`/`window::close` pattern — research in libcosmic source); `--background` starts without a window.
- Logging: `tracing-subscriber` + `tracing-appender` daily file in `paths::state_dir()/log`, `RUST_LOG` env filter, default `info`.

- [ ] **Step 1: Failing tests for pure UI logic** in `src/app.rs`/`src/pages/*.rs` `#[cfg(test)]`: `format_time` (today → "HH:MM", yesterday → "Yesterday", this week → weekday, else `dd/mm/yyyy`); `status_glyph`; `group_reactions` (Vec<Reaction> → Vec<(emoji,count,mine)> sorted by count desc); `should_notify(msg, chat, focused, open_chat, muted)` truth table; `date_separator_needed(prev_ts, ts)`; `Appearance` default is `Dark`; `App::update` with `Message::Backend(Qr{..})` switches to Login page; `Message::Backend(SyncDone)` switches to Main.
- [ ] **Step 2:** `cargo test -p wa-app` → FAIL. Implement helpers and the state machine. → PASS.
- [ ] **Step 3:** Build the full UI. `cargo build -p wa-app` → success. `cargo clippy -p wa-app -- -D warnings` → clean.
- [ ] **Step 4: Smoke run** under the live Wayland session: `timeout 15 cargo run -p wa-app -- --fake` → no panic in output; take a screenshot with `cosmic-screenshot --interactive=false --save-dir <scratch>` (or `grim` if available) and inspect it; verify dark theme, chat list populated, conversation renders. Iterate on layout issues found.
- [ ] **Step 5: Commit** `feat(app): libcosmic UI`.

---

### Task 6: Integration, resources, packaging

**Files:**
- Create: `resources/com.github.whatsapp-cosmic.desktop`, `resources/icons/hicolor/scalable/apps/com.github.whatsapp-cosmic.svg` (original simple speech-bubble+phone glyph in green; NOT the WhatsApp logo), `resources/com.github.whatsapp-cosmic.metainfo.xml`, `packaging/PKGBUILD`, `justfile` (`build`, `run`, `run-fake`, `install` to `$DESTDIR/usr`), `README.md` (build/run, `--fake`, ToS disclaimer, manual E2E checklist from the spec)
- Modify: whatever is needed to make Tasks 2–5 work together

- [ ] **Step 1:** Merge task branches; resolve conflicts (regenerate `Cargo.lock` with `cargo update -w` if it conflicts).
- [ ] **Step 2:** `cargo build --workspace`, `cargo test --workspace`, `cargo clippy --workspace -- -D warnings` → all pass.
- [ ] **Step 3:** Fix every contract mismatch between UI ↔ runtime ↔ backend; run `--fake` smoke test with screenshot as in Task 5 Step 4, exercise: open a chat, send a text (watch Pending → Sent → Delivered → Read), react, open image, play voice note, close window (process stays, tray present), Show from tray.
- [ ] **Step 4:** Resources + PKGBUILD (`makepkg --printsrcinfo` sanity check; `pkgname=whatsapp-cosmic-git`, `depends=(gcc-libs glibc libxkbcommon wayland opus)`, `makedepends=(rust cargo git just)`, build with `cargo build --release --locked`, install binary, desktop file, icon, metainfo).
- [ ] **Step 5: Commit** `feat: integrate, resources, packaging`.

---

### Task 7: Review and verification

- [ ] Independent reviews: correctness/contract adherence, error handling (no panics on runtime paths, no silent drops), UI vs spec. Fix confirmed findings.
- [ ] Final: `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo build --release`, `--fake` smoke screenshot. Report results honestly, including anything not verified (real-account E2E is the user's manual step).
