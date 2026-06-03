// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Add / Edit a single flow. Save submits the *entire* list (this flow inserted
 * or replaced) so the row is written atomically, matching the backend's
 * replace-the-row contract.
 *
 * Validation mirrors the backend `validate_user_flows`: unique name, unique
 * slug, >= 1 step, per-step name + prompt, per-skill name, and no dependency
 * cycle. Client checks are UX-only — the server re-validates and any reason the
 * client missed surfaces as a structured error above the footer.
 */

import { useMemo, useState } from "react";
import { slugify, type UserFlow } from "../../api/flows";
import { StepEditor, type SkillDraft, type StepDraft } from "./FlowEditor/StepEditor";
import { DependsOnSelect } from "./FlowEditor/DependsOnSelect";

interface FlowEditorModalProps {
  flows: UserFlow[];
  editIndex: number | null;
  onSubmit: (next: UserFlow[]) => Promise<void>;
  onClose: () => void;
}

/** Three-colour DFS cycle detection by flow name — a port of the backend's. */
function detectCycle(graph: { name: string; depends_on: string[] }[]): string | null {
  const deps = new Map<string, string[]>();
  for (const f of graph) deps.set(f.name, f.depends_on);
  const color = new Map<string, 0 | 1 | 2>();
  for (const k of deps.keys()) color.set(k, 0);

  let found: string | null = null;
  const visit = (node: string) => {
    if (found) return;
    color.set(node, 1);
    for (const child of deps.get(node) ?? []) {
      if (!deps.has(child)) continue;
      const c = color.get(child);
      if (c === 1) {
        found = child;
        return;
      }
      if (c === 0) {
        visit(child);
        if (found) return;
      }
    }
    color.set(node, 2);
  };

  for (const k of deps.keys()) {
    if (color.get(k) === 0) {
      visit(k);
      if (found) return found;
    }
  }
  return found;
}

function splitArgs(text: string): string[] {
  return text
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s !== "");
}

const blankStep = (): StepDraft => ({ name: "", prompt: "", skills: [] });

