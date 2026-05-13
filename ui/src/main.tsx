// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import { App } from "./App";
import "./styles/index.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <BrowserRouter basename={import.meta.env.DEV ? (window.location.pathname.match(/^\/s\/[a-f0-9]+\//)?.[0] || "/") : "/"}>
      <App />
    </BrowserRouter>
  </StrictMode>
);
