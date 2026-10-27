//! Manual check: shows one notification (optionally with a picture) and
//! waits up to 15 s for it to be clicked.
//!
//! `cargo run -p wa-desktop --example notify_demo -- [picture.jpg]`

use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let picture = std::env::args().nth(1).map(std::path::PathBuf::from);
    let (tx, mut rx) = mpsc::unbounded_channel();
    wa_desktop::notify::prime();
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    wa_desktop::notify::show(
        "Alice Johnson",
        Some("Hey! Are we still on for dinner tomorrow?"),
        "15550101001@s.whatsapp.net".into(),
        picture.as_deref(),
        tx,
    )?;
    println!("notification sent; click it within 15 s");
    match tokio::time::timeout(std::time::Duration::from_secs(15), rx.recv()).await {
        Ok(Some(chat)) => println!("clicked: {chat}"),
        _ => println!("not clicked"),
    }
    Ok(())
}
