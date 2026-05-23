// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Cursor on-disk auth heuristics for the boot-time preflight probe.

use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;

use super::process::preflight_home;

/// Cursor CLI stores browser-login state under `CURSOR_CONFIG_DIR` (default `~/.cursor`).
/// `agent status` often returns non-zero without a TTY even when login succeeded, and the JSON schema
/// for tokens changes between releases — so we also accept “this tree clearly has Cursor CLI data”.
pub(super) fn cursor_agent_auth_likely_on_disk() -> bool {
    let config_dir = std::env::var_os("CURSOR_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| preflight_home().join(".cursor"));

    let mut paths = vec![config_dir.join("cli-config.json")];
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        paths.push(x.join("cursor/cli-config.json"));
    } else {
        paths.push(preflight_home().join(".config/cursor/cli-config.json"));
    }

    for p in &paths {
        if json_config_suggests_auth(p) {
            return true;
        }
    }

    // Any other *.json next to cli-config (Cursor versions may rename or split fields)
    if let Ok(rd) = std::fs::read_dir(&config_dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file()
                && p.extension().and_then(|s| s.to_str()) == Some("json")
                && !paths.iter().any(|known| known == &p)
                && json_config_suggests_auth(&p)
            {
                return true;
            }
        }
    }

    // Browser login may store state in nested dirs / non-JSON files; `agent status` is unreliable headless.
    let xdg_cursor = preflight_home().join(".config/Cursor");
    let xdg_cursor_lower = preflight_home().join(".config/cursor");
    cursor_data_tree_looks_populated(&config_dir)
        || cursor_data_tree_looks_populated(&xdg_cursor)
        || cursor_data_tree_looks_populated(&xdg_cursor_lower)
}

/// True if the directory contains a small amount of non-trivial file data typical after `agent login` / CLI use.
fn cursor_data_tree_looks_populated(root: &Path) -> bool {
    if !root.is_dir() {
        return false;
    }

    fn walk(dir: &Path, depth: u8) -> bool {
        if depth > 10 {
            return false;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return false;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let low = name.to_lowercase();
            if low == ".ds_store" || low.contains("readme") {
                continue;
            }
            if p.is_dir() {
                if walk(&p, depth + 1) {
                    return true;
                }
            } else if let Ok(meta) = p.metadata() {
                if !meta.is_file() {
                    continue;
                }
                let len = meta.len();
                if len < 16 {
                    continue;
                }
                if low.ends_with(".log") && len < 256 {
                    continue;
                }
                // SQLite / VS Code style state DBs
                if low.ends_with(".vscdb") || low.ends_with(".db") {
                    return true;
                }
                if low.ends_with(".json") {
                    if let Ok(raw) = std::fs::read_to_string(&p)
                        && let Ok(v) = serde_json::from_str::<JsonValue>(&raw)
                    {
                        if json_value_has_auth_fields(&v) {
                            return true;
                        }
                        if v.as_object().is_some_and(|m| m.len() >= 2 && len >= 32) {
                            return true;
                        }
                    }
                    continue;
                }
                // Any other non-trivial file (e.g. binary token blob)
                if len >= 48 {
                    return true;
                }
            }
        }
        false
    }

    walk(root, 0)
}

fn json_config_suggests_auth(path: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<JsonValue>(&raw) else {
        return false;
    };
    json_value_has_auth_fields(&v)
}

fn json_value_has_auth_fields(v: &JsonValue) -> bool {
    match v {
        JsonValue::Object(map) => {
            // Cursor may store opaque session strings without "token" in the key name.
            for val in map.values() {
                if val.as_str().is_some_and(|s| s.len() >= 64) {
                    return true;
                }
            }
            for (k, val) in map {
                let kl = k.to_lowercase();
                if (kl.contains("token") || kl.ends_with("apikey") || kl == "api_key")
                    && val.as_str().is_some_and(|s| !s.trim().is_empty())
                {
                    return true;
                }
            }
            map.values().any(json_value_has_auth_fields)
        }
        JsonValue::Array(items) => items.iter().any(json_value_has_auth_fields),
        JsonValue::String(s) if s.len() >= 64 => true,
        _ => false,
    }
}

#[cfg(test)]
mod cursor_preflight_tests {
    use std::io::Write;

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn detects_opaque_session_string_in_cli_config() {
        let d = tempdir().unwrap();
        let p = d.path().join("cli-config.json");
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(
            br#"{"session":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
        )
        .unwrap();
        assert!(json_config_suggests_auth(&p));
    }

    #[test]
    fn tree_populated_finds_nested_vscdb() {
        let d = tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("User/globalStorage")).unwrap();
        std::fs::write(d.path().join("User/globalStorage/state.vscdb"), [0u8; 64]).unwrap();
        assert!(cursor_data_tree_looks_populated(d.path()));
    }
}
