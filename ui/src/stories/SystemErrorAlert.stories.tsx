import type { Meta, StoryObj } from "@storybook/react-vite";
import { fn } from "storybook/test";
import { SystemErrorAlert } from "../components/SystemErrorAlert";
import type { SystemError } from "../hooks/useWorkflows";

/* ── SystemErrorAlert ── */

const errorMeta = {
  title: "Components/SystemErrorAlert",
  component: SystemErrorAlert,
  parameters: {
    layout: "fullscreen",
    backgrounds: {
      default: "dark",
      values: [{ name: "dark", value: "#030712" }],
    },
  },
  tags: ["autodocs"],
} satisfies Meta<typeof SystemErrorAlert>;

export default errorMeta;
type ErrorStory = StoryObj<typeof errorMeta>;

const err1: SystemError = { id: 1, ticketKey: "PROJ-123", message: "Action failed: could not connect to the server.", timestamp: new Date() };
const err2: SystemError = { id: 2, ticketKey: "PROJ-456", message: "Command failed: git push rejected (non-fast-forward).\nPlease pull before pushing.", timestamp: new Date() };

export const SingleError: ErrorStory = {
  args: {
    errors: [err1],
    onDismiss: fn(),
  },
};

export const MultipleErrors: ErrorStory = {
  args: {
    errors: [
      { id: 1, ticketKey: "PROJ-123", message: "Action failed: timeout after 30 seconds.", timestamp: new Date() },
      err2,
    ],
    onDismiss: fn(),
  },
};

export const NoErrors: ErrorStory = {
  args: {
    errors: [],
    onDismiss: fn(),
  },
};
