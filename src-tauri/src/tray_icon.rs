// Trovul - Tray icon status badge: a coloured dot drawn over the app icon, so
// the state is visible without opening the window.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Disconnected,
    /// Waiting to reconnect to the last PC.
    Reconnecting,
    Connected,
    /// Sending or receiving audio.
    Streaming,
}

impl TrayState {
    fn color(self) -> Option<[u8; 3]> {
        match self {
            Self::Disconnected => None,
            Self::Reconnecting => Some([0xf5, 0x9e, 0x0b]),
            Self::Connected => Some([0x22, 0xc5, 0x5e]),
            Self::Streaming => Some([0x3b, 0x8b, 0xeb]),
        }
    }
}

/// Copy of `rgba` (width × height, straight alpha) with the state's dot in the
/// bottom-right corner, anti-aliased and ringed in dark for contrast on any
/// taskbar colour.
pub fn with_badge(rgba: &[u8], width: u32, height: u32, state: TrayState) -> Vec<u8> {
    let mut out = rgba.to_vec();
    let Some(color) = state.color() else {
        return out;
    };
    let size = width.min(height) as f32;
    let radius = (size * 0.21).max(3.0);
    let ring = (size * 0.06).max(1.0);
    let cx = width as f32 - radius - ring - 0.5;
    let cy = height as f32 - radius - ring - 0.5;
    const OUTLINE: [u8; 3] = [16, 16, 20];

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            // Coverage of the dot and of dot+ring, with a one-pixel soft edge.
            let dot = (radius + 0.5 - dist).clamp(0.0, 1.0);
            let disc = (radius + ring + 0.5 - dist).clamp(0.0, 1.0);
            if disc <= 0.0 {
                continue;
            }
            let i = ((y * width + x) * 4) as usize;
            let over = |src: [u8; 3], alpha: f32, px: &mut [u8]| {
                let dst_a = px[3] as f32 / 255.0;
                let out_a = alpha + dst_a * (1.0 - alpha);
                for c in 0..3 {
                    let blended = src[c] as f32 * alpha + px[c] as f32 * dst_a * (1.0 - alpha);
                    px[c] = if out_a > 0.0 {
                        (blended / out_a).round() as u8
                    } else {
                        0
                    };
                }
                px[3] = (out_a * 255.0).round() as u8;
            };
            let px = &mut out[i..i + 4];
            over(OUTLINE, disc, px);
            if dot > 0.0 {
                over(color, dot, px);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icon(size: u32) -> Vec<u8> {
        // Opaque grey square with a transparent border, like a typical icon.
        let mut rgba = vec![0u8; (size * size * 4) as usize];
        for y in 2..size - 2 {
            for x in 2..size - 2 {
                let i = ((y * size + x) * 4) as usize;
                rgba[i..i + 4].copy_from_slice(&[90, 90, 90, 255]);
            }
        }
        rgba
    }

    fn pixel(rgba: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    #[test]
    fn badge_colours_the_corner_only() {
        for size in [16, 32, 64] {
            let base = icon(size);
            assert_eq!(with_badge(&base, size, size, TrayState::Disconnected), base);
            let badged = with_badge(&base, size, size, TrayState::Connected);
            // Top-left untouched, dot centre green and opaque.
            assert_eq!(pixel(&badged, size, 4, 4), pixel(&base, size, 4, 4));
            let r = (size as f32 * 0.21).max(3.0);
            let ring = (size as f32 * 0.06).max(1.0);
            let c = (size as f32 - r - ring - 0.5) as u32;
            let [red, green, blue, alpha] = pixel(&badged, size, c, c);
            assert!(
                green > 150 && red < 80 && blue < 140 && alpha == 255,
                "size {size}"
            );
        }
    }

    /// Writes the badge variants over the real app icon to the temp dir, for a
    /// visual check: `cargo test render_badges -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn render_badges_for_review() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("icons/128x128.png");
        let app_icon = image::open(path).unwrap().to_rgba8();
        let size = app_icon.width();
        let base = app_icon.into_raw();
        for (name, state) in [
            ("connected", TrayState::Connected),
            ("streaming", TrayState::Streaming),
            ("reconnecting", TrayState::Reconnecting),
        ] {
            let rgba = with_badge(&base, size, size, state);
            let path = std::env::temp_dir().join(format!("trovul-tray-{name}.png"));
            image::RgbaImage::from_raw(size, size, rgba)
                .unwrap()
                .save(&path)
                .unwrap();
            println!("{}", path.display());
        }
    }
}
