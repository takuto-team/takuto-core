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

import { useEffect, useRef, useState } from "react";
import { EditableName } from "../../EditableName";
import { TrashIcon } from "../../icons";
import { improveText } from "../../../api/flows";

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

const IMPROVE_TIMEOUT_SECS = 300;

function formatCountdown(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${m}:${String(s).padStart(2, "0")}`;
}

interface StepEditorProps {
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

  const [improving, setImproving] = useState(false);
  const [improveError, setImproveError] = useState<string | null>(null);
  const [originalPrompt, setOriginalPrompt] = useState<string | null>(null);
  const [countdown, setCountdown] = useState(IMPROVE_TIMEOUT_SECS);
  const abortRef = useRef<AbortController | null>(null);
  const tickRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const startCountdown = () => {
    setCountdown(IMPROVE_TIMEOUT_SECS);
    if (tickRef.current) clearInterval(tickRef.current);
    tickRef.current = setInterval(() => {
      setCountdown((p) => Math.max(0, p - 1));
    }, 1000);
  };

  const stopCountdown = () => {
    if (tickRef.current) {
      clearInterval(tickRef.current);
      tickRef.current = null;
    }
  };

  const handleImprove = async () => {
    if (step.prompt.trim() === "" || improving) return;
    const snapshot = step.prompt;
    setImproving(true);
    setImproveError(null);
    startCountdown();
    abortRef.current = new AbortController();
    try {
      const improved = await improveText(snapshot, abortRef.current.signal);
      setOriginalPrompt(snapshot);
      onChange({ ...step, prompt: improved });
    } catch (e) {
      if (e instanceof Error && e.name !== "AbortError") {
        setImproveError(e.message || "Improve request failed");
      }
    } finally {
      setImproving(false);
      stopCountdown();
      abortRef.current = null;
    }
  };

  const handleCancelImprove = () => {
    abortRef.current?.abort();
    abortRef.current = null;
    setImproving(false);
    stopCountdown();
  };

  const handleRevert = () => {
    if (originalPrompt === null) return;
    onChange({ ...step, prompt: originalPrompt });
    setOriginalPrompt(null);
  };

  useEffect(() => {
    return () => {
      abortRef.current?.abort();
      if (tickRef.current) clearInterval(tickRef.current);
    };
  }, []);

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
      className={`relative border border-gray-800 rounded-lg bg-gray-950 p-3 space-y-2 ${isDragging ? "opacity-40" : ""}`}
    >
      {improving && (
        <div className="absolute inset-0 z-10 flex flex-col items-center justify-center bg-gray-900/85 backdrop-blur-sm rounded-lg">
          <div className="w-6 h-6 border-2 border-gray-600 border-t-purple-400 rounded-full animate-spin" />
          <p className="mt-3 text-sm text-gray-300">Improving prompt…</p>
          <p className="mt-1 text-xs text-gray-500">{formatCountdown(countdown)}</p>
          <button
            type="button"
            onClick={handleCancelImprove}
            className="mt-3 text-xs px-3 py-1.5 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
          >
            Cancel
          </button>
        </div>
      )}
      <div className="flex items-center gap-2">
        <span
          className={`text-gray-600 select-none ${draggable ? "cursor-grab" : "cursor-default"}`}
          title="Drag to reorder"
          aria-hidden="true"
        >
          ⠿
        </span>
        <EditableName
          value={step.name}
          onChange={(next) => onChange({ ...step, name: next })}
          placeholder="Untitled step"
          textClassName="flex-1 text-sm font-medium"
        />
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
        <label className="block text-xs text-gray-500 mb-1">Prompt</label>
        <textarea
          value={step.prompt}
          onChange={(e) => onChange({ ...step, prompt: e.target.value })}
          rows={10}
          placeholder="Text sent verbatim to the agent for this step."
          className="w-full bg-gray-950 border border-gray-700 rounded px-3 py-2 text-sm font-mono text-gray-200 focus:outline-none focus:border-blue-500 resize-y"
        />
        <div className="mt-2 flex items-center gap-2 flex-wrap">
          <button
            type="button"
            onClick={handleImprove}
            disabled={improving || step.prompt.trim() === ""}
            className="text-xs px-3 py-1 rounded-lg bg-purple-600/20 text-purple-300 border border-purple-500/30 hover:bg-purple-600/30 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
          >
            {improving ? "Improving…" : "Improve with AI"}
          </button>
          {originalPrompt !== null && !improving && (
            <button
              type="button"
              onClick={handleRevert}
              className="text-xs px-3 py-1 rounded-lg bg-gray-800 text-gray-300 border border-gray-700 hover:bg-gray-700 cursor-pointer"
            >
              Revert
            </button>
          )}
          {improveError && <span className="text-xs text-red-400">{improveError}</span>}
        </div>
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
              title="Remove skill"
              aria-label="Remove skill"
              className="p-1 text-gray-500 hover:text-red-400 cursor-pointer flex-shrink-0"
            >
              <TrashIcon />
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}
