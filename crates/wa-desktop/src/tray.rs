//! StatusNotifierItem tray icon (ksni) with an unread badge and Show/Quit menu.

use std::path::{Path, PathBuf};

use ksni::TrayMethods as _;
use tokio::sync::mpsc::UnboundedSender;

use crate::APP_ID;

/// Icon used when the app icon is not installed in any icon theme directory.
const FALLBACK_ICON: &str = "mail-message-new-symbolic";

/// Badge edge length in pixels.
pub const BADGE_SIZE: usize = 32;

/// Events emitted by the tray.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    /// Left click on the icon or "Show WhatsApp" in the menu.
    Show,
    /// "Quit" in the menu.
    Quit,
}

/// Handle to the running StatusNotifierItem.
pub struct Tray {
    handle: ksni::Handle<WaTray>,
}

impl Tray {
    /// Registers a StatusNotifierItem. Activate (left click) / "Show" → TrayEvent::Show.
    pub async fn spawn(events: UnboundedSender<TrayEvent>) -> anyhow::Result<Tray> {
        let tray = WaTray {
            events,
            unread: 0,
            icon_name: resolve_icon_name(),
        };
        let handle = tray
            .spawn()
            .await
            .map_err(|e| anyhow::anyhow!("failed to register tray icon: {e}"))?;
        Ok(Tray { handle })
    }

    /// Updates tooltip + badge icon; 0 hides the badge.
    pub async fn set_unread(&self, count: u32) {
        if self
            .handle
            .update(|tray: &mut WaTray| tray.unread = count)
            .await
            .is_none()
        {
            tracing::warn!("tray service is no longer running; unread count not shown");
        }
    }
}

struct WaTray {
    events: UnboundedSender<TrayEvent>,
    unread: u32,
    icon_name: String,
}

impl WaTray {
    fn emit(&self, event: TrayEvent) {
        if self.events.send(event).is_err() {
            tracing::debug!(?event, "tray event dropped: receiver is gone");
        }
    }

    fn badge_icons(&self) -> Vec<ksni::Icon> {
        badge_pixmap(self.unread)
            .map(|data| ksni::Icon {
                width: BADGE_SIZE as i32,
                height: BADGE_SIZE as i32,
                data,
            })
            .into_iter()
            .collect()
    }
}

impl ksni::Tray for WaTray {
    fn id(&self) -> String {
        APP_ID.into()
    }

    fn title(&self) -> String {
        "WhatsApp".into()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::Communications
    }

    fn status(&self) -> ksni::Status {
        if self.unread > 0 {
            ksni::Status::NeedsAttention
        } else {
            ksni::Status::Active
        }
    }

    fn icon_name(&self) -> String {
        icon_name_for(self.unread, &self.icon_name)
    }

    // Hosts (cosmic-applet-status-area among them) draw `icon_name` when it
    // is set and ignore overlays/attention icons, so while there is unread
    // mail the name is cleared and the badge pixmap *is* the icon.
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.badge_icons()
    }

    // Shown by hosts that honour NeedsAttention (e.g. KDE).
    fn attention_icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.badge_icons()
    }

    // Drawn on top of the app icon by hosts that support overlays.
    fn overlay_icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.badge_icons()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            icon_name: self.icon_name.clone(),
            icon_pixmap: Vec::new(),
            title: "WhatsApp".into(),
            description: tooltip_text(self.unread),
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.emit(TrayEvent::Show);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem {
                label: "Show WhatsApp".into(),
                activate: Box::new(|this: &mut Self| this.emit(TrayEvent::Show)),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|this: &mut Self| this.emit(TrayEvent::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// The themed icon name to advertise: none while unread messages exist, so
/// hosts fall back to the badge pixmap.
pub(crate) fn icon_name_for(unread: u32, app_icon: &str) -> String {
    if unread > 0 {
        String::new()
    } else {
        app_icon.to_owned()
    }
}

/// "N unread messages" tooltip text.
pub(crate) fn tooltip_text(count: u32) -> String {
    match count {
        0 => "No unread messages".into(),
        1 => "1 unread message".into(),
        n => format!("{n} unread messages"),
    }
}

/// App icon if installed in an icon theme directory, else a stock symbolic icon.
fn resolve_icon_name() -> String {
    let mut bases: Vec<PathBuf> = dirs::data_dir().into_iter().collect();
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    bases.extend(std::env::split_paths(&system));
    if icon_installed_in(&bases, APP_ID) {
        APP_ID.into()
    } else {
        tracing::debug!("app icon not installed; using {FALLBACK_ICON} for the tray");
        FALLBACK_ICON.into()
    }
}

/// True when `name` exists as an app icon under `<base>/icons/hicolor/*/apps`
/// or `<base>/pixmaps` for any of `bases`.
pub(crate) fn icon_installed_in(bases: &[PathBuf], name: &str) -> bool {
    bases.iter().any(|base| {
        if has_icon_file(&base.join("pixmaps"), name) {
            return true;
        }
        let Ok(sizes) = std::fs::read_dir(base.join("icons/hicolor")) else {
            return false;
        };
        sizes
            .flatten()
            .any(|size| has_icon_file(&size.path().join("apps"), name))
    })
}

fn has_icon_file(dir: &Path, name: &str) -> bool {
    ["svg", "png"]
        .iter()
        .any(|ext| dir.join(format!("{name}.{ext}")).is_file())
}

/// Renders `count` as white digits on a red circle into a 32×32 ARGB32
/// (network byte order) buffer. `None` for 0; counts above 99 render "99+".
pub fn badge_pixmap(count: u32) -> Option<Vec<u8>> {
    if count == 0 {
        return None;
    }
    const RED: [u8; 4] = [0xFF, 0xD3, 0x2F, 0x2F]; // ARGB
    const WHITE: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];

    let mut buf = vec![0u8; BADGE_SIZE * BADGE_SIZE * 4];
    let put = |buf: &mut [u8], x: usize, y: usize, color: [u8; 4]| {
        if x < BADGE_SIZE && y < BADGE_SIZE {
            let i = (y * BADGE_SIZE + x) * 4;
            buf[i..i + 4].copy_from_slice(&color);
        }
    };

    // Filled circle centred on the buffer (pixel centres at +0.5).
    let c = BADGE_SIZE as f32 / 2.0;
    for y in 0..BADGE_SIZE {
        for x in 0..BADGE_SIZE {
            let (dx, dy) = (x as f32 + 0.5 - c, y as f32 + 0.5 - c);
            if dx * dx + dy * dy <= c * c {
                put(&mut buf, x, y, RED);
            }
        }
    }

    // Glyphs: 3×5 cells scaled up, one cell of gap between glyphs.
    let glyphs: Vec<[u8; 5]> = badge_text(count).chars().filter_map(glyph).collect();
    let n = glyphs.len();
    let scale = if n <= 2 { 3 } else { 2 };
    let width = n * 3 * scale + n.saturating_sub(1) * scale;
    let height = 5 * scale;
    let x0 = (BADGE_SIZE.saturating_sub(width)) / 2;
    let y0 = (BADGE_SIZE.saturating_sub(height)) / 2;
    for (g, rows) in glyphs.iter().enumerate() {
        let gx = x0 + g * 4 * scale;
        for (row, bits) in rows.iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) == 0 {
                    continue;
                }
                for sy in 0..scale {
                    for sx in 0..scale {
                        put(
                            &mut buf,
                            gx + col * scale + sx,
                            y0 + row * scale + sy,
                            WHITE,
                        );
                    }
                }
            }
        }
    }
    Some(buf)
}

