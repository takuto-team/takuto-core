import type { Meta, StoryObj } from "@storybook/react-vite";
import { Label } from "../components/Label";

function CheckIcon() {
  return (
    <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
    </svg>
  );
}

function XIcon() {
  return (
    <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
    </svg>
  );
}

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

export const WithIcon: Story = {
  args: { variant: "success", children: "Completed", icon: <CheckIcon /> },
};

export const WithIconDanger: Story = {
  name: "With icon (danger)",
  args: { variant: "danger", children: "Error", icon: <XIcon /> },
};

export const WithIconRunning: Story = {
  name: "With icon (animated dot)",
  args: {
    variant: "info",
    children: "Running",
    icon: <span style={{ width: "6px", height: "6px", borderRadius: "50%", backgroundColor: "currentColor", display: "inline-block", animation: "pulse 1.5s infinite" }} />,
  },
};

export const AllVariants: Story = {
  args: { variant: "default", children: "Label" },
  render: () => (
    <div style={{ display: "flex", gap: "8px", flexWrap: "wrap", alignItems: "center" }}>
      <Label variant="default">Default</Label>
      <Label variant="success" icon={<CheckIcon />}>Completed</Label>
      <Label variant="danger" icon={<XIcon />}>Error</Label>
      <Label variant="warning">Paused</Label>
      <Label variant="info" icon={<span className="w-1.5 h-1.5 rounded-full animate-pulse bg-current" />}>Running</Label>
      <Label variant="purple">Merged</Label>
    </div>
  ),
};
