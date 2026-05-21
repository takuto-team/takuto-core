// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Foundation stub for Phase 5 Part A (designer cut-plan migration order step
 * 1). This file will absorb `TicketDetailAiPanel` and the improve-with-AI
 * state/handlers from `TicketDetailModal` in a later commit (step 4).
 *
 * For now it exports only the `PendingImprovement` shape so the
 * StartWorkflow sub-components can depend on a stable import path before
 * the rest of the improve-with-AI logic moves here.
 */

export interface PendingImprovement {
  originalDescription: string;
  improvedDescription: string;
  improvedSummary?: string;
}
