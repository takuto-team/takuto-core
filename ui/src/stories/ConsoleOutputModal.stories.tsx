import type { Meta, StoryObj } from "@storybook/react-vite";
import { fn } from "storybook/test";
import { ConsoleOutputModal } from "../components/modals/ConsoleOutputModal";

const meta = {
  title: "Modals/ConsoleOutputModal",
  component: ConsoleOutputModal,
  parameters: {
    layout: "fullscreen",
    backgrounds: {
      default: "dark",
      values: [{ name: "dark", value: "#030712" }],
    },
  },
  tags: ["autodocs"],
} satisfies Meta<typeof ConsoleOutputModal>;

export default meta;
type Story = StoryObj<typeof meta>;

const manyLines = Array.from({ length: 80 }, (_, i) => ({
  text: `[${String(i + 1).padStart(3, "0")}] Processing item ${i + 1}...`,
  stream: "stdout" as const,
}));

export const Running: Story = {
  args: {
    onClose: fn(),
    state: {
      stepName: "implement_oauth",
      completed: false,
      lines: [
        { text: "Running Claude Code agent...", stream: "stdout" },
        { text: "Reading repository structure", stream: "stdout" },
        { text: "Found 142 files in src/", stream: "stdout" },
        { text: "Implementing OAuth2 flow", stream: "stdout" },
        { text: "Warning: deprecated API usage detected", stream: "stdout" },
        { text: "Writing ui/src/auth/oauth.ts", stream: "stdout" },
        { text: "Writing ui/src/auth/session.ts", stream: "stdout" },
      ],
    },
  },
};

export const Completed: Story = {
  args: {
    onClose: fn(),
    state: {
      stepName: "implement_oauth",
      completed: true,
      lines: [
        { text: "Running Claude Code agent...", stream: "stdout" },
        { text: "Reading repository structure", stream: "stdout" },
        { text: "Implementing OAuth2 flow", stream: "stdout" },
        { text: "All steps completed successfully.", stream: "stdout" },
        { text: "PR created: https://github.com/org/repo/pull/42", stream: "stdout" },
      ],
    },
  },
};

export const WithErrors: Story = {
  args: {
    onClose: fn(),
    state: {
      stepName: "lint_check",
      completed: false,
      lines: [
        { text: "Running lint checks...", stream: "stdout" },
        { text: "ESLint: checking 48 files", stream: "stdout" },
        { text: "error  'foo' is defined but never used  no-unused-vars", stream: "stderr" },
        { text: "error  Missing semicolon  semi", stream: "stderr" },
        { text: "warning  Prefer const over let  prefer-const", stream: "stdout" },
        { text: "ESLint: 2 errors, 1 warning found", stream: "stderr" },
        { text: "Process exited with code 1", stream: "stderr" },
      ],
    },
  },
};

export const Empty: Story = {
  args: {
    onClose: fn(),
    state: {
      stepName: "bootstrap",
      completed: false,
      lines: [],
    },
  },
};

export const LongOutput: Story = {
  args: {
    onClose: fn(),
    state: {
      stepName: "build",
      completed: true,
      lines: manyLines,
    },
  },
};
