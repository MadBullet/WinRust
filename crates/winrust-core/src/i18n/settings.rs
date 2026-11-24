//! Global appearance/behaviour switches shared through `HKCU\SOFTWARE\WinRust`.
//!
//! Everything here is deliberately opt-in: transparency and blur cost GPU time
//! on every frame, so the default is *off* and the values only change when the
//! user drags a slider.

use winrust_core::registry::{self, Root};

const KEY: &str = r"SOFTWARE\WinRust";

/// Which window chrome style a component should use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Appearance {
    /// Window background opacity, 30..=100 (%). Only used when `transparent`.
    pub opacity: u8,
    /// Blur the desktop behind the window (needs transparency).
    pub blur: bool,
    /// Overall transparency on/off — the master switch that saves GPU time.
    pub transparent: bool,
    /// Blur strength 0..=100, applied through the acrylic system backdrop.
    pub blur_amount: u8,
}

impl Default for Appearance {
    fn default() -> Self {
        // Opaque and unblurred: the cheapest, most compatible configuration.
        Self {
            opacity: 100,
            blur: false,
            transparent: false,
            blur_amount: 40,
        }
    }
}

impl Appearance {
    /// Clamp the numeric knobs into the ranges the DWM accepts.
    pub fn sanitized(mut self) -> Self {
        self.opacity = self.opacity.clamp(30, 100);
        self.blur_amount = self.blur_amount.min(100);
        if !self.transparent {
            self.blur = false;
        }
        self
    }

    /// Effective background opacity in `0.3 ..= 1.0`.
    pub fn alpha(&self) -> f32 {
        if self.transparent {
            self.opacity.clamp(30, 100) as f32 / 100.0
        } else {
            1.0
        }
    }

    pub fn blur_active(&self) -> bool {
        self.transparent && self.blur && self.blur_amount > 0
    }
}

/// Read the appearance block, using the defaults for anything missing.
pub fn load() -> Appearance {
    let d = Appearance::default();
    registry::Key::open(Root::CurrentUser, KEY)
        .ok()
        .map(|k| Appearance {
            opacity: k.u32("Opacity").map(|v| v as u8).unwrap_or(d.opacity),
            blur: k.u32("Blur").map(|v| v != 0).unwrap_or(d.blur),
            transparent: k
                .u32("Transparent")
                .map(|v| v != 0)
                .unwrap_or(d.transparent),
            blur_amount: k.u32("BlurAmount").map(|v| v as u8).unwrap_or(d.blur_amount),
        })
        .unwrap_or(d)
        .sanitized()
}

/// Write the appearance block back (no elevation required).
pub fn save(a: &Appearance) -> winrust_core::Result<()> {
    let a = a.sanitized();
    let key = registry::open_for_write(Root::CurrentUser, KEY)?;
    key.set_u32("Opacity", a.opacity as u32)?;
    key.set_u32("Blur", u32::from(a.blur))?;
    key.set_u32("Transparent", u32::from(a.transparent))?;
    key.set_u32("BlurAmount", a.blur_amount as u32)
}

/// Registry path, exported so components can show where the setting lives.
pub const REGISTRY_PATH: &str = KEY;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_opaque() {
        let a = Appearance::default();
        assert_eq!(a.alpha(), 1.0);
        assert!(!a.blur_active());
    }

    #[test]
    fn blur_requires_transparency() {
        let a = Appearance {
            transparent: false,
            blur: true,
            ..Appearance::default()
        }
        .sanitized();
        assert!(a.blur_active() == false);
    }

    #[test]
    fn alpha_follows_opacity_when_transparent() {
        let a = Appearance {
            transparent: true,
            opacity: 65,
            ..Appearance::default()
        };
        assert!((a.alpha() - 0.65).abs() < f32::EPSILON);
    }

    #[test]
    fn opacity_is_clamped() {
        let a = Appearance {
            transparent: true,
            opacity: 5,
            ..Appearance::default()
        }
        .sanitized();
        assert_eq!(a.opacity, 30);
    }
}
