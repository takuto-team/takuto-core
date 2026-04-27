import type { Meta, StoryObj } from "@storybook/react-vite";
import { Button } from "../components/Button";

const meta = {
  title: "Components/Button",
  component: Button,
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
      options: ["primary", "secondary", "success", "danger"],
      description: "Visual style variant",
    },
    children: { control: "text" },
    disabled: { control: "boolean" },
  },
} satisfies Meta<typeof Button>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Primary: Story = {
  args: {
    variant: "primary",
    children: "Start workflow",
  },
};

export const Secondary: Story = {
  args: {
    variant: "secondary",
    children: "Show description",
  },
};

export const Success: Story = {
  args: {
    variant: "success",
    children: "Mark as Done",
  },
};

export const Danger: Story = {
  args: {
    variant: "danger",
    children: "Delete",
  },
};

export const Disabled: Story = {
  args: {
    variant: "primary",
    children: "Disabled action",
    disabled: true,
  },
};

export const AllVariants: Story = {
  args: {
    variant: "primary",
    children: "Primary",
  },
  render: () => (
    <div style={{ display: "flex", gap: "8px", flexWrap: "wrap" }}>
      <Button variant="primary" onClick={() => {}}>Primary</Button>
      <Button variant="secondary" onClick={() => {}}>Secondary</Button>
      <Button variant="success" onClick={() => {}}>Success</Button>
      <Button variant="danger" onClick={() => {}}>Danger</Button>
    </div>
  ),
};
