#[cfg(feature = "tray")]
use crate::model::Phase;

/// Hand-set ANSI block art. This is text, never an embedded image.
pub const ANSI_FACE: &[&str] = &[
    "       ▄▄▄▄▄▄▄▄▄▄▄       ",
    "    ▄███████████████▄    ",
    "  ▄███████████████████▄  ",
    "  █████  ███████  █████  ",
    "  ██████████ ██████████  ",
    "  ████████ █ █ ████████  ",
    "   █████████ ███████▀    ",
    "  ▄████▀▀▀▀▀▀▀▀▀▀▀▀      ",
    "  ▀▀                     ",
];

/// Shared mint-to-blue palette for the welcome art and microphone meter.
pub fn gradient(index: usize, count: usize) -> ratatui::style::Color {
    let t = index.min(count.saturating_sub(1)) as f32 / count.saturating_sub(1).max(1) as f32;
    ratatui::style::Color::Rgb(
        (190. - 80. * t) as u8,
        (245. - 70. * t) as u8,
        (209. + 30. * t) as u8,
    )
}

/// Staggered row arrival followed by a single diagonal glint, then the exact static logo.
pub fn reveal(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, elapsed: f32) {
    use ratatui::style::Color;
    let elapsed = if std::env::var_os("PASTEFACE_REDUCED_MOTION").is_some() {
        2.
    } else {
        elapsed
    };
    let width = ANSI_FACE[0].chars().count() as i32;
    let left = area.x as i32 + (area.width as i32 - width) / 2;
    for (row, line) in ANSI_FACE.iter().enumerate() {
        let progress = ((elapsed - row as f32 * 0.035) / 0.65).clamp(0., 1.);
        let ease = 1. - (1. - progress).powi(3);
        let shift = ((1. - ease) * 8.).round() as i32 * if row % 2 == 0 { -1 } else { 1 };
        let Color::Rgb(r, g, b) = gradient(row, ANSI_FACE.len()) else {
            continue;
        };
        for (column, symbol) in line.chars().enumerate().filter(|(_, c)| *c != ' ') {
            let x = left + column as i32 + shift;
            let y = area.y + row as u16;
            if x < area.x as i32 || x >= area.right() as i32 || y >= area.bottom() {
                continue;
            }
            let position = column as f32 * 0.05 + row as f32 * 0.09;
            let glint = if (0.7..2.).contains(&elapsed) {
                (1. - (position - (elapsed - 0.7) * 2.8).abs() / 0.3).max(0.) * 0.55
            } else {
                0.
            };
            let channel = |base: u8, background: f32| {
                (background + ((base as f32 + (255. - base as f32) * glint) - background) * ease)
                    as u8
            };
            frame.buffer_mut()[(x as u16, y)]
                .set_char(symbol)
                .set_fg(Color::Rgb(
                    channel(r, 18.),
                    channel(g, 23.),
                    channel(b, 30.),
                ));
        }
    }
}

/// A small status circle, independent of the terminal logo.
#[cfg(feature = "tray")]
pub fn rgba(size: usize, phase: Phase) -> Vec<u8> {
    let color = match phase {
        Phase::Recording | Phase::Error => [255, 100, 115],
        Phase::Transcribing => [243, 199, 125],
        Phase::Ready => [158, 228, 182],
        Phase::Idle => [146, 158, 174],
    };
    let mut pixels = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let dx = (x as f32 + 0.5) / size as f32 - 0.5;
            let dy = (y as f32 + 0.5) / size as f32 - 0.5;
            let alpha = ((0.34 - (dx * dx + dy * dy).sqrt()) * size as f32).clamp(0., 1.);
            pixels.extend_from_slice(&[color[0], color[1], color[2], (alpha * 255.) as u8]);
        }
    }
    pixels
}
