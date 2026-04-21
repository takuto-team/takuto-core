import { createContext, useContext, useState, useCallback, type ReactNode } from "react";

export interface Toast {
  id: number;
  message: string;
  type: "error" | "success" | "info";
}

interface ToastContextType {
  toasts: Toast[];
  showToast: (message: string, type?: "error" | "success" | "info") => void;
  dismissToast: (id: number) => void;
}

const ToastContext = createContext<ToastContextType | null>(null);

let toastId = 0;

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);

  const showToast = useCallback((message: string, type: "error" | "success" | "info" = "error") => {
    const id = ++toastId;
    setToasts((prev) => [...prev, { id, message, type }]);
    // Auto-dismiss after 8 seconds
    setTimeout(() => {
      setToasts((prev) => prev.filter((t) => t.id !== id));
    }, 8000);
  }, []);

  const dismissToast = useCallback((id: number) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  return (
    <ToastContext.Provider value={{ toasts, showToast, dismissToast }}>
      {children}
    </ToastContext.Provider>
  );
}

export function useToast() {
  const ctx = useContext(ToastContext);
  if (!ctx) throw new Error("useToast must be used within ToastProvider");
  return ctx;
}

export function ToastContainer() {
  const { toasts, dismissToast } = useToast();

  if (toasts.length === 0) return null;

  const colors = {
    error: { bg: "bg-red-950/90", border: "border-red-700/50", text: "text-red-300", icon: "text-red-400" },
    success: { bg: "bg-green-950/90", border: "border-green-700/50", text: "text-green-300", icon: "text-green-400" },
    info: { bg: "bg-blue-950/90", border: "border-blue-700/50", text: "text-blue-300", icon: "text-blue-400" },
  };

  return (
    <div className="fixed bottom-4 right-4 z-50 flex flex-col gap-2 max-w-lg">
      {toasts.map((toast) => {
        const c = colors[toast.type];
        return (
          <div
            key={toast.id}
            className={`${c.bg} border ${c.border} rounded-xl p-4 shadow-lg backdrop-blur-sm animate-slide-in`}
          >
            <div className="flex items-start justify-between gap-3">
              <p className={`text-sm ${c.text} whitespace-pre-wrap break-words`}>
                {toast.message}
              </p>
              <button
                onClick={() => dismissToast(toast.id)}
                className={`${c.icon} opacity-60 hover:opacity-100 flex-shrink-0 cursor-pointer`}
              >
                &times;
              </button>
            </div>
          </div>
        );
      })}
    </div>
  );
}
