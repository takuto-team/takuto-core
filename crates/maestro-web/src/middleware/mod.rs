// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Cross-cutting Tower/Axum middleware layers.
//!
//! - [`csrf`] — Origin/Referer allowlist for `POST/PUT/DELETE/PATCH` (plan-02 AC-1).
//! - [`security_headers`] — CSP, HSTS, X-Frame-Options, etc. on every response
//!   (plan-02 AC-6).

pub mod csrf;
pub mod security_headers;
