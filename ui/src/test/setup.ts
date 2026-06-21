// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Global vitest setup. Initializes i18next with the real (bundled) resources
 * and pins the language to English so `t()` returns the English source copy
 * synchronously in every test — existing tests that assert English text keep
 * passing without per-test provider wrapping.
 */

import i18n from "../i18n";

void i18n.changeLanguage("en");
