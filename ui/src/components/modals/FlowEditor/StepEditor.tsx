// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * One step row inside the flow editor, with its own skills sub-repeater.
 *
 * Works on editor-local draft shapes: a skill's `args` is kept as the raw
 * comma-separated text the user types and split into the wire `args` array
 * only at save time. Keeping it as text here avoids losing partially-typed
 * separators on every keystroke.
 */

/** A skill row in the editor — `argsText` is the raw comma-separated input. */
export interface SkillDraft {
  name: string;
  argsText: string;
}

/** A step in the editor draft. */
export interface StepDraft {
  name: string;
  prompt: string;
  skills: SkillDraft[];
}

interface StepEditorProps {
  index: number;
  step: StepDraft;
  canRemove: boolean;
  draggable: boolean;
  isDragging: boolean;
  onChange: (next: StepDraft) => void;
  onRemove: () => void;
  onDragStart: () => void;
  onDrop: () => void;
  onDragEnd: () => void;
}

export function StepEditor({
  index,
  step,
  canRemove,
  draggable,
  isDragging,
  onChange,
  onRemove,
  onDragStart,
  onDrop,
  onDragEnd,
}: StepEditorProps) {
  const setSkill = (i: number, next: SkillDraft) => {
    onChange({ ...step, skills: step.skills.map((s, si) => (si === i ? next : s)) });
  };
  const addSkill = () => {
    onChange({ ...step, skills: [...step.skills, { name: "", argsText: "" }] });
  };
  const removeSkill = (i: number) => {
    onChange({ ...step, skills: step.skills.filter((_, si) => si !== i) });
  };

  return (
    <div
      draggable={draggable}
      onDragStart={(e) => {
        e.dataTransfer.effectAllowed = "move";
        onDragStart();
      }}
      onDragOver={(e) => e.preventDefault()}
      onDrop={(e) => {
        e.preventDefault();
        onDrop();
      }}
      onDragEnd={onDragEnd}
      className={`border border-gray-700 rounded-lg bg-gray-800 p-3 space-y-2 ${isDragging ? "opacity-40" : ""}`}
    >
      <div className="flex items-center gap-2">
        <span
          className={`text-gray-600 select-none ${draggable ? "cursor-grab" : "cursor-default"}`}
          title="Drag to reorder"
          aria-hidden="true"
        >
          ⠿
        </span>
        <span className="text-sm font-medium text-gray-300">Step {index + 1}</span>
        <button
          type="button"
          onClick={onRemove}
          disabled={!canRemove}
          title={canRemove ? undefined : "A flow needs at least one step."}
          className="ml-auto text-sm text-red-500/70 hover:text-red-400 disabled:text-gray-600 disabled:cursor-not-allowed cursor-pointer"
        >
          Remove
        </button>
      </div>

      <div>
        <label className="block text-xs text-gray-500 mb-1">Step name</label>
        <input
          type="text"
          value={step.name}
          onChange={(e) => onChange({ ...step, name: e.target.value })}
          placeholder="e.g. cargo fmt"
          className="w-full bg-gray-950 border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-blue-500"
        />
      </div>

      <div>
        <label className="block text-xs text-gray-500 mb-1">Prompt</label>
        <textarea
          value={step.prompt}
          onChange={(e) => onChange({ ...step, prompt: e.target.value })}
          rows={10}
          placeholder="Text sent verbatim to the agent for this step."
          className="w-full bg-gray-950 border border-gray-700 rounded px-3 py-2 text-sm font-mono text-gray-200 focus:outline-none focus:border-blue-500 resize-y"
        />
      </div>

      <div className="space-y-1.5">
        <div className="flex items-center justify-between">
          <label className="text-xs text-gray-500">Skills</label>
          <button
            type="button"
            onClick={addSkill}
            className="text-xs text-blue-400 hover:text-blue-300 cursor-pointer"
          >
            + Add skill
          </button>
        </div>
        {step.skills.map((skill, si) => (
          <div key={si} className="flex items-center gap-2">
            <input
              type="text"
              value={skill.name}
              onChange={(e) => setSkill(si, { ...skill, name: e.target.value })}
              placeholder="skill name"
              className="flex-1 min-w-0 bg-gray-950 border border-gray-700 rounded px-2.5 py-1 text-sm text-gray-200 focus:outline-none focus:border-blue-500"
            />
            <input
              type="text"
              value={skill.argsText}
              onChange={(e) => setSkill(si, { ...skill, argsText: e.target.value })}
              placeholder="args (comma-separated)"
              className="flex-1 min-w-0 bg-gray-950 border border-gray-700 rounded px-2.5 py-1 text-sm font-mono text-gray-200 focus:outline-none focus:border-blue-500"
            />
            <button
              type="button"
              onClick={() => removeSkill(si)}
              className="text-sm text-gray-500 hover:text-gray-300 cursor-pointer whitespace-nowrap"
            >
              Rm
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}
