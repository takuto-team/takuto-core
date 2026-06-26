// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Global vitest setup. Initializes i18next with the real (bundled) resources
 * and pins the language to English so `t()` returns the English source copy
 * synchronously in every test — existing tests that assert English text keep
 * passing without per-test provider wrapping.
 */

import { configure } from "@testing-library/react";

import i18n from "../i18n";

void i18n.changeLanguage("en");

// CI runners are markedly slower than local, so the 1000ms default lets
// `waitFor`/`findBy` race multi-hop async chains (e.g. save → refetch →
// re-render). Give them more headroom; passing assertions still resolve fast.
configure({ asyncUtilTimeout: 5000 });
