// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { useState } from "react";
import type { Meta, StoryObj } from "@storybook/react-vite";
import { fn } from "storybook/test";
import { MemoryRouter } from "react-router-dom";
import {
  ProviderForm,
  ProviderSwitchConfirm,
  type ProviderDraft,
} from "../components/AiProviderSettingsSection";
import { ToastProvider } from "../hooks/useToast";
import type { AgentProviderId } from "../api/types";

const meta = {
  title: "Components/AiProviderSettingsSection",
  parameters: {
    layout: "fullscreen",
    backgrounds: {
      default: "dark",
      values: [{ name: "dark", value: "#030712" }],
    },
  },
  decorators: [
    (Story) => (
      <ToastProvider>
        <MemoryRouter>
          <div className="p-8 max-w-3xl mx-auto">
            <Story />
          </div>
        </MemoryRouter>
      </ToastProvider>
    ),
  ],
} satisfies Meta;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * Wraps <ProviderForm> with local state so each story behaves like a real
 * form (typing into fields, toggling the dropdown). The stories are static
 * starting points — interactions are local-only.
 */
function FormHarness({
  initialProvider,
  initialDraft,
  initialAvailable = ["claude", "cursor", "codex", "opencode"],
}: {
  initialProvider: AgentProviderId;
  initialDraft: ProviderDraft;
  initialAvailable?: AgentProviderId[];
}) {
  const [provider, setProvider] = useState<AgentProviderId>(initialProvider);
  const [draft, setDraft] = useState<ProviderDraft>(initialDraft);
  const [available, setAvailable] =
    useState<AgentProviderId[]>(initialAvailable);
  return (
    <ProviderForm
      selectedProvider={provider}
      onSelectProvider={setProvider}
      draft={draft}
      onDraftChange={setDraft}
      availableProviders={available}
      onToggleAvailable={(p) =>
        setAvailable((prev) =>
          prev.includes(p) ? prev.filter((x) => x !== p) : [...prev, p],
        )
      }
      onSave={fn()}
      saving={false}
    />
  );
}

const EMPTY: ProviderDraft = {
  model: "",
  base_url: "",
  cli: "agent",
  provider_name: "",
  extra_args_text: "",
  allow_shared_default: false,
};

/* ── ProviderForm variants ── */

export const ClaudeSelectedNoBaseUrl: Story = {
  name: "Claude — no base URL",
  render: () => (
    <FormHarness
      initialProvider="claude"
      initialDraft={{ ...EMPTY, model: "claude-3-5-sonnet-latest" }}
    />
  ),
};

export const CursorWithCliAndDisabledBaseUrl: Story = {
  name: "Cursor — CLI shown, base URL disabled",
  render: () => (
    <FormHarness
      initialProvider="cursor"
      initialDraft={{ ...EMPTY, cli: "agent", model: "Auto" }}
    />
  ),
};

export const CodexWithPhase4Warning: Story = {
  name: "Codex — Phase 4 warning visible",
  render: () => (
    <FormHarness
      initialProvider="codex"
      initialDraft={{
        ...EMPTY,
        model: "gpt-5",
        provider_name: "openai",
        base_url: "",
      }}
    />
  ),
};

export const OpenCodeWithLmStudioRecipe: Story = {
  name: "OpenCode — base URL set for LM Studio",
  render: () => (
    <FormHarness
      initialProvider="opencode"
      initialDraft={{
        ...EMPTY,
        model: "lmstudio/llama-3.1-8b",
        base_url: "http://lm-studio:1234/v1",
      }}
    />
  ),
};

/* ── ProviderSwitchConfirm ── */

export const ProviderSwitchConfirmClaudeToCursor: Story = {
  name: "Provider switch confirm (claude → cursor)",
  render: () => (
    <ProviderSwitchConfirm
      from="claude"
      to="cursor"
      onCancel={fn()}
      onConfirm={fn()}
    />
  ),
};

export const ProviderSwitchConfirmCodexToOpenCode: Story = {
  name: "Provider switch confirm (codex → opencode)",
  render: () => (
    <ProviderSwitchConfirm
      from="codex"
      to="opencode"
      onCancel={fn()}
      onConfirm={fn()}
    />
  ),
};

/* ── Denied extra-arg toast (rendered manually via a story helper) ── */

import { useToast } from "../hooks/useToast";

function DeniedExtraArgPreview() {
  const { showToast } = useToast();
  // Fire the toast as soon as the story mounts so the docs panel always
  // shows the rendered error toast in the corner.
  useState(() => {
    showToast(
      "extra_args contains a Maestro-owned flag: --dangerously-skip-permissions (code: denied_extra_arg)",
      "error",
    );
    return null;
  });
  return (
    <p className="text-sm text-gray-400">
      The toast appears in the bottom-right corner — error variant from
      <code className="text-gray-200"> useToast</code>. Triggered when{" "}
      <code className="text-gray-200">putAgentConfig</code> rejects a
      Maestro-owned extra arg.
    </p>
  );
}

export const DeniedExtraArgErrorToast: Story = {
  name: "Denied extra-arg error toast",
  render: () => <DeniedExtraArgPreview />,
};
