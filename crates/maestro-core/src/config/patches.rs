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

        if let Some(ref w) = patch.web {
            let touched = w.dashboard_username.is_some() || w.dashboard_password.is_some();
            if !touched {
                return Err(ConfigError::Validation {
                    section: "web",
                    field: "patch",
                    detail: "must include dashboard_username and/or dashboard_password".to_string(),
                }
                .into());
            }
            applied = true;
            if let Some(ref u) = w.dashboard_username {
                self.web.dashboard_username = u.clone();
            }
            if let Some(ref p) = w.dashboard_password {
                if p.is_empty()
                    && !self.web.dashboard_username.trim().is_empty()
                    && !self.web.dashboard_password.is_empty()
                {
                    // preserve existing secret when UI omits password
                } else {
                    self.web.dashboard_password = p.clone();
                }
            }
        }

        if let Some(ref g) = patch.general {
            let touched = g.max_concurrent_workflows.is_some() || g.max_active_workflows.is_some();
            if !touched {
                return Err(ConfigError::Validation {
                    section: "general",
                    field: "patch",
                    detail: "must include max_concurrent_workflows and/or max_active_workflows".to_string(),
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
        }

        if !applied {
            return Err(ConfigError::Validation {
                section: "runtime",
                field: "patch",
                detail: "empty patch: include \"web\" and/or \"general\" with at least one field".to_string(),
            }
            .into());
        }

        self.validate()
    }
}
