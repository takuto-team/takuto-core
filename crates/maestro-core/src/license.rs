// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::fmt;
use std::sync::OnceLock;

/// License tiers available for Maestro.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LicenseTier {
    Community,
    Cloud,
    Enterprise,
}

impl fmt::Display for LicenseTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Community => f.write_str("community"),
            Self::Cloud => f.write_str("cloud"),
            Self::Enterprise => f.write_str("enterprise"),
        }
    }
}

impl LicenseTier {
    /// Parse from a string value (case-insensitive).
    pub fn from_str_value(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "community" => Some(Self::Community),
            "cloud" => Some(Self::Cloud),
            "enterprise" => Some(Self::Enterprise),
            _ => None,
        }
    }
}

static CURRENT_TIER: OnceLock<LicenseTier> = OnceLock::new();

/// Initialise the license tier from the `MAESTRO_LICENSE_TIER` environment variable.
///
/// Must be called once at startup. Panics with a clear message if the value is
/// unrecognised, so operators see the problem immediately.
pub fn init_license_tier() {
    let raw = std::env::var("MAESTRO_LICENSE_TIER").unwrap_or_else(|_| "community".to_string());
    let tier = LicenseTier::from_str_value(&raw).unwrap_or_else(|| {
        panic!(
            "Invalid MAESTRO_LICENSE_TIER value: \"{raw}\". \
             Expected one of: community, cloud, enterprise."
        );
    });
    // SAFETY: `OnceLock::set` only fails when the cell is already initialised.
    // `init_license_tier` is called exactly once from `maestro-cli/src/main.rs`
    // during startup before any spawned task. A second call is a programming
    // error and panics deterministically.
    CURRENT_TIER
        .set(tier)
        .expect("init_license_tier must be called exactly once at startup");
    tracing::info!("License tier: {tier}");
}

/// Return the active license tier.
pub fn current_tier() -> LicenseTier {
    *CURRENT_TIER.get().unwrap_or(&LicenseTier::Community)
}

/// Check whether the active tier meets a minimum requirement.
pub fn requires_tier(minimum: LicenseTier) -> bool {
    current_tier() >= minimum
}

/// Assert the active tier meets the minimum for a named feature.
///
/// Returns `Err` with a human-readable message when the tier is too low.
pub fn assert_tier(minimum: LicenseTier, feature: &str) -> Result<(), String> {
    if requires_tier(minimum) {
        Ok(())
    } else {
        Err(format!(
            "\"{feature}\" requires the {minimum} tier. \
             Current tier: {}. Contact morphet.contact@gmail.com for upgrade options.",
            current_tier(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_ordering() {
        assert!(LicenseTier::Community < LicenseTier::Cloud);
        assert!(LicenseTier::Cloud < LicenseTier::Enterprise);
    }

    #[test]
    fn parse_tiers() {
        assert_eq!(
            LicenseTier::from_str_value("community"),
            Some(LicenseTier::Community)
        );
        assert_eq!(
            LicenseTier::from_str_value("CLOUD"),
            Some(LicenseTier::Cloud)
        );
        assert_eq!(
            LicenseTier::from_str_value("Enterprise"),
            Some(LicenseTier::Enterprise)
        );
        assert_eq!(LicenseTier::from_str_value("invalid"), None);
    }
}