/// Text shown in the badge.
pub(crate) fn badge_text(count: u32) -> String {
    if count > 99 {
        "99+".into()
    } else {
        count.to_string()
    }
}

/// 3×5 bitmap glyph; each row's low three bits are the pixels, MSB = left.
fn glyph(c: char) -> Option<[u8; 5]> {
    Some(match c {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b001, 0b001, 0b001],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        '+' => [0b000, 0b010, 0b111, 0b010, 0b000],
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(buf: &[u8], x: usize, y: usize) -> [u8; 4] {
        let i = (y * BADGE_SIZE + x) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    fn is_red(p: [u8; 4]) -> bool {
        p[0] == 0xFF && p[1] > 0xA0 && p[2] < 0x60 && p[3] < 0x60
    }

    fn is_white(p: [u8; 4]) -> bool {
        p == [0xFF, 0xFF, 0xFF, 0xFF]
    }

    #[test]
    fn zero_has_no_badge() {
        assert!(badge_pixmap(0).is_none());
    }

    #[test]
    fn single_digit_badge() {
        let buf = badge_pixmap(5).expect("badge for 5");
        assert_eq!(buf.len(), BADGE_SIZE * BADGE_SIZE * 4);
        // Inside the circle, around the glyph: red.
        assert!(is_red(pixel(&buf, 16, 4)), "{:?}", pixel(&buf, 16, 4));
        assert!(is_red(pixel(&buf, 4, 16)));
        assert!(is_red(pixel(&buf, 27, 16)));
        // Corners lie outside the circle: fully transparent.
        assert_eq!(pixel(&buf, 0, 0)[0], 0);
        assert_eq!(pixel(&buf, 31, 31)[0], 0);
        // The digit itself is white, at the centre.
        assert!(is_white(pixel(&buf, 16, 16)));
    }

    #[test]
    fn large_counts_render_99_plus() {
        let a = badge_pixmap(100).expect("badge");
        let b = badge_pixmap(12_345).expect("badge");
        assert_eq!(a.len(), BADGE_SIZE * BADGE_SIZE * 4);
        assert_eq!(a, b, "all counts > 99 render the same \"99+\" badge");
        assert_ne!(a, badge_pixmap(99).expect("badge"));
        assert_eq!(badge_text(100), "99+");
        assert_eq!(badge_text(99), "99");
        assert_eq!(badge_text(7), "7");
    }

    #[test]
    fn different_counts_render_differently() {
        assert_ne!(badge_pixmap(1), badge_pixmap(2));
        assert_ne!(badge_pixmap(9), badge_pixmap(10));
    }

    #[test]
    fn unread_replaces_the_themed_icon_with_the_badge() {
        assert_eq!(icon_name_for(0, APP_ID), APP_ID);
        assert_eq!(icon_name_for(3, APP_ID), "");
    }

    #[test]
    fn tooltip_wording() {
        assert_eq!(tooltip_text(0), "No unread messages");
        assert_eq!(tooltip_text(1), "1 unread message");
        assert_eq!(tooltip_text(5), "5 unread messages");
    }

    #[test]
    fn icon_lookup() {
        let tmp = tempfile::tempdir().unwrap();
        let bases = vec![tmp.path().to_path_buf()];
        assert!(!icon_installed_in(&bases, APP_ID));
        let apps = tmp.path().join("icons/hicolor/scalable/apps");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(apps.join(format!("{APP_ID}.svg")), "<svg/>").unwrap();
        assert!(icon_installed_in(&bases, APP_ID));
        assert!(!icon_installed_in(&bases, "something-else"));
    }
}
