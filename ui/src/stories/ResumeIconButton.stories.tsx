import type { Meta, StoryObj } from "@storybook/react-vite";
import { fn } from "storybook/test";
import { ResumeIconButton } from "../components/ResumeIconButton";

const meta = {
  title: "Atoms/ResumeIconButton",
  component: ResumeIconButton,
  parameters: {
    layout: "centered",
    backgrounds: {
      default: "dark",
      values: [{ name: "dark", value: "#030712" }],
    },
  },
  tags: ["autodocs"],
} satisfies Meta<typeof ResumeIconButton>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: { onClick: fn() },
};

export const WithTitle: Story = {
  args: { onClick: fn(), title: "Retry from last failure" },
};
