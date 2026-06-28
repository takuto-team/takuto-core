// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Runtime installation of the agent + Atlassian CLIs.
//!
//! These CLIs are **not** baked into the image (we have no right to
//! redistribute claude-code / cursor-agent, and codex/opencode/acli follow the
//! same path for consistency). Instead they are installed on container startup
//! into the shared, persistent `takuto-tools` volume (`/opt/takuto-tools/bin`,
//! which is on the worker + workspace `PATH`), so every container resolves them
//! by bare name. Each client's version may be pinned in `config.toml`
//! (`[agent.providers.*].version`, `[jira].acli_version`); empty = latest.
//!
//! Design: a pure [`plan_one`] decides per-client what to do given the detected
//! installed version; an [`Installer`] executes, reporting progress through a
//! [`ProgressSink`] so the same logic serves both the CLI (stdout, used in setup
//! mode) and the web server (status + WebSocket).

use std::path::PathBuf;

use tokio_util::sync::CancellationToken;

use crate::config::Config;

/// How a client binary is obtained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    /// npm global package; npm verifies integrity. `package` is the npm name.
    Npm { package: String },
    /// Cursor agent — official HTTPS download (no build-time sha; TLS-verified).
    Cursor,
    /// Atlassian CLI — direct HTTPS binary from acli.atlassian.com (cross-arch).
    Acli,
}

/// One installable CLI.
#[derive(Debug, Clone)]
pub struct ClientSpec {
    /// Human label shown as the install step.
    pub name: String,
    /// Binary file name placed in `<dir>/bin` and resolved on `PATH`.
    pub bin: String,
    pub kind: InstallKind,
    /// Pinned version; empty = latest.
    pub version: String,
}

/// The target version for an install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionTarget {
    Latest,
    Pinned(String),
}

/// What to do for a client after consulting the installed version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Pinned version already installed — nothing to do.
    Skip,
    Install(VersionTarget),
}

impl VersionTarget {
    /// Label fragment for progress messages.
    pub fn label(&self) -> String {
        match self {
            VersionTarget::Latest => "latest".to_string(),
            VersionTarget::Pinned(v) => v.clone(),
        }
    }
}

/// Decide the action for one client given its currently-installed version.
///
/// - unpinned → always (re)install **latest** (per the confirmed product
///   decision: unpinned refreshes to latest on each startup);
/// - pinned and the detected version matches → **Skip**;
/// - pinned and missing/mismatched → install the **pinned** version.
pub fn plan_one(spec: &ClientSpec, detected: Option<&str>) -> Action {
    if spec.version.is_empty() {
        Action::Install(VersionTarget::Latest)
    } else if detected == Some(spec.version.as_str()) {
        Action::Skip
    } else {
        Action::Install(VersionTarget::Pinned(spec.version.clone()))
    }
}

