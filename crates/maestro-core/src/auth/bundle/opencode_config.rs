// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! OpenCode self-hosted init-shim (spec
//! `lore/audits/2026-05-27-opencode-self-hosted-spec.md`).
//!
//! OpenCode reads provider definitions from `opencode.json` (project-level)
//! or `~/.config/opencode/opencode.json` (per-user). The CLI has no env-var
//! fallback, so the bundle must materialise a config file inside the worker
//! before the agent step spawns.
//!
//! Maestro's OpenCode role in v1 is self-hosted-only (LM Studio / Ollama /
//! vLLM / private OpenAI-compatible gateways). Claude / Cursor / Codex have
//! native adapters that beat OpenCode against their respective vendor APIs;
//! pointing OpenCode at api.anthropic.com or api.openai.com is supported in
//! the sense that nothing blocks it, but the documented path is one
//! self-hosted endpoint per deployment.
//!
//! The shim emits a single `provider.self_hosted` entry using the Vercel
//! AI-SDK's `@ai-sdk/openai-compatible` adapter (the documented OpenCode
//! recipe — see `04_architecture.md §A.2` and the OpenCode providers docs).
//! The user's optional bearer is embedded in `options.apiKey`. LM Studio
//! requires a non-empty placeholder; we default to the literal string
//! `lm-studio` when the user saved no key.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::config::ConfigError;
use crate::error::Result;

use super::write_secret::write_secret_file;

/// File name produced inside the OpenCode config directory. The bundle
/// bind-mounts the parent dir at `/home/maestro/.config/opencode:ro` in
/// the worker; OpenCode reads `opencode.json` from XDG_CONFIG_HOME.
pub(super) const OPENCODE_CONFIG_FILENAME: &str = "opencode.json";

/// Default `apiKey` value when the user saved no bearer. LM Studio's
/// OpenAI-compat server requires a non-empty placeholder; per its public
/// docs, any string works. Other servers (Ollama with auth disabled,
/// local vLLM, …) ignore the field. See
/// <https://lmstudio.ai/docs/developer/openai-compat>.
pub(super) const DEFAULT_DUMMY_API_KEY: &str = "lm-studio";

/// Provider id we register in `opencode.json`. Always the same single
/// entry, since v1 supports one self-hosted endpoint per deployment. The
/// `-m <provider>/<model>` argv that the OpenCode adapter emits references
/// this id verbatim.
pub(super) const SELF_HOSTED_PROVIDER_ID: &str = "self_hosted";

/// npm adapter the Vercel AI SDK uses for any OpenAI-compatible server.
/// This is the magic value OpenCode dispatches on; renaming it breaks
/// LM Studio / Ollama / vLLM integration in identical ways.
const NPM_ADAPTER: &str = "@ai-sdk/openai-compatible";

/// Top-level shape of the emitted `opencode.json`. Built from a typed
/// struct (not a format string) so a schema bump from OpenCode upstream
/// becomes a `serde_json` typed-error, not a silent renderer failure.
#[derive(Debug, Serialize)]
struct OpenCodeConfig {
    #[serde(rename = "$schema")]
    schema: &'static str,
    provider: Map<String, Value>,
}

