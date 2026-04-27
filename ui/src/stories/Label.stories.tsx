import type { Meta, StoryObj } from "@storybook/react-vite";
import { Label } from "../components/Label";

const meta = {
  title: "Atoms/Label",
  component: Label,
  parameters: {
    layout: "centered",
    backgrounds: {
      default: "dark",
      values: [{ name: "dark", value: "#030712" }],
    },
  },
  tags: ["autodocs"],
  argTypes: {
    variant: {
      control: "select",
      options: ["default", "success", "danger", "warning", "info", "purple"],
    },
    href: { control: "text" },
    children: { control: "text" },
  },
} satisfies Meta<typeof Label>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: { variant: "default", children: "Label" },
};

export const Success: Story = {
  args: { variant: "success", children: "Completed" },
};

export const Danger: Story = {
  args: { variant: "danger", children: "Failed" },
};

export const Warning: Story = {
  args: { variant: "warning", children: "Paused" },
};

export const Info: Story = {
  args: { variant: "info", children: "Running" },
};

export const Purple: Story = {
  args: { variant: "purple", children: "Merged" },
};

export const AsLink: Story = {
  args: {
    variant: "info",
    children: "PR #42",
    href: "https://github.com/org/repo/pull/42",
  },
};

export const MergedPrLink: Story = {
  name: "Merged PR link",
  args: {
    variant: "purple",
    children: "PR #42",
    href: "https://github.com/org/repo/pull/42",
  },
};

export const AllVariants: Story = {
  args: { variant: "default", children: "Label" },
  render: () => (
    <div style={{ display: "flex", gap: "8px", flexWrap: "wrap", alignItems: "center" }}>
      <Label variant="default">Default</Label>
      <Label variant="success">Success</Label>
      <Label variant="danger">Danger</Label>
      <Label variant="warning">Warning</Label>
      <Label variant="info">Info</Label>
      <Label variant="purple">Purple</Label>
    </div>
  ),
};
