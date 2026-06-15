// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.
//! Dashboard runtime patch application (`PUT /api/config`). Split out of
//! `mod.rs` to keep the facade ≤ 200 LOC per the PO plan.

use crate::error::Result;

use super::{Config, ConfigError, RuntimeDashboardConfigPatch};

impl Config {
    /// Merge runtime-editable fields from the dashboard. Returns an error if the patch is empty
    /// or leaves the config invalid.
    pub fn apply_runtime_dashboard_patch(
        &mut self,
        patch: RuntimeDashboardConfigPatch,
    ) -> Result<()> {
        let mut applied = false;

        if let Some(ref g) = patch.general {
            let touched = g.max_concurrent_workflows.is_some()
                || g.max_active_workflows.is_some()
                || g.ticketing_system.is_some();
            if !touched {
                return Err(ConfigError::Validation {
                    section: "general",
                    field: "patch",
                    detail:
                        "must include max_concurrent_workflows, max_active_workflows, and/or ticketing_system"
                            .to_string(),
                }
                .into());
            }
            applied = true;
            if let Some(mc) = g.max_concurrent_workflows {
                self.general.max_concurrent_workflows = mc;
            }
            if let Some(ma) = g.max_active_workflows {
                self.general.max_active_workflows = ma;
            }
            if let Some(ts) = g.ticketing_system {
                self.general.ticketing_system = ts;
            }
        }

        if !applied {
            return Err(ConfigError::Validation {
                section: "runtime",
                field: "patch",
                detail: "empty patch: include \"general\" with at least one field".to_string(),
            }
            .into());
        }

        self.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::general::TicketingSystem;
    use crate::config::web::GeneralConcurrencyPatch;

    #[test]
    fn ticketing_system_alone_is_applied() {
        let mut cfg = Config::default();
        assert_eq!(cfg.general.ticketing_system, TicketingSystem::None);
        let patch = RuntimeDashboardConfigPatch {
            general: Some(GeneralConcurrencyPatch {
                max_concurrent_workflows: None,
                max_active_workflows: None,
                ticketing_system: Some(TicketingSystem::Jira),
            }),
        };
        cfg.apply_runtime_dashboard_patch(patch).expect("apply");
        assert_eq!(cfg.general.ticketing_system, TicketingSystem::Jira);
    }

    #[test]
    fn empty_general_patch_is_rejected() {
        let mut cfg = Config::default();
        let patch = RuntimeDashboardConfigPatch {
            general: Some(GeneralConcurrencyPatch {
                max_concurrent_workflows: None,
                max_active_workflows: None,
                ticketing_system: None,
            }),
        };
        assert!(cfg.apply_runtime_dashboard_patch(patch).is_err());
    }
}
