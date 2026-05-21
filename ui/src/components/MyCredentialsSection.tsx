// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Backwards-compatibility re-export. The section + its panels now live under
 * `./credentials/`; this file keeps the historical import path stable for
 * tests and consumers (CODING_STANDARDS §5 minimum viable change).
 */

export { MyCredentialsSection } from "./credentials/MyCredentialsSection";
