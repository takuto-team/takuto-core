// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Runtime container configuration: editor (browser VS Code), web terminal,
//! and egress network rules.

use serde::{Deserialize, Serialize};

/// Browser-based VS Code editor launched from the dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorConfig {
    /// Application ports to expose in the editor container (e.g. `[3000, 5173]`).
    /// Each port is mapped to a host port from the DinD range (9100–9200).
    #[serde(default)]
    pub ports: Vec<u16>,
    /// Number of extra ports pre-allocated for dynamic forwarding (auto-detected dev servers).
    /// Set to `0` to disable dynamic port forwarding. Default: `10`.
    #[serde(default = "default_dynamic_ports")]
    pub dynamic_ports: usize,
    /// VS Code color theme (e.g. `"One Dark Pro"`, `"GitHub Dark"`).
    #[serde(default)]
    pub theme: String,
    /// Extension marketplace IDs to pre-install (e.g. `["esbenp.prettier-vscode"]`).
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Arbitrary VS Code settings written to `settings.json`.
    /// Keys are VS Code setting paths (e.g. `"editor.fontSize" = 14`).
    #[serde(default)]
    pub settings: std::collections::HashMap<String, toml::Value>,
}

fn default_dynamic_ports() -> usize {
    10
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            ports: Vec::new(),
            dynamic_ports: default_dynamic_ports(),
            theme: String::new(),
            extensions: Vec::new(),
            settings: std::collections::HashMap::new(),
        }
    }
}

/// Web terminal (ttyd) customization launched from the dashboard.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TerminalConfig {
    /// Shell commands run once per editor container lifetime, before the first ttyd session.
    /// Use this for expensive one-time setup (apt installs, tool configuration).
    /// Commands are executed with `/etc/takuto/env` sourced so API tokens are available.
    /// Guarded by `/tmp/.takuto-terminal-setup-done` — won't re-run on the same container.
    #[serde(default)]
    pub setup_commands: Vec<String>,
    /// Shell commands run every time a fresh editor container is created.
    /// Use for tools that should be refreshed on each editor open, e.g.:
    ///   `mise use -g ruby@3.3` — installs on first open, verifies on subsequent opens.
    /// Installs via mise persist in the shared mise volume so only the first run is slow.
    /// `/etc/takuto/env` is sourced before each command.
    #[serde(default)]
    pub startup_commands: Vec<String>,
    /// Default git editor installed and configured inside every editor container.
    /// Set to a package name available via apt (e.g. `"nano"`, `"vim"`, `"micro"`).
    /// When set, the package is installed and `git config --global core.editor` is
    /// updated for the `takuto` user. Empty string (default) leaves git's default.
    #[serde(default)]
    pub git_editor: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkConfig {
    #[serde(default)]
    pub extra_egress_hosts: Vec<String>,
    #[serde(default)]
    pub allow_all_https: bool,
}
