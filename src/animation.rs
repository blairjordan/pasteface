use std::sync::OnceLock;
use std::time::Instant;

/// A gentle moving highlight, independent of microphone amplitude.
pub fn shimmer(index: usize, length: usize) -> f32 {
    static START: OnceLock<Instant> = OnceLock::new();
    if std::env::var_os("PASTEFACE_REDUCED_MOTION").is_some() {
        return 1.;
    }
    let time = START.get_or_init(Instant::now).elapsed().as_secs_f32();
    let center = (time * 7.) % (length as f32 + 8.) - 4.;
    let distance = (index as f32 - center).abs();
    0.45 + 0.55 * (1. - distance / 3.).clamp(0., 1.)
}

pub fn pulse() -> f32 {
    static START: OnceLock<Instant> = OnceLock::new();
    if std::env::var_os("PASTEFACE_REDUCED_MOTION").is_some() {
        return 1.;
    }
    let time = START.get_or_init(Instant::now).elapsed().as_secs_f32();
    0.65 + 0.35 * (time * 3.).sin()
}
