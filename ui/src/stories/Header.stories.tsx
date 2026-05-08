import type { Meta, StoryObj } from "@storybook/react-vite";
import { fn } from "storybook/test";
import { MemoryRouter } from "react-router-dom";
import { Header } from "../components/Header";

const meta = {
  title: "Components/Header",
  component: Header,
  decorators: [(Story) => <MemoryRouter><Story /></MemoryRouter>],
  parameters: {
    layout: "fullscreen",
    backgrounds: {
      default: "dark",
      values: [{ name: "dark", value: "#030712" }],
    },
  },
  tags: ["autodocs"],
  args: {
    connected: true,
    authEnabled: false,
    githubAppConfigured: false,
    onLogout: fn(),
  },
} satisfies Meta<typeof Header>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Minimal: Story = {
  name: "Minimal (no bot, no repo, no auth)",
};

export const WithNamedApp: Story = {
  name: "Named GitHub App (sous-coder)",
  args: {
    githubAppConfigured: true,
    githubAppInstallationId: 12345,
    githubAppName: "sous-coder",
  },
};

export const WithAppNoName: Story = {
  name: "GitHub App without name configured",
  args: {
    githubAppConfigured: true,
    githubAppInstallationId: 12345,
  },
};

export const WithRepo: Story = {
  name: "With app + repository",
  args: {
    githubAppConfigured: true,
    githubAppInstallationId: 12345,
    githubAppName: "sous-coder",
    repoName: "maestro-core",
    repoHtmlUrl: "https://github.com/morphet81/maestro-core",
    onChangeRepo: fn(),
  },
};

export const WithRepoNoUrl: Story = {
  name: "With repository (no remote URL)",
  args: {
    githubAppConfigured: true,
    githubAppName: "sous-coder",
    repoName: "my-private-repo",
    repoHtmlUrl: null,
    onChangeRepo: fn(),
  },
};

export const LongRepoName: Story = {
  name: "Long repository name (truncated)",
  args: {
    githubAppConfigured: true,
    githubAppName: "sous-coder",
    repoName: "my-very-long-repository-name-that-should-truncate",
    repoHtmlUrl: "https://github.com/some-org/my-very-long-repository-name-that-should-truncate",
    onChangeRepo: fn(),
  },
};

export const Disconnected: Story = {
  name: "WebSocket disconnected",
  args: {
    connected: false,
    githubAppConfigured: true,
    githubAppName: "sous-coder",
    repoName: "maestro-core",
    repoHtmlUrl: "https://github.com/morphet81/maestro-core",
    onChangeRepo: fn(),
  },
};

export const FullFeatured: Story = {
  name: "All features enabled",
  args: {
    connected: true,
    authEnabled: true,
    githubAppConfigured: true,
    githubAppInstallationId: 12345,
    githubAppName: "sous-coder",
    repoName: "maestro-core",
    repoHtmlUrl: "https://github.com/morphet81/maestro-core",
    onChangeRepo: fn(),
  },
};