/// Extract a version token from a CLI's `--version` output. Returns the first
/// whitespace-separated token that starts with a digit (trimmed of surrounding
/// punctuation), e.g. `"2.1.178 (Claude Code)"` → `"2.1.178"`,
/// `"codex-cli 0.140.0"` → `"0.140.0"`.
pub fn parse_version(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .find(|tok| tok.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(|tok| {
            tok.trim_matches(|c: char| !c.is_ascii_alphanumeric())
                .to_string()
        })
        .filter(|s| !s.is_empty())
}

/// `package@version` (or `package@latest`) for `npm install -g`.
pub fn npm_pkg_spec(package: &str, target: &VersionTarget) -> String {
    match target {
        VersionTarget::Latest => format!("{package}@latest"),
        VersionTarget::Pinned(v) => format!("{package}@{v}"),
    }
}

/// Cursor agent tarball URL for a concrete version + arch (`x64` / `arm64`).
pub fn cursor_tarball_url(version: &str, arch: &str) -> String {
    format!("https://downloads.cursor.com/lab/{version}/linux/{arch}/agent-cli-package.tar.gz")
}

/// The set of clients to install, derived from config:
/// the agents listed in `[agent].available_providers` (defaults to all four),
/// plus `acli` **always** (the ticketing system can be switched at runtime
/// without restarting, so acli must already be present if the user moves to
/// Jira).
pub fn specs_from_config(cfg: &Config) -> Vec<ClientSpec> {
    let mut specs = Vec::new();
    let p = &cfg.agent.providers;
    let want = |name: &str| cfg.agent.available_providers.iter().any(|x| x == name);

    if want("claude") {
        specs.push(ClientSpec {
            name: "Claude Code".into(),
            bin: "claude".into(),
            kind: InstallKind::Npm {
                package: "@anthropic-ai/claude-code".into(),
            },
            version: p.claude.version.clone(),
        });
    }
    if want("codex") {
        specs.push(ClientSpec {
            name: "Codex".into(),
            bin: "codex".into(),
            kind: InstallKind::Npm {
                package: "@openai/codex".into(),
            },
            version: p.codex.version.clone(),
        });
    }
    if want("opencode") {
        specs.push(ClientSpec {
            name: "OpenCode".into(),
            bin: "opencode".into(),
            kind: InstallKind::Npm {
                package: "opencode-ai".into(),
            },
            version: p.opencode.version.clone(),
        });
    }
    if want("cursor") {
        specs.push(ClientSpec {
            name: "Cursor Agent".into(),
            bin: "agent".into(),
            kind: InstallKind::Cursor,
            version: p.cursor.version.clone(),
        });
    }
    specs.push(ClientSpec {
        name: "Atlassian CLI".into(),
        bin: "acli".into(),
        kind: InstallKind::Acli,
        version: cfg.jira.acli_version.clone(),
    });
    specs
}

/// Receives install progress so the same executor can drive a CLI (stdout) and
/// the web server (status struct + WebSocket).
pub trait ProgressSink: Send + Sync {
    /// Starting step `index` (0-based) of `total`, with a human `label`.
    fn step(&self, index: usize, total: usize, label: &str);
    /// All steps finished successfully.
    fn finished(&self);
    /// Step `label` failed with `error` (install aborts).
    fn failed(&self, label: &str, error: &str);
}

/// Executes installs into `install_dir` (e.g. `/opt/takuto-tools`). Binaries
/// land in `<install_dir>/bin`.
pub struct Installer {
    install_dir: PathBuf,
}

impl Installer {
    pub fn new(install_dir: impl Into<PathBuf>) -> Self {
        Self {
            install_dir: install_dir.into(),
        }
    }

    fn bin_dir(&self) -> PathBuf {
        self.install_dir.join("bin")
    }

    fn bin_path(&self, bin: &str) -> PathBuf {
        self.bin_dir().join(bin)
    }

    /// Detect the installed version of `spec`'s binary, if present.
    async fn detect_version(&self, spec: &ClientSpec) -> Option<String> {
        let path = self.bin_path(&spec.bin);
        if !path.exists() {
            return None;
        }
        let out = crate::process::run_command(
            &path.to_string_lossy(),
            &["--version"],
            &self.install_dir,
            CancellationToken::new(),
        )
        .await
        .ok()?;
        if !out.success() {
            return None;
        }
        parse_version(&out.stdout)
    }

    /// Plan + run every install for `cfg`, reporting through `sink`. Returns the
    /// first error (and reports it to the sink) so callers can mark a failure.
    pub async fn install_all(&self, cfg: &Config, sink: &dyn ProgressSink) -> Result<(), String> {
        let specs = specs_from_config(cfg);
        let total = specs.len();
        for (i, spec) in specs.iter().enumerate() {
            let detected = self.detect_version(spec).await;
            let action = plan_one(spec, detected.as_deref());
            let label = match &action {
                Action::Skip => format!("{} (already installed)", spec.name),
                Action::Install(t) => format!("{} ({})", spec.name, t.label()),
            };
            sink.step(i, total, &label);
            let target = match action {
                Action::Skip => continue,
                Action::Install(t) => t,
            };
            if let Err(e) = self.install_one(spec, &target).await {
                sink.failed(&spec.name, &e);
                return Err(format!("{}: {e}", spec.name));
            }
        }
        sink.finished();
        Ok(())
    }

    async fn install_one(&self, spec: &ClientSpec, target: &VersionTarget) -> Result<(), String> {
        match &spec.kind {
            InstallKind::Npm { package } => self.npm_install(package, target).await,
            InstallKind::Cursor => self.cursor_install(target).await,
            InstallKind::Acli => self.acli_install(target).await,
        }
    }

    async fn run_shell(&self, script: &str) -> Result<(), String> {
        let out = crate::process::run_command(
            "bash",
            &["-c", script],
            &self.install_dir,
            CancellationToken::new(),
        )
        .await
        .map_err(|e| e.to_string())?;
        if out.success() {
            Ok(())
        } else {
            // Surface stdout when stderr is empty: some installers (e.g. piped
            // `curl … | bash`) emit their diagnostics on stdout, and a failure
            // with empty stderr otherwise reaches the UI as a blank error.
            let stderr = out.stderr.trim();
            let detail = if stderr.is_empty() {
                out.stdout.trim()
            } else {
                stderr
            };
            Err(detail.to_string())
        }
    }

    async fn npm_install(&self, package: &str, target: &VersionTarget) -> Result<(), String> {
        let spec = npm_pkg_spec(package, target);
        let prefix = self.install_dir.to_string_lossy();
        // --prefix lands the binary in <prefix>/bin; npm verifies integrity.
        self.run_shell(&format!(
            "npm install -g --no-fund --no-audit --prefix {} {}",
            shell_quote(&prefix),
            shell_quote(&spec),
        ))
        .await
    }

    async fn cursor_install(&self, target: &VersionTarget) -> Result<(), String> {
        let bin_dir = self.bin_dir();
        let share = self.install_dir.join("share").join("cursor-agent");
        // Cursor ships no stable "latest" download URL: its official installer
        // (`cursor.com/install`) hardcodes the current version inside a
        // `downloads.cursor.com/lab/<version>/…` URL. For the unpinned case we
        // parse the version out of THAT URL — the installer's own source of
        // truth, resilient to its version-string format changing — then download
        // the concrete versioned tarball ourselves (silently, `-fsSL`). We do
        // NOT pipe the installer into `bash`: its progress meter spews thousands
        // of lines with no TTY and floods the install output. We extract the
        // tree and symlink the launcher (its realpath lookup needs index.js
        // beside the script).
        let pinned = match target {
            VersionTarget::Pinned(v) => v.clone(),
            VersionTarget::Latest => String::new(),
        };
        let script = format!(
            r#"set -euo pipefail
arch="$(dpkg --print-architecture)"
case "$arch" in
  amd64) carch=x64 ;;
  arm64) carch=arm64 ;;
  *) echo "unsupported arch: $arch" >&2; exit 1 ;;
esac
version={pinned}
if [ -z "$version" ]; then
  version="$(curl -fsSL https://cursor.com/install \
    | grep -oE 'downloads\.cursor\.com/lab/[^/"]+/' | head -n1 \
    | sed -E 's#.*/lab/([^/]+)/#\1#')"
  [ -n "$version" ] || {{ echo "could not resolve latest cursor version from cursor.com/install" >&2; exit 1; }}
