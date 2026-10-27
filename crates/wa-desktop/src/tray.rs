//! StatusNotifierItem tray icon (ksni) with an unread badge and Show/Quit menu.

use std::path::{Path, PathBuf};

use ksni::TrayMethods as _;
use tokio::sync::mpsc::UnboundedSender;

use crate::APP_ID;

/// Themed tray icon; COSMIC recolours `-symbolic` icons to suit the panel.
const SYMBOLIC_ICON: &str = "com.github.whatsapp-cosmic-symbolic";

/// The white tray logo as raw ARGB32 (network byte order), rendered from
/// `resources/icons/hicolor/symbolic/apps/com.github.whatsapp-cosmic-symbolic.svg`.
const LOGO_32: &[u8] = include_bytes!("../assets/tray-logo-32.argb");
const LOGO_64: &[u8] = include_bytes!("../assets/tray-logo-64.argb");

/// Pixmap edge lengths offered to the host (1× and 2× scale).
pub const PIXMAP_SIZES: [usize; 2] = [32, 64];

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
            symbolic_installed: symbolic_icon_installed(),
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
    symbolic_installed: bool,
}

impl WaTray {
    fn emit(&self, event: TrayEvent) {
        if self.events.send(event).is_err() {
            tracing::debug!(?event, "tray event dropped: receiver is gone");
        }
    }

    fn pixmaps(&self) -> Vec<ksni::Icon> {
        PIXMAP_SIZES
            .iter()
            .map(|&size| ksni::Icon {
                width: size as i32,
                height: size as i32,
                data: tray_pixmap(self.unread, size),
            })
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
        icon_name_for(self.unread, self.symbolic_installed)
    }

    // Hosts (cosmic-applet-status-area among them) draw `icon_name` when it
    // is set and ignore overlays/attention icons, so while there is unread
    // mail the name is cleared and the badged logo pixmap *is* the icon.
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.pixmaps()
    }

    // Shown by hosts that honour NeedsAttention (e.g. KDE).
    fn attention_icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.pixmaps()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            icon_name: String::new(),
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