export function FlowEditorModal({ flows, editIndex, onSubmit, onClose }: FlowEditorModalProps) {
  const editing = editIndex !== null ? flows[editIndex] : null;

  const [name, setName] = useState(editing?.name ?? "");
  const [dependsOn, setDependsOn] = useState<string[]>(editing?.depends_on ?? []);
  const [steps, setSteps] = useState<StepDraft[]>(
    editing && editing.steps.length > 0
      ? editing.steps.map((s) => ({
          name: s.name,
          prompt: s.prompt,
          skills: s.skills.map((k) => ({ name: k.name, argsText: k.args.join(", ") })),
        }))
      : [blankStep()],
  );
  const [saving, setSaving] = useState(false);
  const [serverError, setServerError] = useState("");
  const [dragStep, setDragStep] = useState<number | null>(null);

  const otherFlows = useMemo(
    () => flows.filter((_, i) => i !== editIndex),
    [flows, editIndex],
  );
  const otherNames = useMemo(() => otherFlows.map((f) => f.name), [otherFlows]);

  const trimmedName = name.trim();
  const slug = slugify(trimmedName);

  const nameError = useMemo(() => {
    if (trimmedName === "") return null;
    if (otherNames.includes(trimmedName)) {
      return `A flow named "${trimmedName}" already exists.`;
    }
    if (slug === "") {
      return "A flow name must contain at least one letter or number.";
    }
    const collision = otherFlows.find((f) => slugify(f.name) === slug);
    if (collision) {
      return `This name collides with "${collision.name}" — both become "${slug}".`;
    }
    return null;
  }, [trimmedName, slug, otherNames, otherFlows]);

  const cycleError = useMemo(() => {
    if (trimmedName === "" || nameError) return null;
    const graph = [
      ...otherFlows.map((f) => ({ name: f.name, depends_on: f.depends_on })),
      { name: trimmedName, depends_on: dependsOn },
    ];
    const involved = detectCycle(graph);
    return involved
      ? `These dependencies create a cycle involving "${involved}". Remove one link to save.`
      : null;
  }, [trimmedName, nameError, otherFlows, dependsOn]);

  const stepsValid =
    steps.length >= 1 &&
    steps.every(
      (s) =>
        s.name.trim() !== "" &&
        s.prompt.trim() !== "" &&
        !s.skills.some((k) => k.name.trim() === "" && k.argsText.trim() !== ""),
    );

  const canSave = trimmedName !== "" && !nameError && !cycleError && stepsValid && !saving;

  const setStep = (i: number, next: StepDraft) =>
    setSteps((prev) => prev.map((s, si) => (si === i ? next : s)));
  const addStep = () => setSteps((prev) => [...prev, blankStep()]);
  const removeStep = (i: number) => setSteps((prev) => prev.filter((_, si) => si !== i));

  const dropStep = (target: number) => {
    const from = dragStep;
    setDragStep(null);
    if (from === null || from === target) return;
    setSteps((prev) => {
      const next = [...prev];
      const [moved] = next.splice(from, 1);
      next.splice(target, 0, moved);
      return next;
    });
  };

  const buildFlow = (): UserFlow => ({
    name: trimmedName,
    depends_on: dependsOn,
    steps: steps.map((s) => ({
      name: s.name,
      prompt: s.prompt,
      skills: s.skills
        .filter((k: SkillDraft) => k.name.trim() !== "")
        .map((k) => ({ name: k.name.trim(), args: splitArgs(k.argsText) })),
    })),
  });

  const handleSave = async () => {
    if (!canSave) return;
    const flow = buildFlow();
    const next =
      editIndex === null ? [...flows, flow] : flows.map((f, i) => (i === editIndex ? flow : f));
    setSaving(true);
    setServerError("");
    try {
      await onSubmit(next);
      onClose();
    } catch (e) {
      setServerError(String((e as Error).message || e));
      setSaving(false);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape" && !saving) {
      onClose();
    } else if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
      handleSave();
    }
  };

  const lastStep = steps.length === 1;

  return (
    <div className="modal-backdrop" onClick={() => !saving && onClose()} onKeyDown={handleKeyDown}>
      <div
        className="bg-gray-900 border border-gray-700 rounded-xl max-w-2xl w-full mx-4 max-h-[85vh] flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between p-4 border-b border-gray-800">
          <h3 className="text-lg font-medium text-white">
            {editIndex === null ? "Add flow" : "Edit flow"}
          </h3>
          <button
            type="button"
            onClick={() => !saving && onClose()}
            className="text-gray-500 hover:text-gray-300 cursor-pointer"
            aria-label="Close"
          >
            &times;
          </button>
        </div>

        <div className="overflow-y-auto flex-1 p-4 space-y-5">
          <div>
            <label className="block text-sm text-gray-400 mb-1">Name</label>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="e.g. lint_and_test"
              autoFocus
              className="w-full bg-gray-950 border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-blue-500"
            />
            {nameError && <p className="text-sm text-red-400 mt-1">{nameError}</p>}
          </div>

          <div>
            <label className="block text-sm text-gray-400 mb-1">Depends on</label>
            <DependsOnSelect options={otherNames} selected={dependsOn} onChange={setDependsOn} />
            {cycleError && <p className="text-sm text-amber-400 mt-1">{cycleError}</p>}
          </div>

          <div className="space-y-2">
            <div className="flex items-center justify-between">
              <label className="text-sm text-gray-400">Steps</label>
              <button
                type="button"
                onClick={addStep}
                className="text-sm text-blue-400 hover:text-blue-300 cursor-pointer"
              >
                + Add step
              </button>
            </div>
            {steps.map((step, i) => (
              <StepEditor
                key={i}
                index={i}
                step={step}
                canRemove={!lastStep}
                draggable={steps.length > 1}
                isDragging={dragStep === i}
                onChange={(next) => setStep(i, next)}
                onRemove={() => removeStep(i)}
                onDragStart={() => setDragStep(i)}
                onDrop={() => dropStep(i)}
                onDragEnd={() => setDragStep(null)}
              />
            ))}
          </div>
        </div>

        <div className="p-4 border-t border-gray-800 space-y-2">
          {serverError && <p className="text-sm text-red-400">{serverError}</p>}
          <div className="flex justify-end gap-3">
            <button
              type="button"
              onClick={() => !saving && onClose()}
              disabled={saving}
              className="text-sm px-4 py-2 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
            >
              Cancel
            </button>
            <button
              type="button"
              onClick={handleSave}
              disabled={!canSave}
              className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
            >
              {saving ? "Saving…" : editIndex === null ? "Create flow" : "Save flow"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
