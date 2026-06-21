// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * The header language switcher shows the current language code and a dropdown
 * of all supported languages; picking one changes the i18next language and
 * updates the document <html lang>.
 */

import { describe, it, expect, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, within, waitFor } from "@testing-library/react";
import i18n from "../i18n";
import { LanguageSwitcher } from "./LanguageSwitcher";
import { LANGUAGES } from "../i18n/config";

afterEach(async () => {
  cleanup();
  await i18n.changeLanguage("en");
});

describe("LanguageSwitcher", () => {
  it("shows the current language code on the trigger", () => {
    render(<LanguageSwitcher />);
    // English is pinned by the test setup → trigger shows "EN".
    expect(screen.getByRole("button", { name: /change language/i }).textContent).toBe("EN");
  });

  it("opens a menu listing all five language codes", () => {
    render(<LanguageSwitcher />);
    fireEvent.click(screen.getByRole("button", { name: /change language/i }));
    const menu = screen.getByRole("menu");
    for (const lang of LANGUAGES) {
      expect(within(menu).getByText(lang.label)).toBeTruthy();
    }
  });

  it("changes language and updates <html lang> when a code is picked", async () => {
    render(<LanguageSwitcher />);
    fireEvent.click(screen.getByRole("button", { name: /change language/i }));
    fireEvent.click(within(screen.getByRole("menu")).getByText("FR"));
    // changeLanguage resolves asynchronously on the i18n singleton.
    await waitFor(() => expect(i18n.language).toBe("fr"));
    expect(document.documentElement.lang).toBe("fr");
  });
});
