// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import type { Meta, StoryObj } from "@storybook/react-vite";
import { fn } from "storybook/test";
import { UsersTab } from "../components/UsersTab";
import type { User } from "../api/types";

const now = new Date().toISOString();

const sampleUsers: User[] = [
  { id: "1", username: "admin", role: "admin", suspended: false, created_at: now, updated_at: now },
  { id: "2", username: "alice", role: "user", suspended: false, created_at: now, updated_at: now },
  { id: "3", username: "bob", role: "user", suspended: true, created_at: now, updated_at: now },
];

const noopCreate = async () => ({ recovery_codes: ["ABCD-EFGH", "IJKL-MNOP", "QRST-UVWX", "YZ12-3456", "7890-ABCD", "EFGH-IJKL", "MNOP-QRST", "UVWX-YZ12"] });

const meta = {
  title: "Pages/UsersTab",
  component: UsersTab,
  parameters: {
    layout: "padded",
    backgrounds: {
      default: "dark",
      values: [{ name: "dark", value: "#030712" }],
    },
  },
  tags: ["autodocs"],
  args: {
    users: sampleUsers,
    onCreateUser: fn(noopCreate),
    onDeleteUser: fn(),
    onSuspendToggle: fn(),
    onRoleToggle: fn(),
  },
} satisfies Meta<typeof UsersTab>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  name: "With users",
};

export const SingleAdmin: Story = {
  name: "Single admin",
  args: {
    users: [sampleUsers[0]],
  },
};

export const Empty: Story = {
  name: "No users",
  args: {
    users: [],
  },
};

export const ManyUsers: Story = {
  name: "Many users",
  args: {
    users: [
      ...sampleUsers,
      { id: "4", username: "carol", role: "user", suspended: false, created_at: now, updated_at: now },
      { id: "5", username: "dave", role: "admin", suspended: false, created_at: now, updated_at: now },
      { id: "6", username: "eve", role: "user", suspended: true, created_at: now, updated_at: now },
      { id: "7", username: "frank", role: "user", suspended: false, created_at: now, updated_at: now },
    ],
  },
};
