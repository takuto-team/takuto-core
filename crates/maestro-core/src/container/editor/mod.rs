// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! openvscode-server editor container lifecycle, plus the URL / token
//! helpers used by the shared-port reverse proxy (`/s/<token>/…`).
//!
//! Split into five files under §7 push-to-A audit (previously a single
//! 1157-LOC `container/editor.rs`):
//! - `mod.rs`               — re-exports + full unit-test suite
//! - `port_alloc.rs`        — 9100–9200 range, in-memory allocator,
//!                            docker-side discovery, restart recovery
//! - `token_gen.rs`         — connection token (UUIDv4) + path token (CSPRNG)
//! - `urls.rs`              — direct + shared-port-proxy URL builders
//! - `labels.rs`            — deterministic container name + label parsing
//! - `container_builder.rs` — `EditorInfo` + `start_editor` / `stop_editor` /
//!                            `get_editor_info` (1 of 3 docker-run-style
//!                            orchestrators in the codebase)

mod container_builder;
mod labels;
mod port_alloc;
mod token_gen;
mod urls;

pub use container_builder::{EditorInfo, get_editor_info, start_editor, stop_editor};
pub use labels::{parse_connection_token_from_labels, parse_label_value};
pub use port_alloc::{allocate_single_port, release_editor_ports};
pub use token_gen::{generate_connection_token, generate_session_path_token};
pub use urls::{
    build_editor_url, build_session_dynamic_port_url, build_session_editor_url,
    build_session_terminal_url, build_terminal_url, session_publish_arg,
};