fi
url="https://downloads.cursor.com/lab/$version/linux/$carch/agent-cli-package.tar.gz"
dest={share}/$version
mkdir -p "$dest" {bin}
curl -fsSL --retry 3 --retry-delay 5 "$url" -o /tmp/cursor-agent.tar.gz
tar --strip-components=1 -xzf /tmp/cursor-agent.tar.gz -C "$dest"
rm -f /tmp/cursor-agent.tar.gz
ln -sf "$dest/cursor-agent" {bin}/agent
ln -sf "$dest/cursor-agent" {bin}/cursor-agent
test -f "$dest/index.js"
"#,
            pinned = shell_quote(&pinned),
            share = shell_quote(&share.to_string_lossy()),
            bin = shell_quote(&bin_dir.to_string_lossy()),
        );
        self.run_shell(&script).await
    }

    async fn acli_install(&self, target: &VersionTarget) -> Result<(), String> {
        let bin_dir = self.bin_dir();
        // Atlassian publishes acli as a direct, cross-arch binary
        // (acli_linux_amd64 / acli_linux_arm64) under `.../linux/latest/...`.
        // Only `latest` is served (versioned URLs 403), so a pin can't be
        // honoured — warn and install latest. `dpkg --print-architecture`
        // already yields `amd64`/`arm64`, matching acli's naming.
        if let VersionTarget::Pinned(v) = target {
            tracing::warn!(
                version = %v,
                "acli has no versioned download (only `latest`); installing latest"
            );
        }
        let script = format!(
            r#"set -euo pipefail
arch="$(dpkg --print-architecture)"
url="https://acli.atlassian.com/linux/latest/acli_linux_$arch/acli"
mkdir -p {bin}
curl -fSL --retry 3 --retry-delay 5 "$url" -o {bin}/acli
chmod +x {bin}/acli
{bin}/acli --version >/dev/null
"#,
            bin = shell_quote(&bin_dir.to_string_lossy()),
        );
        self.run_shell(&script).await
    }
}

