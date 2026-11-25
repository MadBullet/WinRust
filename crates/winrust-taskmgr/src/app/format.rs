//! The printers the rows and the left strip share: bytes, rates, heat colours,
//! uptime.  One style for the whole window, written once.

use egui::Color32;
use winrust_core::ts;

/// Byte-per-second rate, shaped like Task Manager (B/s, KB/s, MB/s, GB/s).
pub(super) fn rate(bps: f64) -> String {
    if !bps.is_finite() || bps <= 0.0 {
        return "0 Б/с".into();
    }
    let unit_b = ts("B/s");
    if bps < 1024.0 {
        format!("{bps:.0} {unit_b}")
    } else if bps < 1024.0 * 1024.0 {
        format!("{:.1} {}{unit_b}", bps / 1024.0, ts("K"))
    } else if bps < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} {}{unit_b}", bps / (1024.0 * 1024.0), ts("M"))
    } else {
        format!(
            "{:.2} {}{unit_b}",
            bps / (1024.0 * 1024.0 * 1024.0),
            ts("G")
        )
    }
}

pub(super) fn mb(bytes: u64) -> String {
    let m = bytes as f64 / 1_048_576.0;
    if m >= 1024.0 {
        bytes_text(bytes)
    } else {
        format!("{m:.1} {}", ts("MB"))
    }
}

pub(super) fn bytes_text(bytes: u64) -> String {
    let gb = bytes as f64 / 1_073_741_824.0;
    if gb >= 1.0 {
        format!("{gb:.2} {}", ts("GB"))
    } else {
        format!("{:.0} {}", bytes as f64 / 1_048_576.0, ts("MB"))
    }
}

pub(super) fn heat_color(percent: f32, scale: f32) -> Color32 {
    let t = (percent / scale.max(1.0)).clamp(0.0, 1.0);
    if t < 0.005 {
        return Color32::from_rgb(150, 150, 160);
    }
    let r = (120.0 + 135.0 * t) as u8;
    let g = (225.0 - 135.0 * t) as u8;
    Color32::from_rgb(r, g, 110)
}

/// The metric chips are narrow: keep the text short without splitting a char.
pub(super) fn icon_text(s: &str, max: usize) -> String {
    winrust_core::i18n::end(s, max)
}

pub(super) fn format_uptime(ms: u64) -> String {
    let s = ms / 1000;
    let d = s / 86400;
    let h = (s % 86400) / 3600;
    let m = (s % 3600) / 60;
    if d > 0 {
        format!("{}d {}h {}m", d, h, m)
    } else {
        format!("{}h {}m", h, m)
    }
}