/// The themed icon name to advertise: none while unread messages exist (or
/// when the icon is not installed), so hosts draw the logo pixmap instead.
pub(crate) fn icon_name_for(unread: u32, symbolic_installed: bool) -> String {
    if unread == 0 && symbolic_installed {
        SYMBOLIC_ICON.to_owned()
    } else {
        String::new()
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

/// Whether the symbolic tray icon is installed in an icon theme directory.
fn symbolic_icon_installed() -> bool {
    let mut bases: Vec<PathBuf> = dirs::data_dir().into_iter().collect();
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    bases.extend(std::env::split_paths(&system));
    let installed = icon_installed_in(&bases, SYMBOLIC_ICON);
    if !installed {
        tracing::debug!("{SYMBOLIC_ICON} not installed; using the embedded tray pixmap");
    }
    installed
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

/// The white logo at `size`×`size` (32 or 64) as ARGB32 in network byte
/// order, with a red unread badge in the bottom-right corner when `count > 0`.
/// Counts above 99 render "99+".
pub fn tray_pixmap(count: u32, size: usize) -> Vec<u8> {
    let mut buf = if size == 64 { LOGO_64 } else { LOGO_32 }.to_vec();
    let size = if size == 64 { 64 } else { 32 };
    if count == 0 {
        return buf;
    }
    const RED: [u8; 4] = [0xFF, 0xD3, 0x2F, 0x2F]; // ARGB
    const WHITE: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];
    const CLEAR: [u8; 4] = [0, 0, 0, 0];
    let put = |buf: &mut [u8], x: usize, y: usize, color: [u8; 4]| {
        if x < size && y < size {
            let i = (y * size + x) * 4;
            buf[i..i + 4].copy_from_slice(&color);
        }
    };

    // Badge circle covering the bottom-right half, with a thin transparent
    // ring cut out of the logo so the badge stays legible on top of it.
    let unit = size / 32;
    let radius = 8.0 * unit as f32;
    let (cx, cy) = (size as f32 - radius, size as f32 - radius);
    let ring = radius + 1.5 * unit as f32;
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let d2 = dx * dx + dy * dy;
            if d2 <= radius * radius {
                put(&mut buf, x, y, RED);
            } else if d2 <= ring * ring {
                put(&mut buf, x, y, CLEAR);
            }
        }
    }

    // Glyphs: 3×5 cells scaled up, one cell of gap between glyphs.
    let glyphs: Vec<[u8; 5]> = badge_text(count).chars().filter_map(glyph).collect();
    let n = glyphs.len();
    let scale = if n <= 2 { 2 * unit } else { unit };
    let width = n * 3 * scale + n.saturating_sub(1) * scale;
    let height = 5 * scale;
    let x0 = (cx as usize).saturating_sub(width / 2);
    let y0 = (cy as usize).saturating_sub(height / 2);
    for (g, rows) in glyphs.iter().enumerate() {
        let gx = x0 + g * 4 * scale;
        for (row, bits) in rows.iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) == 0 {
                    continue;
                }
                for sy in 0..scale {
                    for sx in 0..scale {
                        put(&mut buf, gx + col * scale + sx, y0 + row * scale + sy, WHITE);
                    }
                }
            }
        }
    }
    buf
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

    fn pixel(buf: &[u8], size: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * size + x) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    fn is_red(p: [u8; 4]) -> bool {
        p[0] == 0xFF && p[1] > 0xA0 && p[2] < 0x60 && p[3] < 0x60
    }

    #[test]
    fn embedded_logos_have_the_right_size() {
        assert_eq!(LOGO_32.len(), 32 * 32 * 4);
        assert_eq!(LOGO_64.len(), 64 * 64 * 4);
    }

    #[test]
    fn zero_unread_is_the_plain_logo() {
        assert_eq!(tray_pixmap(0, 32), LOGO_32);
        assert_eq!(tray_pixmap(0, 64), LOGO_64);
    }

    #[test]
    fn badge_sits_bottom_right_and_keeps_the_logo_elsewhere() {
        for size in PIXMAP_SIZES {
            let buf = tray_pixmap(5, size);
            assert_eq!(buf.len(), size * size * 4);
            let u = size / 32;
            // Inside the badge circle, beside the digit: red.
            assert!(is_red(pixel(&buf, size, size - 3 * u, size - 8 * u)));
            // The top-left area, beyond the badge's cut-out ring, is untouched logo.
            for y in 0..size * 3 / 8 {
                for x in 0..size * 3 / 8 {
                    let logo = if size == 64 { LOGO_64 } else { LOGO_32 };
                    assert_eq!(pixel(&buf, size, x, y), pixel(logo, size, x, y));
                }
            }
        }
    }

    #[test]
    fn large_counts_render_99_plus() {
        assert_eq!(tray_pixmap(100, 32), tray_pixmap(12_345, 32));
        assert_ne!(tray_pixmap(100, 32), tray_pixmap(99, 32));
        assert_eq!(badge_text(100), "99+");
        assert_eq!(badge_text(99), "99");
        assert_eq!(badge_text(7), "7");
    }

    #[test]
    fn different_counts_render_differently() {
        assert_ne!(tray_pixmap(1, 32), tray_pixmap(2, 32));
        assert_ne!(tray_pixmap(9, 64), tray_pixmap(10, 64));
    }

    #[test]
    fn themed_icon_only_when_installed_and_nothing_unread() {
        assert_eq!(icon_name_for(0, true), SYMBOLIC_ICON);
        assert_eq!(icon_name_for(3, true), "");
        assert_eq!(icon_name_for(0, false), "");
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
        assert!(!icon_installed_in(&bases, SYMBOLIC_ICON));
        let apps = tmp.path().join("icons/hicolor/symbolic/apps");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(apps.join(format!("{SYMBOLIC_ICON}.svg")), "<svg/>").unwrap();
        assert!(icon_installed_in(&bases, SYMBOLIC_ICON));
        assert!(!icon_installed_in(&bases, "something-else"));
    }
}
