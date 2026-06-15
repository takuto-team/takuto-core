// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Cryptographically random tokens: the editor `?tkn=` value (UUIDv4) and
//! the shared-port proxy path token (16-byte OS CSPRNG, 128 bits).

pub(super) const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// Generate a cryptographically random connection token for editor sessions.
/// Returns a 32-character lowercase hex string (UUID v4 simple format).
///
/// NOTE: This token is consumed by `openvscode-server`'s built-in `?tkn=`
/// authentication and `ttyd`'s `-b /TOKEN` base-path. It is NOT the
/// session path token used by the shared-port reverse proxy — see
/// [`generate_session_path_token`] for that.
pub fn generate_connection_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Generate a session path token for the shared-port reverse proxy.
///
/// Returns a 32-character lowercase hex string encoding 16 bytes (128 bits)
/// drawn from the operating system's CSPRNG via [`getrandom`]. UUID v4 is
/// deliberately NOT used here: a v4 UUID has only 122 random bits because
/// six bits encode the version + variant nibbles, which falls below the
/// ≥128-bit entropy floor required by GH-45 for the session URL path.
///
/// Panicking only on `getrandom` failure is acceptable here because:
/// - failure means the kernel CSPRNG is unavailable, in which case we have
///   no business minting URL secrets at all;
/// - the call site in [`crate::container`] runs on a worker thread, not the
///   axum request path, so a panic surfaces as a 500 to the caller, not a
///   silent token reuse.
pub fn generate_session_path_token() -> String {
    let mut buf = [0u8; 16];
    // SAFETY: `getrandom::fill` only fails when the OS CSPRNG is
    // unavailable. See the function-level doc comment above for the full
    // rationale — token reuse is unacceptable, so panicking is correct.
    getrandom::fill(&mut buf).expect("OS CSPRNG (getrandom) must be available");
    let mut out = String::with_capacity(32);
    for byte in buf {
        // hex::encode adds a transitive dep we don't need in core. Hand-roll
        // the 32-char lowercase hex encoding to keep the surface small.
        out.push(HEX_DIGITS[(byte >> 4) as usize] as char);
        out.push(HEX_DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}
