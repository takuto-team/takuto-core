// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Inline editor body for a single flow. Rendered inside an expanded `FlowCard`
 * (existing flow) or inside the "new draft" card the Add button appends to
 * the list. Save submits the entire flow list with this flow either replaced
 * (`editIndex` set) or appended (`editIndex === null`), matching the
 * backend's replace-the-row contract.
 *
 * Validation mirrors the backend `validate_user_flows`: unique name, unique
 * slug, >= 1 step, per-step name + prompt, per-skill name, and no dependency
 * cycle. Client checks are UX-only — the server re-validates and any reason
 * the client missed surfaces as a structured error above the footer.
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { slugify, type UserFlow } from "../api/flows";
import { StepEditor, type SkillDraft, type StepDraft } from "./modals/FlowEditor/StepEditor";
import { DependsOnSelect } from "./modals/FlowEditor/DependsOnSelect";

interface FlowEditorProps {
  flows: UserFlow[];
  editIndex: number | null;
  /** Current draft name, owned by the parent so the card header can render it. */
  name: string;
  /** Surface client-side name validation upward (parent shows the error near the header). */
  onNameError?: (err: string | null) => void;
  onSubmit: (next: UserFlow[]) => Promise<void>;
  onCancel: () => void;
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

export function FlowEditor({
  flows,
  editIndex,
  name,
  onNameError,
  onSubmit,
  onCancel,
}: FlowEditorProps) {
  const editing = editIndex !== null ? flows[editIndex] : null;

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

  const lastStepRef = useRef<HTMLDivElement | null>(null);
  const prevStepCountRef = useRef(steps.length);

  useEffect(() => {
    if (steps.length > prevStepCountRef.current && lastStepRef.current?.scrollIntoView) {
      lastStepRef.current.scrollIntoView({ behavior: "smooth", block: "nearest" });
    }
    prevStepCountRef.current = steps.length;
  }, [steps.length]);

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

  useEffect(() => {
    if (onNameError) onNameError(nameError);
  }, [nameError, onNameError]);

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
    } catch (e) {
      setServerError(String((e as Error).message || e));
      setSaving(false);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
      e.preventDefault();
      handleSave();
    }
  };

  const lastStep = steps.length === 1;

  return (
    <div
      className="border-t border-gray-800 px-4 py-4 space-y-5 bg-gray-900 rounded-b-lg"
      onKeyDown={handleKeyDown}
    >
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
          <div key={i} ref={i === steps.length - 1 ? lastStepRef : undefined}>
            <StepEditor
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
          </div>
        ))}
      </div>

      <div className="space-y-2 pt-2 border-t border-gray-800">
        {serverError && <p className="text-sm text-red-400">{serverError}</p>}
        <div className="flex justify-end gap-3">
          <button
            type="button"
            onClick={() => !saving && onCancel()}
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
  );
}
