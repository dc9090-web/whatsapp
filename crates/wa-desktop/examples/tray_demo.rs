//! Manual check for the tray + notifications.
//!
//! `cargo run -p wa-desktop --example tray_demo`
//!
//! A tray icon appears; every 2 s the unread counter increments (tooltip and
//! badge update) and a notification is shown. Clicking a notification prints
//! its chat id; "Show WhatsApp" / left click prints Show; "Quit" exits.

use std::time::Duration;

use tokio::sync::mpsc;
use wa_desktop::{
    notify,
    tray::{Tray, TrayEvent},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (tray_tx, mut tray_rx) = mpsc::unbounded_channel();
    let (click_tx, mut click_rx) = mpsc::unbounded_channel();

    let tray = Tray::spawn(tray_tx).await?;
    println!("tray registered; use its menu to quit");

    let mut ticker = tokio::time::interval(Duration::from_secs(2));
    ticker.tick().await; // first tick fires immediately
    let mut count = 0u32;

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                count += 1;
                tray.set_unread(count).await;
                let body = format!("Demo message number {count}");
                if let Err(e) = notify::show("Demo chat", Some(&body), format!("demo-{count}@s.whatsapp.net"), click_tx.clone()) {
                    eprintln!("notification failed: {e:#}");
                }
                println!("unread = {count}");
            }
            Some(event) = tray_rx.recv() => match event {
                TrayEvent::Show => println!("tray: Show"),
                TrayEvent::Quit => {
                    println!("tray: Quit");
                    break;
                }
            },
            Some(chat) = click_rx.recv() => println!("notification clicked: {chat}"),
        }
    }
    Ok(())
}
