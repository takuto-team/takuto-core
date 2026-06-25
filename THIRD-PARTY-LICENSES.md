# Third-party software in the Takuto Core image

The Takuto Core container image bundles the third-party software listed below, each under
its own license. Takuto Core's own source is licensed separately under the **FSL (FSL-1.1-ALv2)** (see
[`LICENSE`](LICENSE)); including these tools in the image is *mere aggregation* — they
remain governed by the licenses below, and their licenses do not affect Takuto Core's.

Full, per-package license texts for the Debian base system are shipped **inside the image**
at `/usr/share/doc/*/copyright`.

| Component | Purpose in the image | License | Project |
|---|---|---|---|
| Debian (bookworm) base | Base OS + core utilities (bash, coreutils, …) | GPL-2.0 / LGPL / BSD / MIT and others, per package | https://www.debian.org |
| Node.js | JavaScript runtime | MIT (bundled components under their own licenses) | https://github.com/nodejs/node |
| npm | Node package manager (ships with Node.js) | Artistic-2.0 | https://github.com/npm/cli |
| Git | Version control / worktrees | GPL-2.0 | https://git-scm.com |
| GitHub CLI (`gh`) | GitHub operations (push, PRs) | MIT | https://github.com/cli/cli |
| Docker CLI (`docker.io`) | Container orchestration client | Apache-2.0 | https://github.com/docker/cli |
| mise | Runtime / toolchain version manager | MIT | https://github.com/jdx/mise |
| ttyd | Web terminal (dashboard "Open terminal") | MIT | https://github.com/tsl0922/ttyd |
| openvscode-server | Browser VS Code editor (Code-OSS build) | MIT | https://github.com/gitpod-io/openvscode-server |
| socat | Dynamic port forwarding | GPL-2.0 | http://www.dest-unreach.org/socat/ |
| iptables | Egress allowlist (firewall rules) | GPL-2.0 | https://www.netfilter.org/projects/iptables/ |
| curl | HTTP client | curl license (MIT/X-style) | https://curl.se |
| ca-certificates | CA trust bundle | MPL-2.0 / public domain | https://www.mozilla.org |

## Source for copyleft components

For the **GPL-2.0 / LGPL** components (Debian base utilities, Git, socat, iptables…), the
corresponding source is available from the upstream projects above and from the Debian
source archive (https://snapshot.debian.org). These packages are bundled **unmodified**.

## Source-code dependencies (compiled into Takuto Core)

The image also ships **Takuto Core's own binary and dashboard**, which statically include
third-party libraries you also redistribute:

- **Rust crates** — linked into the `takuto` binary (`Cargo.toml` / `Cargo.lock`).
- **JavaScript/TypeScript packages** — bundled into the dashboard (`ui/package.json`).

These are too many and too version-specific to hand-list, so they are covered by
**generated** reports rather than this file:

- **[`THIRD-PARTY-RUST.md`](THIRD-PARTY-RUST.md)** — Rust crates + full license texts.
- **[`THIRD-PARTY-NPM.txt`](THIRD-PARTY-NPM.txt)** — dashboard JS/TS deps + full license texts.

Regenerate them per release:

```bash
# Rust crates → attribution file
cargo install cargo-about
cargo about generate about.hbs > THIRD-PARTY-RUST.md

# Enforce a license allowlist (catches GPL/unknown/incompatible crates)
cargo install cargo-deny
cargo deny check licenses

# Dashboard JS/TS dependencies
cd ui && npx license-checker-rseidelsohn --production --out ../THIRD-PARTY-NPM.txt
```

The vast majority of these dependencies are permissive (**MIT / Apache-2.0 / BSD / ISC**),
which only requires preserving their notices — handled by the generated reports above.
Because Takuto Core ships these dependencies in its image, the dependency tree must stay permissively licensed;
`cargo deny` (with a `deny.toml` license allowlist) is the gate that flags any
GPL-2.0-only, proprietary, or unlicensed crate before release.

## Regenerating this inventory

This list covers what is **redistributed in the image**. To produce a complete, verifiable
software bill of materials from a built image:

```bash
# Full SBOM with licenses (recommended)
syft <image-ref>

# Or harvest the Debian package copyrights directly
docker run --rm --entrypoint sh <image-ref> -c \
  'for f in /usr/share/doc/*/copyright; do echo "== $f =="; head -n 5 "$f"; done'
```