// `pub(crate)` re-exports so sibling modules (`port_scanner`, `run_command`,
// `terminal`) keep their pre-split `super::editor::*` imports without
// re-routing each call site through the new submodules.
pub(crate) use labels::editor_container_name;
pub(crate) use port_alloc::{
    EDITOR_PORT_MAX, EDITOR_PORT_MIN, allocate_editor_ports, release_container_ports,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_connection_token_is_valid_hex() {
        let token = generate_connection_token();
        assert_eq!(token.len(), 32, "Token must be 32 hex characters");
        assert!(
            token.chars().all(|c| c.is_ascii_hexdigit()),
            "Token must be lowercase hex: {token}"
        );
        assert_eq!(token, token.to_lowercase(), "Token must be lowercase");
    }

    #[test]
    fn generate_connection_token_is_unique() {
        let t1 = generate_connection_token();
        let t2 = generate_connection_token();
        assert_ne!(t1, t2, "Two generated tokens must be different");
    }

    // ---------------------------------------------------------------------
    // GH-45: session path token (CSPRNG, ≥128 bits) and loopback publish arg
    // ---------------------------------------------------------------------

    #[test]
    fn session_path_token_is_32_char_lowercase_hex() {
        let token = generate_session_path_token();
        assert_eq!(
            token.len(),
            32,
            "Token must be 32 hex characters (16 bytes)"
        );
        assert!(
            token.chars().all(|c| c.is_ascii_hexdigit()),
            "Token must be hex: {token}"
        );
        assert_eq!(token, token.to_lowercase(), "Token must be lowercase");
    }

    #[test]
    fn session_path_token_is_unique() {
        // Statistical: with 128 bits of entropy, 1024 generations should
        // produce 1024 distinct values with overwhelming probability.
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1024 {
            let t = generate_session_path_token();
            assert!(
                seen.insert(t),
                "duplicate token in 1024 generations — entropy too low?"
            );
        }
    }

    #[test]
    fn session_path_token_is_not_uuid_v4_shape() {
        // UUID v4 simple has fixed bits at positions 12 (always '4') and 16
        // (always one of '8','9','a','b'). A pure 16-byte random token must
        // not impose those constraints. Verify across many samples that we
        // observe values outside the UUID v4 alphabet at those positions.
        let mut pos12 = std::collections::HashSet::new();
        let mut pos16 = std::collections::HashSet::new();
        for _ in 0..512 {
            let t = generate_session_path_token();
            pos12.insert(t.as_bytes()[12]);
            pos16.insert(t.as_bytes()[16]);
        }
        // We expect each set to contain more than 1 distinct hex digit at
        // those positions — UUID v4 would lock pos12 to b'4' and pos16 to
        // {b'8',b'9',b'a',b'b'}.
        assert!(
            pos12.len() > 1,
            "position 12 was constant — looks UUID-shaped"
        );
        assert!(
            !(pos12 == [b'4'].into_iter().collect()),
            "position 12 locked to '4'"
        );
        assert!(
            pos16.len() > 4,
            "position 16 alphabet too narrow — looks UUID-shaped"
        );
    }

    #[test]
    fn build_session_editor_url_uses_relative_proxy_path() {
        let url = build_session_editor_url(
            "0123456789abcdef0123456789abcdef",
            "deadbeefdeadbeefdeadbeefdeadbeef",
            "/workspace/proj",
        );
        assert_eq!(
            url,
            "/s/0123456789abcdef0123456789abcdef/?tkn=deadbeefdeadbeefdeadbeefdeadbeef&folder=/workspace/proj"
        );
    }

    #[test]
    fn build_session_editor_url_encodes_special_chars_in_folder() {
        let url = build_session_editor_url("tok", "conn", "/workspace/my project&foo#bar");
        assert_eq!(
            url,
            "/s/tok/?tkn=conn&folder=/workspace/my%20project%26foo%23bar"
        );
    }

    #[test]
    fn encode_query_value_preserves_slashes_and_alphanumerics() {
        assert_eq!(
            urls::encode_query_value("/workspace/proj-name"),
            "/workspace/proj-name"
        );
    }

    #[test]
    fn encode_query_value_encodes_query_unsafe_chars() {
        assert_eq!(
            urls::encode_query_value("a&b=c#d+e f%g"),
            "a%26b%3dc%23d%2be%20f%25g"
        );
    }

    #[test]
    fn build_session_terminal_url_uses_relative_proxy_path() {
        let url = build_session_terminal_url(
            "0123456789abcdef0123456789abcdef",
            "deadbeefdeadbeefdeadbeefdeadbeef",
        );
        assert_eq!(
            url,
            "/s/0123456789abcdef0123456789abcdef/deadbeefdeadbeefdeadbeefdeadbeef/"
        );
    }

    #[test]
    fn session_publish_arg_format_matches_env() {
        let arg = session_publish_arg(9101, 9101);
        if std::env::var("DOCKER_HOST").is_ok() {
            // DinD mode: no loopback prefix, just host:container.
            assert_eq!(arg, "9101:9101");
            assert_eq!(session_publish_arg(9201, 9101), "9201:9101");
        } else {
            // Local Docker: loopback-only binding.
            assert_eq!(arg, "127.0.0.1:9101:9101");
            assert_eq!(session_publish_arg(9201, 9101), "127.0.0.1:9201:9101");
        }
    }

    #[test]
    fn build_editor_url_includes_tkn_param() {
        let url = build_editor_url(9100, "abcdef0123456789abcdef0123456789", "/workspace/proj");
        assert_eq!(
            url,
            "http://localhost:9100/?tkn=abcdef0123456789abcdef0123456789&folder=/workspace/proj"
        );
    }

    #[test]
    fn parse_connection_token_from_labels_present() {
        let json = r#"{"maestro.connection_token":"abcdef0123456789abcdef0123456789","other":"x"}"#;
        assert_eq!(
            parse_connection_token_from_labels(json),
            Some("abcdef0123456789abcdef0123456789".to_string())
        );
    }

    #[test]
    fn parse_connection_token_from_labels_missing() {
        let json = r#"{"other.label":"x"}"#;
        assert_eq!(parse_connection_token_from_labels(json), None);
    }

    #[test]
    fn parse_connection_token_from_labels_empty_value() {
        let json = r#"{"maestro.connection_token":""}"#;
        assert_eq!(parse_connection_token_from_labels(json), None);
    }

    #[test]
    fn parse_connection_token_from_labels_invalid_json() {
        assert_eq!(parse_connection_token_from_labels("not json"), None);
        assert_eq!(parse_connection_token_from_labels(""), None);
    }

    #[test]
    fn editor_info_serializes_connection_token() {
        let info = EditorInfo {
            url: "http://localhost:9100/?tkn=abc&folder=/w".to_string(),
            connection_token: "abc".to_string(),
            vscode_port: 9100,
            port_mappings: vec![],
            spare_ports: vec![],
            folder: "/w".to_string(),
            path_token: "deadbeef".to_string(),
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["connection_token"], "abc");
        assert_eq!(json["url"], "http://localhost:9100/?tkn=abc&folder=/w");
    }

    #[test]
    fn build_terminal_url_includes_token_in_path() {
        let url = build_terminal_url(9150, "abcdef0123456789abcdef0123456789");
        assert_eq!(
            url,
            "http://localhost:9150/abcdef0123456789abcdef0123456789/"
        );
    }

    #[test]
    fn build_terminal_url_trailing_slash() {
        let url = build_terminal_url(9100, "aabb");
        assert!(url.ends_with('/'), "Terminal URL must end with /: {url}");
        // The token is immediately before the trailing slash.
        assert!(
            url.ends_with("aabb/"),
            "Token must be immediately before trailing slash: {url}"
        );
    }
}
