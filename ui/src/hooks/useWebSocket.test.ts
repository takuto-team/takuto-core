import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { renderHook } from "@testing-library/react";
import { useWebSocket } from "./useWebSocket";

// WebSocket stub
class MockWebSocket {
  static instances: MockWebSocket[] = [];
  url: string;
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  closed = false;

  constructor(url: string) {
    this.url = url;
    MockWebSocket.instances.push(this);
    // Auto-trigger onopen in next microtask
    queueMicrotask(() => this.onopen?.());
  }

  close() {
    this.closed = true;
  }

  // Helper: simulate server sending a message
  simulateMessage(data: string) {
    this.onmessage?.({ data });
  }

  simulateClose() {
    this.onclose?.();
  }
}

beforeEach(() => {
  MockWebSocket.instances = [];
  vi.stubGlobal("WebSocket", MockWebSocket);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("useWebSocket", () => {
  it("connects to /ws", async () => {
    const handler = vi.fn();
    const { result } = renderHook(() => useWebSocket(handler));

    await vi.waitFor(() => {
      expect(MockWebSocket.instances).toHaveLength(1);
    });

    expect(MockWebSocket.instances[0].url).toContain("/ws");

    await vi.waitFor(() => {
      expect(result.current.connected).toBe(true);
    });
  });

  it("dispatches parsed JSON messages to the handler", async () => {
    const handler = vi.fn();
    renderHook(() => useWebSocket(handler));

    await vi.waitFor(() => {
      expect(MockWebSocket.instances).toHaveLength(1);
    });

    const ws = MockWebSocket.instances[0];

    const event = {
      event_type: "workflow_updated",
      workflow_id: "uuid-1",
      ticket_key: "TEST-1",
      state: "Done",
    };

    ws.simulateMessage(JSON.stringify(event));

    expect(handler).toHaveBeenCalledWith(event);
  });

  it("does not crash on malformed JSON", async () => {
    const handler = vi.fn();
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    renderHook(() => useWebSocket(handler));

    await vi.waitFor(() => {
      expect(MockWebSocket.instances).toHaveLength(1);
    });

    const ws = MockWebSocket.instances[0];

    // Should not throw
    ws.simulateMessage("not valid json {{{");

    expect(handler).not.toHaveBeenCalled();
    expect(errorSpy).toHaveBeenCalledWith("Failed to parse WS message");
    errorSpy.mockRestore();
  });

  it("closes websocket on unmount", async () => {
    const handler = vi.fn();
    const { unmount } = renderHook(() => useWebSocket(handler));

    await vi.waitFor(() => {
      expect(MockWebSocket.instances).toHaveLength(1);
    });

    const ws = MockWebSocket.instances[0];
    unmount();

    expect(ws.closed).toBe(true);
  });
});
