import { describe, it, expect, vi } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { ToastProvider, useToast } from "./useToast";
import type { ReactNode } from "react";

function wrapper({ children }: { children: ReactNode }) {
  return <ToastProvider>{children}</ToastProvider>;
}

describe("useToast", () => {
  it("throws when used outside ToastProvider", () => {
    // Suppress console.error for expected error
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    expect(() => {
      renderHook(() => useToast());
    }).toThrow("useToast must be used within ToastProvider");
    spy.mockRestore();
  });

  it("showToast() adds a toast to the list", () => {
    const { result } = renderHook(() => useToast(), { wrapper });

    act(() => {
      result.current.showToast("Something went wrong", "error");
    });

    expect(result.current.toasts).toHaveLength(1);
    expect(result.current.toasts[0].message).toBe("Something went wrong");
    expect(result.current.toasts[0].type).toBe("error");
  });

  it("dismissToast() removes the toast by id", () => {
    const { result } = renderHook(() => useToast(), { wrapper });

    act(() => {
      result.current.showToast("Toast 1", "info");
    });

    const id = result.current.toasts[0].id;

    act(() => {
      result.current.dismissToast(id);
    });

    expect(result.current.toasts).toHaveLength(0);
  });

  it("multiple toasts stack correctly", () => {
    const { result } = renderHook(() => useToast(), { wrapper });

    act(() => {
      result.current.showToast("First", "error");
      result.current.showToast("Second", "success");
      result.current.showToast("Third", "info");
    });

    expect(result.current.toasts).toHaveLength(3);
    expect(result.current.toasts[0].message).toBe("First");
    expect(result.current.toasts[1].message).toBe("Second");
    expect(result.current.toasts[2].message).toBe("Third");
  });

  it("dismissing one toast leaves others intact", () => {
    const { result } = renderHook(() => useToast(), { wrapper });

    act(() => {
      result.current.showToast("A", "error");
      result.current.showToast("B", "success");
    });

    const idA = result.current.toasts[0].id;

    act(() => {
      result.current.dismissToast(idA);
    });

    expect(result.current.toasts).toHaveLength(1);
    expect(result.current.toasts[0].message).toBe("B");
  });

  it("defaults to error type when none specified", () => {
    const { result } = renderHook(() => useToast(), { wrapper });

    act(() => {
      result.current.showToast("default type");
    });

    expect(result.current.toasts[0].type).toBe("error");
  });
});
