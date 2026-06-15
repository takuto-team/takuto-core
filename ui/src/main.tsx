// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import { QueryClientProvider } from "@tanstack/react-query";
import { App } from "./App";
import { queryClient } from "./api/queryClient";
import { ErrorBoundary } from "./components/ErrorBoundary";
import "@fontsource-variable/geist/index.css";
import "@fontsource-variable/geist-mono/index.css";
import "./styles/index.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <ErrorBoundary>
        <BrowserRouter basename={import.meta.env.DEV ? (window.location.pathname.match(/^\/s\/[a-f0-9]{32}\//)?.[0] || "/") : "/"}>
          <App />
        </BrowserRouter>
      </ErrorBoundary>
    </QueryClientProvider>
  </StrictMode>
);
