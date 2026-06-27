import type { Page } from '@playwright/test';
import type { WorkflowEvent } from './types.js';

/** Matcher for {@link WorkflowEventStream.waitForEvent}. All set fields must hold. */
export interface EventMatch {
  /** `event_type` discriminator, e.g. `work_item_updated`, `port_forwarded`,
   *  `run_command_port_forwarded` (`engine/types.rs:77`). */
  eventType?: string;
  /** Restrict to a single work item. */
  ticketKey?: string;
  /** Require `forwarded_port` to be present (the port-forward events). */
  requireForwardedPort?: boolean;
}

/** Per-page key under which buffered events accumulate on `window`. */
const BUFFER_KEY = '__takutoWorkflowEvents';

/**
 * Live subscription to `GET /ws` for the implement-workflow specs
 * (`IMPLEMENT_WORKFLOW_CONTRACT.md §3.8`).
 *
 * The socket is opened inside the page so the same-origin handshake carries the
 * authenticated `takuto_session` cookie automatically (no manual cookie
 * plumbing, no extra dependency). Every `WorkflowEvent` is buffered on the page;
 * {@link waitForEvent} resolves once a buffered or future event matches.
 *
 * Connect BEFORE the action that emits the event, then await:
 *
 * ```ts
 * const events = await WorkflowEventStream.connect(page);
 * await api.startRunCommand(ticketKey, 0);
 * const ev = await events.waitForEvent(
 *   { eventType: 'run_command_port_forwarded', ticketKey, requireForwardedPort: true },
 *   60_000,
 * );
 * ```
 */
export class WorkflowEventStream {
  private constructor(private readonly page: Page) {}

  /** Open the socket and start buffering events. Resolves once it is OPEN. */
  static async connect(page: Page): Promise<WorkflowEventStream> {
    await page.evaluate((key) => {
      const w = window as unknown as Record<string, unknown>;
      if (w[key]) {
        return;
      }
      const buffer: unknown[] = [];
      w[key] = buffer;
      const wsUrl = `${location.origin.replace(/^http/, 'ws')}/ws`;
      const socket = new WebSocket(wsUrl);
      socket.addEventListener('message', (ev: MessageEvent) => {
        try {
          buffer.push(JSON.parse(String(ev.data)));
        } catch {
          // Non-JSON frames are not workflow events; ignore.
        }
      });
      w[`${key}__socket`] = socket;
    }, BUFFER_KEY);

    // Wait for the socket to reach OPEN so events emitted by the very next
    // action are not missed on a slow handshake.
    await page.waitForFunction(
      (key) => {
        const w = window as unknown as Record<string, unknown>;
        const socket = w[`${key}__socket`] as { readyState: number } | undefined;
        return socket != null && socket.readyState === 1;
      },
      BUFFER_KEY,
      { timeout: 15_000 },
    );

    return new WorkflowEventStream(page);
  }

  /**
   * Resolve with the first buffered-or-future event matching `match`. Throws via
   * Playwright's timeout if none arrives within `timeoutMs`.
   */
  async waitForEvent(match: EventMatch, timeoutMs = 60_000): Promise<WorkflowEvent> {
    const handle = await this.page.waitForFunction(
      ({ key, m }) => {
        const w = window as unknown as Record<string, unknown>;
        const buffer = (w[key] as Array<Record<string, unknown>> | undefined) ?? [];
        const hit = buffer.find(
          (e) =>
            (!m.eventType || e.event_type === m.eventType) &&
            (!m.ticketKey || e.ticket_key === m.ticketKey) &&
            (!m.requireForwardedPort || e.forwarded_port != null),
        );
        return hit ?? null;
      },
      { key: BUFFER_KEY, m: match },
      { timeout: timeoutMs },
    );
    return (await handle.jsonValue()) as unknown as WorkflowEvent;
  }

  /** Snapshot of every event buffered so far (e.g. for diagnostics on failure). */
  async all(): Promise<WorkflowEvent[]> {
    return this.page.evaluate((key) => {
      const w = window as unknown as Record<string, unknown>;
      return ((w[key] as WorkflowEvent[] | undefined) ?? []).slice();
    }, BUFFER_KEY);
  }
}
