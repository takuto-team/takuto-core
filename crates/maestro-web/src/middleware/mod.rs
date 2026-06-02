// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Cross-cutting Tower/Axum middleware layers.
//!
//! - [`csrf`] — Origin/Referer allowlist for `POST/PUT/DELETE/PATCH`.
//! - [`security_headers`] — CSP, HSTS, X-Frame-Options, etc. on every response.

pub mod csrf;
pub mod deprecation;
pub mod security_headers;
