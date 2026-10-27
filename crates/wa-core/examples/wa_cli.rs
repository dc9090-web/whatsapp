//! Headless manual harness for the real WhatsApp backend.
//!
//! ```text
//! cargo run -p wa-core --example wa_cli -- --dir /tmp/wa-cli
//! ```
//!
//! Prints the pairing QR as unicode, logs every backend event, and reads
//! commands from stdin:
//!
//! ```text
//! send <jid> <text>      e.g. send 15551234567@s.whatsapp.net hello
//! react <jid> <msg-id> <emoji>
//! logout
//! quit
//! ```
//!
//! This talks to the real WhatsApp servers; it is for the user's manual
//! end-to-end checks only and is never run by automated tests.

use std::path::PathBuf;

use qrcode::QrCode;
use qrcode::render::unicode::Dense1x2;
use wa_core::backend::whatsapp_rust::WhatsAppRustBackend;
use wa_core::{BackendCommand, BackendEvent, ChatId, MessageId, Store};

fn parse_dir() -> PathBuf {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--dir"
            && let Some(dir) = args.next()
        {
            return PathBuf::from(dir);
        }
        if let Some(dir) = arg.strip_prefix("--dir=") {
            return PathBuf::from(dir);
        }
    }
    std::env::temp_dir().join("wa-cli")
}

fn print_qr(code: &str) {
    match QrCode::new(code.as_bytes()) {
        Ok(qr) => {
            let art = qr.render::<Dense1x2>().quiet_zone(true).build();
            println!("{art}");
            println!("Scan with WhatsApp > Linked devices > Link a device");
        }
        Err(e) => println!("could not render QR ({e}); raw payload: {code}"),
    }
}

fn parse_command(line: &str) -> Option<BackendCommand> {
    let line = line.trim();
    let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
    match verb {
        "send" => {
            let (jid, text) = rest.trim().split_once(' ')?;
            Some(BackendCommand::SendText {
                chat: ChatId(jid.to_owned()),
                text: text.to_owned(),
                reply_to: None,
                local_id: MessageId::new_local(),
            })
        }
        "react" => {
            let mut parts = rest.split_whitespace();
            let jid = parts.next()?;
            let id = parts.next()?;
            let emoji = parts.next().unwrap_or("");
            Some(BackendCommand::React {
                chat: ChatId(jid.to_owned()),
                target: MessageId(id.to_owned()),
                emoji: emoji.to_owned(),
            })
        }
        "logout" => Some(BackendCommand::Logout),
        "quit" | "exit" => Some(BackendCommand::Shutdown),
        _ => None,
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let dir = parse_dir();
    std::fs::create_dir_all(&dir)?;
    println!("state directory: {}", dir.display());

    let backend = WhatsAppRustBackend::new(dir.join("session.db"), dir.join("media"));
    let store = Store::open(&dir.join("app.db"))?;
    let mut handle = wa_core::start(Box::new(backend), store);

    // stdin is read on a plain thread so no extra tokio features are needed.
    let (line_tx, mut line_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut line = String::new();
        while stdin.read_line(&mut line).is_ok_and(|n| n > 0) {
            if line_tx.send(std::mem::take(&mut line)).is_err() {
                break;
            }
        }
    });

    loop {
        tokio::select! {
            ev = handle.events.recv() => {
                let Some(ev) = ev else {
                    println!("backend stopped");
                    break;
                };
                match &ev {
                    BackendEvent::Qr { code, valid_for_secs } => {
                        println!("QR (valid {valid_for_secs}s):");
                        print_qr(code);
                    }
                    BackendEvent::Message(m) => println!(
                        "[{}] {} {}: {} (id {})",
                        m.chat.0,
                        if m.from_me { "me" } else { m.sender_name.as_deref().unwrap_or(&m.sender) },
                        m.timestamp,
                        m.content.preview(),
                        m.id.0
                    ),
                    other => println!("{other:?}"),
                }
            }
            line = line_rx.recv() => {
                let Some(line) = line else { continue };
                match parse_command(&line) {
                    Some(cmd) => {
                        let quit = cmd == BackendCommand::Shutdown;
                        if handle.commands.send(cmd).await.is_err() {
                            break;
                        }
                        if quit {
                            // Give the backend a moment to disconnect cleanly.
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                            break;
                        }
                    }
                    None if line.trim().is_empty() => {}
                    None => println!("commands: send <jid> <text> | react <jid> <id> <emoji> | logout | quit"),
                }
            }
        }
    }
    Ok(())
}
