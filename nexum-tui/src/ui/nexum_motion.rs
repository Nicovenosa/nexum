//! Nexum motion policy.
//!
//! Centralizes environment-driven motion defaults so visual effects don't drift
//! across welcome, mascot, logo, and background layers.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionMode {
    Off,
    Subtle,
    Full,
}

impl MotionMode {
    pub fn from_env() -> Self {
        match std::env::var("NEXUM_MOTION") {
            Ok(v) if v.eq_ignore_ascii_case("off") => Self::Off,
            Ok(v) if v.eq_ignore_ascii_case("full") => Self::Full,
            Ok(v) if v.eq_ignore_ascii_case("subtle") => Self::Subtle,
            _ => Self::Subtle,
        }
    }

    pub fn is_enabled(self) -> bool {
        self != Self::Off
    }

    pub fn carousel_interval_ticks(self) -> u64 {
        if let Ok(ms) = std::env::var("NEXUM_CAROUSEL_INTERVAL_MS") {
            if let Ok(ms) = ms.parse::<u64>() {
                return (ms / 100).max(1);
            }
        }
        match self {
            Self::Off => u64::MAX,
            Self::Subtle => 80, // ~8s at 10fps
            Self::Full => 50,   // ~5s at 10fps
        }
    }

    pub fn shine_interval_ticks(self) -> u64 {
        match self {
            Self::Off => u64::MAX,
            Self::Subtle => 300, // ~30s at 10fps
            Self::Full => 220,   // ~22s at 10fps
        }
    }

    pub fn shine_duration_ticks(self) -> u64 {
        match self {
            Self::Off => 0,
            Self::Subtle => 18, // ~1.8s at 10fps
            Self::Full => 22,   // ~2.2s at 10fps
        }
    }

    pub fn meteor_interval_ticks(self) -> u64 {
        match self {
            Self::Off => u64::MAX,
            Self::Subtle => 150, // ~15s at 10fps
            Self::Full => 100,   // ~10s at 10fps
        }
    }

    pub fn blink_interval_ticks(self) -> u64 {
        match self {
            Self::Off => 90,    // very slow, almost static
            Self::Subtle => 55, // ~5.5s at 10fps
            Self::Full => 45,   // ~4.5s at 10fps
        }
    }
}

pub fn logo_shine_enabled(mode: MotionMode) -> bool {
    mode.is_enabled()
        && std::env::var("NEXUM_LOGO_SHINE")
            .map(|v| !v.eq_ignore_ascii_case("off") && v != "0")
            .unwrap_or(true)
}

pub fn meteors_enabled(mode: MotionMode) -> bool {
    mode.is_enabled()
        && std::env::var("NEXUM_METEORS")
            .map(|v| !v.eq_ignore_ascii_case("off") && v != "0")
            .unwrap_or(true)
}