/// Write `opencode.json` into `dir` (mode 0400, parent must already be
/// 0700) populating the `self_hosted` provider entry with `base_url`,
/// `model`, and the user's optional `bearer`. Returns the path to the
/// written file.
///
/// `base_url` and `model` MUST be non-empty (the load validator rejects
/// the parent config when either is blank — see `config/load.rs`
/// validator under `provider == OpenCode`). The shim still defends in
/// depth and returns a typed error if either slips through.
///
/// `bearer = None` (or `Some([])`) renders as `apiKey = "lm-studio"` per
/// [`DEFAULT_DUMMY_API_KEY`].
pub(super) fn write_opencode_config(
    dir: &Path,
    base_url: &str,
    model: &str,
    bearer: Option<&[u8]>,
) -> Result<PathBuf> {
    let base_url_trim = base_url.trim();
    if base_url_trim.is_empty() {
        return Err(ConfigError::Operational {
            op: "opencode_config_shim",
            detail: "base_url is empty — validator should have caught this; \
                     spec lore/audits/2026-05-27-opencode-self-hosted-spec.md \
                     §2.4 invariant violated"
                .to_string(),
        }
        .into());
    }
    let model_trim = model.trim();
    if model_trim.is_empty() {
        return Err(ConfigError::Operational {
            op: "opencode_config_shim",
            detail: "model is empty — validator should have caught this; \
                     spec lore/audits/2026-05-27-opencode-self-hosted-spec.md \
                     §2.4 invariant violated"
                .to_string(),
        }
        .into());
    }

    // Resolve apiKey. `Some([])` is treated as "no bearer" — an
    // explicitly-empty saved bearer carries no meaning for any
    // OpenAI-compat server we care about.
    let api_key: String = match bearer {
        Some(bytes) if !bytes.is_empty() => {
            // The bearer is plaintext bytes from the unseal path. Servers
            // expect a UTF-8 string; reject non-UTF-8 with a typed error
            // rather than silently producing invalid JSON.
            std::str::from_utf8(bytes)
                .map_err(|e| ConfigError::Operational {
                    op: "opencode_config_shim",
                    detail: format!("bearer is not valid UTF-8: {e}"),
                })?
                .to_string()
        }
        _ => DEFAULT_DUMMY_API_KEY.to_string(),
    };

    // Build `provider.self_hosted` as a JSON object so the serialized
    // shape matches the OpenCode schema byte-for-byte. The Vercel
    // AI-SDK adapter expects `npm`, `name`, `options{baseURL,apiKey}`
    // and `models{<id>{}}`.
    let mut models = Map::new();
    models.insert(model_trim.to_string(), json!({}));

    let mut provider_entry = Map::new();
    provider_entry.insert("npm".to_string(), Value::String(NPM_ADAPTER.to_string()));
    provider_entry.insert(
        "name".to_string(),
        Value::String("Self-hosted (Maestro)".to_string()),
    );
    provider_entry.insert(
        "options".to_string(),
        json!({
            "baseURL": base_url_trim,
            "apiKey": api_key,
        }),
    );
    provider_entry.insert("models".to_string(), Value::Object(models));

    let mut provider = Map::new();
    provider.insert(
        SELF_HOSTED_PROVIDER_ID.to_string(),
        Value::Object(provider_entry),
    );

    let config = OpenCodeConfig {
        schema: "https://opencode.ai/config.json",
        provider,
    };

    let bytes = serde_json::to_vec_pretty(&config).map_err(|e| ConfigError::Operational {
        op: "opencode_config_shim",
        detail: format!("serialize opencode.json: {e}"),
    })?;

    let path = dir.join(OPENCODE_CONFIG_FILENAME);
    write_secret_file(&path, &bytes)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value as Json;
    use tempfile::TempDir;

    fn read_and_parse(path: &Path) -> Json {
        let bytes = std::fs::read(path).expect("read opencode.json");
        serde_json::from_slice::<Json>(&bytes).expect("opencode.json is valid JSON")
    }

    /// Happy path: admin base_url + model + user bearer → JSON contains
    /// all three in the documented self_hosted provider shape.
    #[test]
    fn writes_config_with_bearer_and_admin_values() {
        let dir = TempDir::new().unwrap();
        let path = write_opencode_config(
            dir.path(),
            "http://lm-studio:1234/v1",
            "lmstudio/qwen3-coder",
            Some(b"user-bearer-token"),
        )
        .expect("write opencode.json");
        assert!(path.ends_with(OPENCODE_CONFIG_FILENAME));

        let v = read_and_parse(&path);
        assert_eq!(v["$schema"], "https://opencode.ai/config.json");

        let provider = &v["provider"][SELF_HOSTED_PROVIDER_ID];
        assert_eq!(provider["npm"], NPM_ADAPTER);
        assert_eq!(provider["options"]["baseURL"], "http://lm-studio:1234/v1");
        assert_eq!(provider["options"]["apiKey"], "user-bearer-token");
        // Model id keys the `models` map.
        assert!(provider["models"]["lmstudio/qwen3-coder"].is_object());
    }

    /// No bearer (None) → apiKey defaults to the LM Studio dummy.
    #[test]
    fn writes_config_with_dummy_api_key_when_bearer_none() {
        let dir = TempDir::new().unwrap();
        let path = write_opencode_config(
            dir.path(),
            "http://lm-studio:1234/v1",
            "lmstudio/qwen3-coder",
            None,
        )
        .unwrap();
        let v = read_and_parse(&path);
        assert_eq!(
            v["provider"][SELF_HOSTED_PROVIDER_ID]["options"]["apiKey"],
            DEFAULT_DUMMY_API_KEY
        );
    }

    /// Empty bearer (`Some([])`) is equivalent to no bearer — LM Studio
    /// behaviour, defended in depth even though `unseal` won't emit
    /// `Some(empty)` in practice.
    #[test]
    fn writes_config_with_dummy_api_key_when_bearer_empty_bytes() {
        let dir = TempDir::new().unwrap();
        let path = write_opencode_config(
            dir.path(),
            "http://lm-studio:1234/v1",
            "lmstudio/qwen3-coder",
            Some(&[]),
        )
        .unwrap();
        let v = read_and_parse(&path);
        assert_eq!(
            v["provider"][SELF_HOSTED_PROVIDER_ID]["options"]["apiKey"],
            DEFAULT_DUMMY_API_KEY
        );
    }

    /// Empty `base_url` → typed error (validator should have caught).
    #[test]
    fn empty_base_url_returns_typed_error() {
        let dir = TempDir::new().unwrap();
        let err = write_opencode_config(dir.path(), "", "lmstudio/qwen3-coder", None)
            .expect_err("empty base_url must error");
        assert!(
            err.to_string().contains("base_url is empty"),
            "error should name the violation; got: {err}"
        );
    }

    /// Whitespace-only `base_url` is treated as empty.
    #[test]
    fn whitespace_base_url_returns_typed_error() {
        let dir = TempDir::new().unwrap();
        let err = write_opencode_config(dir.path(), "   ", "lmstudio/qwen3-coder", None)
            .expect_err("whitespace base_url must error");
        assert!(err.to_string().contains("base_url is empty"));
    }

    /// Empty `model` → typed error.
    #[test]
    fn empty_model_returns_typed_error() {
        let dir = TempDir::new().unwrap();
        let err = write_opencode_config(dir.path(), "http://lm:1234/v1", "", None)
            .expect_err("empty model must error");
        assert!(err.to_string().contains("model is empty"));
    }

    /// Non-UTF-8 bearer → typed error (server can't authenticate with
    /// non-UTF-8 anyway; better to fail loudly).
    #[test]
    fn non_utf8_bearer_returns_typed_error() {
        let dir = TempDir::new().unwrap();
        let bad: &[u8] = &[0xff, 0xfe, 0xfd];
        let err = write_opencode_config(
            dir.path(),
            "http://lm:1234/v1",
            "lmstudio/qwen3-coder",
            Some(bad),
        )
        .expect_err("non-UTF-8 bearer must error");
        assert!(err.to_string().contains("not valid UTF-8"));
    }

    /// File permissions: written via `write_secret_file` which is mode
    /// 0400 on Unix. We re-check that here so a refactor of
    /// `write_secret.rs` doesn't silently widen the perms.
    #[cfg(unix)]
    #[test]
    fn writes_config_with_mode_0400() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = write_opencode_config(
            dir.path(),
            "http://lm-studio:1234/v1",
            "lmstudio/qwen3-coder",
            Some(b"k"),
        )
        .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o400, "opencode.json must be 0400 (owner read-only)");
    }
}