/// Minimal single-quote shell escaping for interpolated paths/URLs.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// A [`ProgressSink`] that prints each step to stdout (used by the CLI / setup
/// mode, where there is no web server to report to).
pub struct StdoutSink;

impl ProgressSink for StdoutSink {
    fn step(&self, index: usize, total: usize, label: &str) {
        println!("[{}/{}] {}", index + 1, total, label);
    }
    fn finished(&self) {
        println!("Dependencies ready.");
    }
    fn failed(&self, label: &str, error: &str) {
        eprintln!("Install failed for {label}: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(version: &str) -> ClientSpec {
        ClientSpec {
            name: "Claude Code".into(),
            bin: "claude".into(),
            kind: InstallKind::Npm {
                package: "@anthropic-ai/claude-code".into(),
            },
            version: version.into(),
        }
    }

    #[test]
    fn unpinned_always_installs_latest() {
        assert_eq!(
            plan_one(&spec(""), Some("2.1.178")),
            Action::Install(VersionTarget::Latest)
        );
        assert_eq!(
            plan_one(&spec(""), None),
            Action::Install(VersionTarget::Latest)
        );
    }

    #[test]
    fn pinned_matching_detected_skips() {
        assert_eq!(plan_one(&spec("2.1.178"), Some("2.1.178")), Action::Skip);
    }

    #[test]
    fn pinned_mismatch_or_absent_installs_pinned() {
        assert_eq!(
            plan_one(&spec("2.1.178"), Some("2.1.0")),
            Action::Install(VersionTarget::Pinned("2.1.178".into()))
        );
        assert_eq!(
            plan_one(&spec("2.1.178"), None),
            Action::Install(VersionTarget::Pinned("2.1.178".into()))
        );
    }

    #[test]
    fn parse_version_handles_common_shapes() {
        assert_eq!(
            parse_version("2.1.178 (Claude Code)").as_deref(),
            Some("2.1.178")
        );
        assert_eq!(
            parse_version("codex-cli 0.140.0").as_deref(),
            Some("0.140.0")
        );
        assert_eq!(parse_version("1.17.7\n").as_deref(), Some("1.17.7"));
        assert_eq!(parse_version("no version here"), None);
    }

    #[test]
    fn npm_pkg_spec_formats() {
        assert_eq!(
            npm_pkg_spec("opencode-ai", &VersionTarget::Latest),
            "opencode-ai@latest"
        );
        assert_eq!(
            npm_pkg_spec("@openai/codex", &VersionTarget::Pinned("0.140.0".into())),
            "@openai/codex@0.140.0"
        );
    }

    #[test]
    fn cursor_url_builds_per_arch() {
        assert_eq!(
            cursor_tarball_url("1.2.3", "arm64"),
            "https://downloads.cursor.com/lab/1.2.3/linux/arm64/agent-cli-package.tar.gz"
        );
    }

    #[test]
    fn specs_include_all_agents_and_always_acli() {
        use crate::config::TicketingSystem;
        // acli installs regardless of ticketing system — it can be switched at
        // runtime without restarting, so it must already be present.
        for ts in [
            TicketingSystem::None,
            TicketingSystem::Jira,
            TicketingSystem::GitHub,
        ] {
            let mut cfg = Config::default();
            cfg.general.ticketing_system = ts;
            let names: Vec<_> = specs_from_config(&cfg)
                .iter()
                .map(|s| s.bin.clone())
                .collect();
            assert!(names.contains(&"claude".to_string()));
            assert!(names.contains(&"agent".to_string()));
            assert!(names.contains(&"acli".to_string()), "acli always installs");
        }
    }

    #[test]
    fn specs_respect_available_providers() {
        let mut cfg = Config::default();
        cfg.agent.available_providers = vec!["claude".into()];
        let names: Vec<_> = specs_from_config(&cfg)
            .iter()
            .map(|s| s.bin.clone())
            .collect();
        // The agent set follows available_providers; acli is always appended.
        assert_eq!(names, vec!["claude".to_string(), "acli".to_string()]);
    }
}
