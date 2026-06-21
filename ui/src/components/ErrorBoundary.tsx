// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/**
 * Top-level error boundary. Catches render-time exceptions anywhere in the
 * tree and shows a recoverable fallback instead of a blank white screen — the
 * last line of defence behind the per-fetch toast surface (`QueryErrorToaster`).
 */

import { Component, type ErrorInfo, type ReactNode } from "react";
import { withTranslation, type WithTranslation } from "react-i18next";

interface Props extends WithTranslation {
  children: ReactNode;
}

interface State {
  error: Error | null;
}

class ErrorBoundaryInner extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    // Surface to the browser console for diagnostics; the fallback UI is what
    // the user sees. (A console.error here is an intentional diagnostic, not a
    // stray debug log.)
    console.error("Unhandled UI error:", error, info.componentStack);
  }

  handleReload = (): void => {
    window.location.reload();
  };

  render(): ReactNode {
    const { t } = this.props;
    if (this.state.error) {
      return (
        <div className="flex flex-col items-center justify-center min-h-screen gap-4 px-6 text-center">
          <h1 className="text-lg font-semibold text-red-300">{t("boundary.title")}</h1>
          <p className="text-sm text-gray-400 max-w-md">{t("boundary.body")}</p>
          <button
            type="button"
            onClick={this.handleReload}
            className="text-sm px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-500 cursor-pointer"
          >
            {t("boundary.reload")}
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}

export const ErrorBoundary = withTranslation("errors")(ErrorBoundaryInner);
