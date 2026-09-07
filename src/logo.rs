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
