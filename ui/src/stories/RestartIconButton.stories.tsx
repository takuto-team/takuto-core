import type { Meta, StoryObj } from "@storybook/react-vite";
import { fn } from "storybook/test";
import { RestartIconButton } from "../components/RestartIconButton";

const meta = {
  title: "Atoms/RestartIconButton",
  component: RestartIconButton,
  parameters: {
    layout: "centered",
    backgrounds: {
      default: "dark",
      values: [{ name: "dark", value: "#030712" }],
    },
  },
  tags: ["autodocs"],
} satisfies Meta<typeof RestartIconButton>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: { onClick: fn() },
};

export const WithTitle: Story = {
  args: { onClick: fn(), title: "Restart workflow" },
};
