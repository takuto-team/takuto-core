// Maestro Dashboard Application

let workflows = {};
/** @type {string[]} Stable grid order (ticket keys). New workflows append at end; pause/stop/refetch must not reshuffle. */
let workflowOrderKeys = [];
let terminalState = {}; // { [ticket_key]: { stepName: string, lines: OutputLine[], completed: bool } }
let ws = null;
let wsReconnectTimer = null;
let dryMode = false;
let initialLoadDone = false;
let pollingPaused = false;
/** `0` = unlimited. From `[general] max_concurrent_manual_workflows`. */
let maxConcurrentManual = 0;
let jiraProjectsConfigured = false;
/** `[jira] site` from config (fallback if workflow payload omits `jira_browse_url`). */
let jiraSite = '';
/** `true` when acli (Jira) is authenticated — from `GET /api/config`. */
let jiraAvailable = true;
/** Set when the ticket detail modal is open; used by **Start** to run the workflow. */
let pendingManualTicketSelection = null;
/** Bumps on each detail open so slower Jira preview responses cannot overwrite a newer selection. */
let manualTicketPreviewSeq = 0;
const TERMINAL_MAX_LINES = 500;

/** Same-origin cookie session from `POST /api/auth/login`; redirect to sign-in on 401. */
async function dashboardFetch(input, init = {}) {
  const headers = new Headers(init.headers ?? undefined);
  const res = await fetch(input, { ...init, credentials: 'same-origin', headers });
  if (res.status === 401) {
    const ret = encodeURIComponent(location.pathname + location.search);
    window.location.href = '/login.html?return=' + ret;
  }
  return res;
}

// --- WebSocket ---

function connectWebSocket() {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  // Session cookie is sent automatically on same-origin WebSocket upgrade.
  ws = new WebSocket(`${proto}//${location.host}/ws`);

  ws.onopen = () => {
    updateWsStatus(true);
    if (wsReconnectTimer) {
      clearTimeout(wsReconnectTimer);
      wsReconnectTimer = null;
    }
    // Only fetch on reconnect, not initial connect
    if (initialLoadDone) {
      fetchWorkflowsSilent();
    }
  };

  ws.onclose = () => {
    updateWsStatus(false);
    scheduleReconnect();
  };

  ws.onerror = () => {
    updateWsStatus(false);
  };

  ws.onmessage = (event) => {
    try {
      const evt = JSON.parse(event.data);
      handleWorkflowEvent(evt);
    } catch (e) {
      console.error('Failed to parse WS message:', e);
    }
  };
}

function scheduleReconnect() {
  if (!wsReconnectTimer) {
    wsReconnectTimer = setTimeout(() => {
      wsReconnectTimer = null;
      connectWebSocket();
    }, 3000);
  }
}

function updateWsStatus(connected) {
  const el = document.getElementById('wsStatus');
  if (!el) return;
  if (connected) {
    el.innerHTML = '<span class="inline-block w-2 h-2 bg-green-500 rounded-full animate-pulse-dot"></span> Connected';
  } else {
    el.innerHTML = '<span class="inline-block w-2 h-2 bg-gray-600 rounded-full"></span> Disconnected';
  }
}

function handleWorkflowEvent(evt) {
  const eventType = evt.event_type;

  if (eventType === 'workflow_removed') {
    delete workflows[evt.ticket_key];
    delete terminalState[evt.ticket_key];
    workflowOrderKeys = workflowOrderKeys.filter(k => k !== evt.ticket_key);
    renderWorkflows();
    updateCounts(Object.values(workflows));
    return;
  }

  // Dynamic port forwarding events
  if (eventType === 'port_forwarded') {
    handlePortForwarded(evt);
    return;
  }
  if (eventType === 'port_unforwarded') {
    handlePortUnforwarded(evt);
    return;
  }

  // Handle terminal-related events — DOM-only updates, no full re-render
  if (eventType === 'step_output') {
    handleStepOutput(evt);
    return;
  }
  if (eventType === 'step_started') {
    handleStepStarted(evt);
    return;
  }
  if (eventType === 'step_completed') {
    handleStepCompleted(evt);
    return;
  }

  // Workflow state events — update local state
  const wf = Object.values(workflows).find(w => w.ticket_key === evt.ticket_key);
  if (wf) {
    wf.state = evt.state;
    if (typeof evt.progress_percent === 'number' && Number.isFinite(evt.progress_percent)) {
      wf.progress_percent = evt.progress_percent;
    }
    if (typeof evt.progress_steps_total === 'number' && Number.isFinite(evt.progress_steps_total)) {
      wf.progress_steps_total = Math.max(0, Math.floor(evt.progress_steps_total));
    }
    if (evt.error) {
      wf.error = evt.error;
    }
    // Terminal states: re-fetch full workflow to get pr_url and action flags
    // (WebSocket events don't carry pr_url or can_* fields)
    const terminalStates = ['done', 'error', 'stopped'];
    if (terminalStates.includes(wf.state.toLowerCase())) {
      fetchWorkflowsSilent();
      return;
    }
    // Update just this card, not the entire grid
    updateCardState(wf);
    updateCounts(Object.values(workflows));
  } else if (evt.ticket_key) {
    // New workflow — fetch once to get full data
    fetchWorkflowsSilent();
  }
}

function ensureTerminalState(ticketKey) {
  if (!terminalState[ticketKey]) {
    terminalState[ticketKey] = { stepName: 'Waiting...', lines: [], completed: false };
  }
  return terminalState[ticketKey];
}

function handleStepStarted(evt) {
  const ts = ensureTerminalState(evt.ticket_key);
  ts.stepName = evt.step_name;
  ts.lines = [];
  ts.completed = false;

  const headerEl = document.getElementById(`terminal-step-${evt.ticket_key}`);
  if (headerEl) {
    headerEl.textContent = `$ ${evt.step_name}`;
    headerEl.closest('.terminal-header').classList.remove('completed');
  }

  const bodyEl = document.getElementById(`terminal-body-${evt.ticket_key}`);
  if (bodyEl) {
    bodyEl.innerHTML = '';
  }

  // Update local workflow state so subsequent updateCardState calls don't overwrite with a stale label.
  const wf = workflows[evt.ticket_key];
  if (wf) {
    wf.state = evt.step_name;
  }

  // Also update the current step display on the card
  const stepEl = document.getElementById(`step-display-${evt.ticket_key}`);
  if (stepEl) {
    stepEl.textContent = wf ? formatStepLineWithProgress(wf, evt.step_name) : evt.step_name;
  }
}

function handleStepOutput(evt) {
  const text = evt.output_line || '';

  // Skip empty lines
  if (!text) return;

  const ts = ensureTerminalState(evt.ticket_key);
  ts.lines.push({ text, stream: evt.stream });

  if (ts.lines.length > TERMINAL_MAX_LINES) {
    ts.lines.shift();
    const bodyEl = document.getElementById(`terminal-body-${evt.ticket_key}`);
    if (bodyEl && bodyEl.firstChild) {
      bodyEl.removeChild(bodyEl.firstChild);
    }
  }

  const bodyEl = document.getElementById(`terminal-body-${evt.ticket_key}`);
  if (bodyEl) {
    const lineEl = document.createElement('div');
    lineEl.textContent = text;
    const isWarn = /\bwarn(ing)?\b/i.test(text) || /\bWARN\b/.test(text);
    if (isWarn) {
      lineEl.className = 'terminal-line-warn';
    } else if (evt.stream === 'stderr') {
      lineEl.className = 'terminal-line-stderr';
    }
    bodyEl.appendChild(lineEl);
    bodyEl.scrollTop = bodyEl.scrollHeight;
  }
}

function handleStepCompleted(evt) {
  const ts = ensureTerminalState(evt.ticket_key);
  ts.completed = true;

  const headerEl = document.getElementById(`terminal-step-${evt.ticket_key}`);
  if (headerEl) {
    headerEl.textContent = `${evt.step_name} -- completed`;
    headerEl.closest('.terminal-header').classList.add('completed');
  }

  const wf = workflows[evt.ticket_key];
  if (wf) {
    if (typeof evt.progress_percent === 'number' && Number.isFinite(evt.progress_percent)) {
      wf.progress_percent = evt.progress_percent;
    }
    if (typeof evt.progress_steps_total === 'number' && Number.isFinite(evt.progress_steps_total)) {
      wf.progress_steps_total = Math.max(0, Math.floor(evt.progress_steps_total));
    }
    updateCardState(wf);
  }
}

// Client-side tracking of dynamic port forwards (from WebSocket events).
const dynamicForwards = {}; // ticket_key → [[containerPort, hostPort], ...]

function handlePortForwarded(evt) {
  if (!evt.forwarded_port) return;
  const [containerPort, hostPort] = evt.forwarded_port;
  const key = evt.ticket_key;
  if (!dynamicForwards[key]) dynamicForwards[key] = [];
  dynamicForwards[key].push([containerPort, hostPort]);
  showToast(`Port ${containerPort} forwarded`, `<a href="http://localhost:${hostPort}" target="_blank" rel="noopener" class="text-teal-300 underline">localhost:${hostPort}</a>`, key);
  // Open the forwarded port in a new tab.
  window.open(`http://localhost:${hostPort}`, '_blank', 'noopener,noreferrer');
  // Update the port mappings display on the card.
  updateDynamicPortsDisplay(key);
}

function handlePortUnforwarded(evt) {
  if (!evt.forwarded_port) return;
  const [containerPort] = evt.forwarded_port;
  const key = evt.ticket_key;
  if (dynamicForwards[key]) {
    dynamicForwards[key] = dynamicForwards[key].filter(([cp]) => cp !== containerPort);
  }
  showToast(`Port ${containerPort} removed`, 'Forward stopped', key);
  updateDynamicPortsDisplay(key);
}

function updateDynamicPortsDisplay(ticketKey) {
  const container = document.getElementById(`port-mappings-${ticketKey}`);
  if (!container) return;
  const forwards = dynamicForwards[ticketKey] || [];
  // Rebuild dynamic portion — keep any existing configured-port spans, replace dynamic ones.
  // Simplest: re-render the entire container content from current state.
  let html = '';
  // Configured port mappings (from workflow data).
  const wf = workflows[ticketKey];
  if (wf && wf.editor_port_mappings && wf.editor_port_mappings.length > 0) {
    html += wf.editor_port_mappings.map(([cp, hp]) =>
      `<span class="text-xs text-gray-500">${cp} &#x2192; <a href="http://localhost:${hp}" target="_blank" rel="noopener" class="text-violet-400 hover:text-violet-300">localhost:${hp}</a></span>`
    ).join(' ');
  }
  // Dynamic forwards.
  if (forwards.length > 0) {
    html += forwards.map(([cp, hp]) =>
      `<span class="text-xs text-gray-500">${cp} &#x2192; <a href="http://localhost:${hp}" target="_blank" rel="noopener" class="text-teal-400 hover:text-teal-300">localhost:${hp}</a></span>`
    ).join(' ');
  }
  container.innerHTML = html;
  container.hidden = !html;
}

// Simple toast notification — auto-dismisses after 5 seconds.
function showToast(title, bodyHtml, ticketKey) {
  let container = document.getElementById('toast-container');
  if (!container) {
    container = document.createElement('div');
    container.id = 'toast-container';
    container.className = 'fixed bottom-4 right-4 z-50 flex flex-col gap-2';
    document.body.appendChild(container);
  }
  const toast = document.createElement('div');
  toast.className = 'toast-notification bg-gray-800 border border-gray-700 rounded-lg px-4 py-3 shadow-lg max-w-sm';
  toast.innerHTML = `<div class="text-sm font-medium text-gray-200">${escapeHtml(ticketKey)}: ${title}</div><div class="text-xs text-gray-400 mt-0.5">${bodyHtml}</div>`;
  container.appendChild(toast);
  setTimeout(() => {
    toast.style.opacity = '0';
    toast.style.transition = 'opacity 0.3s';
    setTimeout(() => toast.remove(), 300);
  }, 5000);
}

// Update a single card's state indicators without re-rendering the whole grid
function updateCardState(wf) {
  const card = document.getElementById(`card-${wf.ticket_key}`);
  if (!card) {
    // Card doesn't exist yet — need full render
    renderWorkflows();
    return;
  }

  const status = getStatusInfo(wf.state);

  // Update status badge
  const badgeEl = card.querySelector('.status-badge');
  if (badgeEl) badgeEl.innerHTML = statusBadgeHtml(status);

  // Update step display
  const stepEl = document.getElementById(`step-display-${wf.ticket_key}`);
  if (stepEl) stepEl.textContent = formatStepLineWithProgress(wf, wf.state);

  // Update progress bar (segmented or legacy fill)
  const progressSlot = card.querySelector('.workflow-progress-slot');
  if (progressSlot) {
    progressSlot.innerHTML = progressBarInnerHtml(wf, status);
  }

  // If workflow finished (completed/error/stopped), do a full render to update buttons and terminal visibility
  if (status.label === 'Running') {
    const term = document.getElementById(`terminal-body-${wf.ticket_key}`);
    if (!term) {
      renderWorkflows();
      return;
    }
  }
  if (['Completed', 'Error', 'Stopped'].includes(status.label)) {
    renderWorkflows();
  }
}

// --- API ---

/** Merge API list into `workflows` and update `workflowOrderKeys` without re-sorting existing cards. */
function ingestWorkflowList(list) {
  const next = {};
  for (const w of list) {
    next[w.ticket_key] = {
      ...w,
      started_manually: !!w.started_manually,
      counts_toward_manual_cap: !!w.counts_toward_manual_cap,
    };
  }
  const keysInApi = new Set(list.map(w => w.ticket_key));
  workflowOrderKeys = workflowOrderKeys.filter(k => keysInApi.has(k));
  const inOrder = new Set(workflowOrderKeys);
  for (const w of list) {
    if (!inOrder.has(w.ticket_key)) {
      workflowOrderKeys.push(w.ticket_key);
      inOrder.add(w.ticket_key);
    }
  }
  workflows = next;
}

// Silent fetch — doesn't cause a visual flash
async function fetchWorkflowsSilent() {
  try {
    const res = await dashboardFetch('/api/workflows');
    if (!res.ok) return;
    const list = await res.json();
    ingestWorkflowList(list);
    list.forEach(w => {
      // Populate terminal state from API data if not already set by WebSocket.
      // This ensures terminal output is visible on page load/reload.
      if (w.terminal_lines && w.terminal_lines.length > 0 && !terminalState[w.ticket_key]) {
        terminalState[w.ticket_key] = {
          stepName: w.state,
          lines: w.terminal_lines.map(l => ({ text: l.text, stream: l.stream })),
          completed: false,
        };
      }
    });
    renderWorkflows();
  } catch (e) {
    console.error('Failed to fetch workflows:', e);
  }
}

async function fetchPollingStatus() {
  try {
    const res = await dashboardFetch('/api/polling');
    if (!res.ok) return;
    const data = await res.json();
    pollingPaused = !!data.paused;
    applyPollingUi();
  } catch (_) {
    // non-fatal
  }
}

function applyPollingUi() {
  const btn = document.getElementById('pollingToggleBtn');
  const label = document.getElementById('pollingStatusLabel');
  if (!btn) return;
  // When Jira is not available, hide polling controls entirely.
  const pollingContainer = btn.closest('.border-l');
  if (!jiraAvailable) {
    if (pollingContainer) pollingContainer.style.display = 'none';
    return;
  }
  if (pollingContainer) pollingContainer.style.display = '';
  if (pollingPaused) {
    btn.textContent = 'Resume polling';
    btn.className =
      'dashboard-header-btn bg-amber-500/10 text-amber-300 border-amber-500/25 hover:bg-amber-500/20';
    if (label) {
      label.textContent = 'Jira poll: paused';
      label.className = 'text-xs text-amber-500/80 hidden sm:inline';
    }
  } else {
    btn.textContent = 'Pause polling';
    btn.className =
      'dashboard-header-btn bg-gray-800/80 text-gray-300 border-gray-700 hover:bg-gray-800 hover:border-gray-600';
    if (label) {
      label.textContent = 'Jira poll: active';
      label.className = 'text-xs text-emerald-500/80 hidden sm:inline';
    }
  }
}

async function togglePolling() {
  const btn = document.getElementById('pollingToggleBtn');
  if (btn) btn.disabled = true;
  try {
    const url = pollingPaused ? '/api/polling/resume' : '/api/polling/pause';
    const res = await dashboardFetch(url, { method: 'POST' });
    if (!res.ok) {
      const t = await res.text();
      alert(t || 'Failed to update polling');
      return;
    }
    const data = await res.json();
    pollingPaused = !!data.paused;
    applyPollingUi();
  } catch (e) {
    console.error(e);
    alert('Failed to update polling');
  } finally {
    if (btn) btn.disabled = false;
  }
}

async function fetchConfig() {
  try {
    const res = await dashboardFetch('/api/config');
    const cfg = await res.json();
    dryMode = cfg.general.dry_mode;
    maxConcurrentManual =
      typeof cfg.general.max_concurrent_manual_workflows === 'number' &&
      Number.isFinite(cfg.general.max_concurrent_manual_workflows)
        ? Math.max(0, Math.floor(cfg.general.max_concurrent_manual_workflows))
        : 0;
    jiraProjectsConfigured =
      Array.isArray(cfg.jira?.project_keys) && cfg.jira.project_keys.length > 0;
    jiraSite = typeof cfg.jira?.site === 'string' ? cfg.jira.site : '';
    jiraAvailable = cfg.jira_available !== false;
    const banner = document.getElementById('dryBanner');
    if (banner) {
      banner.classList.toggle('hidden', !dryMode);
    }
    const noJiraBanner = document.getElementById('noJiraBanner');
    if (noJiraBanner) {
      noJiraBanner.classList.toggle('hidden', jiraAvailable);
    }
    applyPollingUi();
  } catch (e) {
    // ignore
  }
}

function manualSlotsUsed() {
  return Object.values(workflows).filter(w => w.counts_toward_manual_cap).length;
}

function isAddWorkflowDisabled() {
  if (jiraAvailable && !jiraProjectsConfigured) return true;
  if (maxConcurrentManual > 0 && manualSlotsUsed() >= maxConcurrentManual) return true;
  return false;
}

function addWorkflowTitle() {
  if (!jiraAvailable) {
    if (maxConcurrentManual > 0 && manualSlotsUsed() >= maxConcurrentManual) {
      return `Manual workflow limit reached (${maxConcurrentManual})`;
    }
    return 'Paste a description to start a workflow';
  }
  if (!jiraProjectsConfigured) return 'Configure [jira] project_keys to enable manual starts';
  if (maxConcurrentManual > 0 && manualSlotsUsed() >= maxConcurrentManual) {
    return `Manual workflow limit reached (${maxConcurrentManual})`;
  }
  return 'Start workflow for a To Do ticket';
}

function renderAddWorkflowCell() {
  if (jiraAvailable && !jiraProjectsConfigured) return '';
  const dis = isAddWorkflowDisabled();
  return `
    <div class="workflow-add-cell">
      <button type="button"
        onclick="openManualWorkflowModal()"
        class="workflow-add-btn"
        ${dis ? 'disabled' : ''}
        title="${escapeAttr(addWorkflowTitle())}"
        aria-label="Start workflow for a To Do ticket">+</button>
    </div>`;
}

function setManualTicketDetailStartVisible(visible) {
  const btn = document.getElementById('manualTicketDetailStartBtn');
  if (!btn) return;
  if (visible) btn.classList.remove('hidden');
  else btn.classList.add('hidden');
}

function closeManualTicketDetailModal() {
  const modal = document.getElementById('manualTicketDetailModal');
  if (modal) modal.classList.add('hidden');
  pendingManualTicketSelection = null;
  setManualTicketDetailStartVisible(true);
}

/** Matches server `jira::ticket_browse_url` when `jira_browse_url` is missing from the API. */
function jiraBrowseUrlFallback(site, ticketKey) {
  let s = String(site || '').trim();
  if (!s) return `https://jira.atlassian.net/browse/${encodeURIComponent(ticketKey)}`;
  if (s.startsWith('https://')) s = s.slice(8);
  else if (s.startsWith('http://')) s = s.slice(7);
  s = s.trim().replace(/\/$/, '');
  if (!s) return `https://jira.atlassian.net/browse/${encodeURIComponent(ticketKey)}`;
  return `https://${s}/browse/${encodeURIComponent(ticketKey)}`;
}

function workflowJiraBrowseUrl(w) {
  const u = w.jira_browse_url;
  if (typeof u === 'string' && u.trim()) return u.trim();
  return jiraBrowseUrlFallback(jiraSite, w.ticket_key);
}

/**
 * @param {string} ticketKey
 * @param {string} summaryHint
 * @param {number} seq
 * @param {boolean} syncPending — update `pendingManualTicketSelection.summary` from Jira when set
 */
async function runTicketDescriptionPreviewLoad(ticketKey, summaryHint, seq, syncPending) {
  const body = document.getElementById('manualTicketDetailBody');
  const summaryEl = document.getElementById('manualTicketDetailSummary');
  if (!body || !summaryEl) return;

  try {
    const res = await dashboardFetch(
      `/api/jira/tickets/${encodeURIComponent(ticketKey)}/preview`
    );
    const text = await res.text();
    if (seq !== manualTicketPreviewSeq) return;
    if (!res.ok) {
      body.innerHTML = `<p class="text-sm text-red-400 px-5 py-6">${escapeHtml(text || res.statusText || 'Failed to load ticket')}</p>`;
      return;
    }
    let data;
    try {
      data = JSON.parse(text);
    } catch {
      if (seq !== manualTicketPreviewSeq) return;
      body.innerHTML = '<p class="text-sm text-red-400 px-5 py-6">Invalid response from server</p>';
      return;
    }
    if (seq !== manualTicketPreviewSeq) return;
    const hint = typeof summaryHint === 'string' && summaryHint.trim() ? summaryHint.trim() : ticketKey;
    const sum = typeof data.summary === 'string' ? data.summary : hint;
    summaryEl.textContent = sum || ticketKey;
    if (
      syncPending &&
      pendingManualTicketSelection &&
      pendingManualTicketSelection.key === ticketKey
    ) {
      pendingManualTicketSelection.summary = sum || ticketKey;
    }

    const md = typeof data.description_markdown === 'string' ? data.description_markdown : '';
    const html = renderMarkdownToSafeHtml(md);
    if (!html) {
      body.innerHTML = '<p class="text-gray-500 italic px-5 py-6">No description</p>';
    } else {
      body.innerHTML = `<div class="manual-ticket-detail-prose px-5 py-4">${html}</div>`;
    }
  } catch (e) {
    if (seq !== manualTicketPreviewSeq) return;
    console.error(e);
    body.innerHTML = '<p class="text-sm text-red-400 px-5 py-6">Could not load ticket</p>';
  }
}

function closeManualWorkflowModal() {
  closeManualTicketDetailModal();
  const modal = document.getElementById('manualWorkflowModal');
  if (modal) modal.classList.add('hidden');
}

function renderMarkdownToSafeHtml(markdown) {
  const md = markdown == null ? '' : String(markdown);
  if (!md.trim()) {
    return '';
  }
  try {
    const parseFn =
      typeof marked !== 'undefined' && marked && typeof marked.parse === 'function'
        ? marked.parse.bind(marked)
        : null;
    const purifyFn =
      typeof DOMPurify !== 'undefined' && DOMPurify && typeof DOMPurify.sanitize === 'function'
        ? DOMPurify.sanitize.bind(DOMPurify)
        : null;
    if (!parseFn || !purifyFn) {
      return `<pre class="manual-ticket-detail-fallback px-5 py-4">${escapeHtml(md)}</pre>`;
    }
    const raw = parseFn(md, { breaks: true });
    return purifyFn(raw, { USE_PROFILES: { html: true } });
  } catch (e) {
    console.error('Markdown render failed', e);
    return `<pre class="manual-ticket-detail-fallback px-5 py-4">${escapeHtml(md)}</pre>`;
  }
}

async function openManualTicketDetailModal(ticketKey, listSummary) {
  const modal = document.getElementById('manualTicketDetailModal');
  const body = document.getElementById('manualTicketDetailBody');
  const keyEl = document.getElementById('manualTicketDetailKey');
  const summaryEl = document.getElementById('manualTicketDetailSummary');
  if (!modal || !body || !keyEl || !summaryEl) {
    console.error(
      '[Maestro] Ticket detail modal elements missing (is index.html outdated?). Expected #manualTicketDetailModal, #manualTicketDetailBody, #manualTicketDetailKey, #manualTicketDetailSummary.'
    );
    return;
  }

  setManualTicketDetailStartVisible(true);
  const seq = ++manualTicketPreviewSeq;
  pendingManualTicketSelection = {
    key: ticketKey,
    summary: typeof listSummary === 'string' && listSummary.trim() ? listSummary.trim() : ticketKey,
  };
  keyEl.textContent = ticketKey;
  summaryEl.textContent = pendingManualTicketSelection.summary;
  body.innerHTML = `
    <div class="manual-workflow-loading px-5 py-8">
      <div class="manual-workflow-spinner" role="status" aria-label="Loading"></div>
      <span>Loading description…</span>
    </div>`;
  modal.classList.remove('hidden');

  await runTicketDescriptionPreviewLoad(
    ticketKey,
    pendingManualTicketSelection.summary,
    seq,
    true
  );
}

/** Workflow card: same description modal as manual start, without **Start**. */
async function openWorkflowTicketDescriptionModal(ticketKey, listSummary) {
  const modal = document.getElementById('manualTicketDetailModal');
  const body = document.getElementById('manualTicketDetailBody');
  const keyEl = document.getElementById('manualTicketDetailKey');
  const summaryEl = document.getElementById('manualTicketDetailSummary');
  if (!modal || !body || !keyEl || !summaryEl) {
    console.error('[Maestro] Ticket detail modal elements missing.');
    return;
  }

  pendingManualTicketSelection = null;
  setManualTicketDetailStartVisible(false);

  // For non-Jira workflows, show the description from in-memory workflow data
  // instead of fetching from the Jira preview endpoint.
  const wf = workflows[ticketKey];
  if (wf && wf.jira_available === false && wf.ticket_description) {
    const sumHint =
      typeof listSummary === 'string' && listSummary.trim() ? listSummary.trim() : ticketKey;
    keyEl.textContent = ticketKey;
    summaryEl.textContent = sumHint;
    const rendered = typeof marked !== 'undefined'
      ? DOMPurify.sanitize(marked.parse(wf.ticket_description))
      : `<pre class="whitespace-pre-wrap text-sm text-gray-300 px-5 py-4">${escapeHtml(wf.ticket_description)}</pre>`;
    body.innerHTML = `<div class="ticket-detail-prose px-5 py-4">${rendered}</div>`;
    modal.classList.remove('hidden');
    return;
  }

  const seq = ++manualTicketPreviewSeq;
  const sumHint =
    typeof listSummary === 'string' && listSummary.trim() ? listSummary.trim() : ticketKey;
  keyEl.textContent = ticketKey;
  summaryEl.textContent = sumHint;
  body.innerHTML = `
    <div class="manual-workflow-loading px-5 py-8">
      <div class="manual-workflow-spinner" role="status" aria-label="Loading"></div>
      <span>Loading description…</span>
    </div>`;
  modal.classList.remove('hidden');

  await runTicketDescriptionPreviewLoad(ticketKey, sumHint, seq, false);
}

async function confirmManualWorkflowStart() {
  const p = pendingManualTicketSelection;
  if (!p) return;
  const ticketKey = p.key;
  const ticketSummary = p.summary;
  closeManualWorkflowModal();
  await startManualWorkflowRequest(ticketKey, ticketSummary);
}

async function startManualWorkflowRequest(ticketKey, ticketSummary) {
  try {
    const res = await dashboardFetch('/api/workflows/start-manual', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ ticket_key: ticketKey, ticket_summary: ticketSummary }),
    });
    if (!res.ok) {
      const errText = await res.text();
      alert(errText || 'Failed to start workflow');
      return;
    }
    await fetchWorkflowsSilent();
  } catch (e) {
    console.error(e);
    alert('Failed to start workflow');
  }
}

async function openManualWorkflowModal() {
  if (isAddWorkflowDisabled()) return;
  if (!jiraAvailable) {
    openPasteDescriptionModal();
    return;
  }
  const modal = document.getElementById('manualWorkflowModal');
  const body = document.getElementById('manualWorkflowModalBody');
  if (!modal || !body) return;
  modal.classList.remove('hidden');
  body.innerHTML = `
    <div class="manual-workflow-loading">
      <div class="manual-workflow-spinner" role="status" aria-label="Loading"></div>
      <span>Loading tickets from Jira…</span>
    </div>`;
  try {
    const res = await dashboardFetch('/api/jira/todo-tickets-manual');
    const text = await res.text();
    if (!res.ok) {
      body.innerHTML = `<p class="text-sm text-red-400 px-2 py-6 text-center">${escapeHtml(text || res.statusText || 'Failed to load tickets')}</p>`;
      return;
    }
    let tickets;
    try {
      tickets = JSON.parse(text);
    } catch {
      body.innerHTML = '<p class="text-sm text-red-400 px-2 py-6 text-center">Invalid response from server</p>';
      return;
    }
    const existingKeys = new Set(Object.keys(workflows));
    const available = Array.isArray(tickets)
      ? tickets.filter(t => typeof t.key === 'string' && t.key && !existingKeys.has(t.key))
      : [];
    if (available.length === 0) {
      body.innerHTML =
        '<p class="text-sm text-gray-500 px-2 py-8 text-center">No available To Do tickets (all listed tickets already have a workflow on the dashboard, or Jira returned none).</p>';
      return;
    }
    body.innerHTML = '';

    // Build type filter bar if more than one issue type
    const types = [...new Set(available.map(t => t.item_type || '').filter(Boolean))];
    if (types.length > 1) {
      const filterBar = document.createElement('div');
      filterBar.className = 'flex gap-2 flex-wrap px-1 pb-3';
      const makeBtn = (label, active) => {
        const btn = document.createElement('button');
        btn.type = 'button';
        btn.textContent = label;
        btn.dataset.typeFilter = label;
        btn.className = active
          ? 'text-xs px-3 py-1 rounded-full border bg-blue-500/20 text-blue-300 border-blue-500/40'
          : 'text-xs px-3 py-1 rounded-full border bg-gray-800/50 text-gray-400 border-gray-700 hover:bg-gray-700/50';
        btn.onclick = () => {
          const isAll = label === 'All';
          filterBar.querySelectorAll('button').forEach(b => {
            const on = b.dataset.typeFilter === label;
            b.className = on
              ? 'text-xs px-3 py-1 rounded-full border bg-blue-500/20 text-blue-300 border-blue-500/40'
              : 'text-xs px-3 py-1 rounded-full border bg-gray-800/50 text-gray-400 border-gray-700 hover:bg-gray-700/50';
          });
          listEl.querySelectorAll('.manual-workflow-row').forEach(row => {
            row.style.display = isAll || row.dataset.ticketType === label ? '' : 'none';
          });
        };
        return btn;
      };
      filterBar.appendChild(makeBtn('All', true));
      types.forEach(t => filterBar.appendChild(makeBtn(t, false)));
      body.appendChild(filterBar);
    }

    const listEl = document.createElement('div');
    listEl.className = 'manual-workflow-list';
    for (const t of available) {
      const key = t.key;
      const summary = typeof t.summary === 'string' ? t.summary : '';
      const row = document.createElement('button');
      row.type = 'button';
      row.className = 'manual-workflow-row';
      row.dataset.ticketKey = key;
      row.dataset.ticketSummary = summary || key;
      row.dataset.ticketType = t.item_type || '';
      const kEl = document.createElement('div');
      kEl.className = 'manual-workflow-row-key';
      kEl.textContent = key;
      const sEl = document.createElement('div');
      sEl.className = 'manual-workflow-row-summary';
      sEl.textContent = summary || key;
      row.appendChild(kEl);
      row.appendChild(sEl);
      listEl.appendChild(row);
    }
    body.appendChild(listEl);
  } catch (e) {
    console.error(e);
    body.innerHTML = '<p class="text-sm text-red-400 px-2 py-6 text-center">Could not load tickets</p>';
  }
}

async function pauseWorkflow(id) {
  try {
    await dashboardFetch(`/api/workflows/${encodeURIComponent(id)}/pause`, { method: 'POST' });
    // State update comes via WebSocket, no need to refetch
  } catch (e) {
    console.error('Failed to pause workflow:', e);
  }
}

async function resumeWorkflow(id) {
  try {
    await dashboardFetch(`/api/workflows/${encodeURIComponent(id)}/resume`, { method: 'POST' });
  } catch (e) {
    console.error('Failed to resume workflow:', e);
  }
}

async function retryWorkflow(id) {
  try {
    await dashboardFetch(`/api/workflows/${encodeURIComponent(id)}/retry`, { method: 'POST' });
    // Clear terminal state for this workflow
    delete terminalState[id];
    // Fetch fresh state since the workflow was replaced
    fetchWorkflowsSilent();
  } catch (e) {
    console.error('Failed to retry workflow:', e);
  }
}

async function resumeFromError(id) {
  try {
    await dashboardFetch(`/api/workflows/${encodeURIComponent(id)}/resume-from-error`, { method: 'POST' });
    delete terminalState[id];
    fetchWorkflowsSilent();
  } catch (e) {
    console.error('Failed to resume workflow from error:', e);
  }
}

async function stopWorkflow(id) {
  const ok = await showDashboardConfirm({
    title: 'Stop workflow',
    message:
      'Are you sure you want to stop this workflow? The ticket will be unassigned and moved back to To Do.',
    confirmLabel: 'Stop',
    danger: true,
  });
  if (!ok) return;
  try {
    await dashboardFetch(`/api/workflows/${encodeURIComponent(id)}/stop`, { method: 'POST' });
  } catch (e) {
    console.error('Failed to stop workflow:', e);
  }
}

async function addressPrComments(id) {
  try {
    const res = await dashboardFetch(`/api/workflows/${encodeURIComponent(id)}/address-pr-comments`, { method: 'POST' });
    if (!res.ok) {
      const t = await res.text();
      alert(t || 'Failed to start PR review');
      return;
    }
    delete terminalState[id];
    fetchWorkflowsSilent();
  } catch (e) {
    console.error('Failed to start PR review:', e);
    alert('Failed to start PR review');
  }
}

async function mergeBaseBranch(id) {
  try {
    const res = await dashboardFetch(`/api/workflows/${encodeURIComponent(id)}/merge-base-branch`, { method: 'POST' });
    if (!res.ok) {
      const t = await res.text();
      alert(t || 'Failed to start merge base branch');
      return;
    }
    delete terminalState[id];
    fetchWorkflowsSilent();
  } catch (e) {
    console.error('Failed to start merge base branch:', e);
    alert('Failed to start merge base branch');
  }
}

async function deleteWorkflow(id) {
  const ok = await showDashboardConfirm({
    title: 'Delete workflow',
    message:
      'Remove this workflow from the dashboard? The Jira ticket will not be updated. The local worktree and branch will be removed if they still exist.',
    confirmLabel: 'Delete',
    danger: true,
  });
  if (!ok) return;
  setWorkflowCardLoading(id, true, 'Deleting…');
  try {
    const res = await dashboardFetch(`/api/workflows/${encodeURIComponent(id)}/delete`, { method: 'POST' });
    if (!res.ok) {
      const t = await res.text();
      alert(t || 'Delete failed');
      return;
    }
    delete terminalState[id];
    fetchWorkflowsSilent();
  } catch (e) {
    console.error('Failed to delete workflow:', e);
    alert('Failed to delete workflow');
  } finally {
    setWorkflowCardLoading(id, false);
  }
}

async function openEditor(id) {
  setWorkflowCardLoading(id, true, 'Starting editor…');
  try {
    const res = await dashboardFetch(`/api/workflows/${encodeURIComponent(id)}/open-editor`, { method: 'POST' });
    if (!res.ok) {
      const t = await res.text();
      alert(t || 'Failed to start editor');
      return;
    }
    const data = await res.json();
    if (data.url) window.open(data.url, '_blank');
    fetchWorkflowsSilent();
  } catch (e) {
    console.error('Failed to open editor:', e);
    alert('Failed to open editor');
  } finally {
    setWorkflowCardLoading(id, false);
  }
}

async function openTerminal(id) {
  setWorkflowCardLoading(id, true, 'Starting terminal…');
  try {
    // Ensure editor container is running first (terminal lives inside it).
    let res = await dashboardFetch(`/api/workflows/${encodeURIComponent(id)}/open-terminal`, { method: 'POST' });
    if (res.status === 409) {
      // Editor not running — start it silently, then retry terminal.
      const editorRes = await dashboardFetch(`/api/workflows/${encodeURIComponent(id)}/open-editor`, { method: 'POST' });
      if (!editorRes.ok) {
        const t = await editorRes.text();
        alert(t || 'Failed to start editor container');
        return;
      }
      res = await dashboardFetch(`/api/workflows/${encodeURIComponent(id)}/open-terminal`, { method: 'POST' });
    }
    if (!res.ok) {
      const t = await res.text();
      alert(t || 'Failed to start terminal');
      return;
    }
    const data = await res.json();
    if (data.url) window.open(data.url, '_blank');
    fetchWorkflowsSilent();
  } catch (e) {
    console.error('Failed to open terminal:', e);
    alert('Failed to open terminal');
  } finally {
    setWorkflowCardLoading(id, false);
  }
}

async function closeEditor(id) {
  try {
    await dashboardFetch(`/api/workflows/${encodeURIComponent(id)}/close-editor`, { method: 'POST' });
    fetchWorkflowsSilent();
  } catch (e) {
    console.error('Failed to close editor:', e);
  }
}

async function markWorkflowDone(id) {
  const ok = await showDashboardConfirm({
    title: 'Mark as Done',
    message:
      'Mark this ticket Done in Jira and remove the local worktree? The workflow will leave the dashboard only if both steps succeed.',
    confirmLabel: 'Mark as Done',
    danger: false,
  });
  if (!ok) return;
  setWorkflowCardLoading(id, true, 'Marking as Done…');
  try {
    const res = await dashboardFetch(`/api/workflows/${encodeURIComponent(id)}/mark-done`, { method: 'POST' });
    const text = await res.text();
    if (!res.ok) {
      alert(text || 'Mark as Done failed');
      return;
    }
    let data;
    try {
      data = JSON.parse(text);
    } catch {
      alert(text);
      fetchWorkflowsSilent();
      return;
    }
    const lines = [];
    if (data.jira_ok) lines.push('Jira: transitioned to Done.');
    else lines.push(`Jira: failed${data.jira_error ? ` — ${data.jira_error}` : ''}.`);
    if (data.worktree_ok) lines.push('Worktree: removed (or already absent).');
    else lines.push(`Worktree: failed${data.worktree_error ? ` — ${data.worktree_error}` : ''}.`);
    if (data.workflow_removed) lines.push('Workflow removed from the list.');
    else lines.push('Workflow kept on the dashboard (fix errors and try again if needed).');
    alert(lines.join('\n'));
    fetchWorkflowsSilent();
  } catch (e) {
    console.error('Mark as Done failed:', e);
    alert('Mark as Done failed');
  } finally {
    setWorkflowCardLoading(id, false);
  }
}

async function openReportModal(ticketKey) {
  try {
    const res = await dashboardFetch(`/api/workflows/${encodeURIComponent(ticketKey)}`);
    if (!res.ok) return;
    const w = await res.json();
    renderReport(w);
    document.getElementById('reportModal').classList.remove('hidden');
    document.body.style.overflow = 'hidden';
  } catch (e) {
    console.error('Failed to load report:', e);
  }
}

function closeReportModal() {
  document.getElementById('reportModal').classList.add('hidden');
  document.body.style.overflow = '';
}

function dashboardConfirmModalOpen() {
  const m = document.getElementById('dashboardConfirmModal');
  return !!(m && !m.classList.contains('hidden'));
}

let dashboardConfirmResolver = null;

function closeDashboardConfirmModal(result) {
  const modal = document.getElementById('dashboardConfirmModal');
  if (modal) modal.classList.add('hidden');
  const report = document.getElementById('reportModal');
  if (!report || report.classList.contains('hidden')) {
    document.body.style.overflow = '';
  }
  const fn = dashboardConfirmResolver;
  dashboardConfirmResolver = null;
  if (fn) fn(!!result);
}

/**
 * @param {{ title: string, message: string, confirmLabel?: string, cancelLabel?: string, danger?: boolean }} opts
 * @returns {Promise<boolean>}
 */
function showDashboardConfirm(opts) {
  const modal = document.getElementById('dashboardConfirmModal');
  const titleEl = document.getElementById('dashboardConfirmTitle');
  const msgEl = document.getElementById('dashboardConfirmMessage');
  const okBtn = document.getElementById('dashboardConfirmOk');
  const cancelBtn = document.getElementById('dashboardConfirmCancel');
  if (!modal || !titleEl || !msgEl || !okBtn || !cancelBtn) {
    return Promise.resolve(false);
  }
  if (dashboardConfirmModalOpen()) {
    return Promise.resolve(false);
  }
  return new Promise(resolve => {
    dashboardConfirmResolver = resolve;
    titleEl.textContent = opts.title || 'Confirm';
    msgEl.textContent = opts.message || '';
    okBtn.textContent = opts.confirmLabel || 'Confirm';
    cancelBtn.textContent = opts.cancelLabel || 'Cancel';
    if (opts.danger) {
      okBtn.className =
        'dashboard-header-btn min-w-[5.5rem] bg-red-600/90 text-white border-red-500 hover:bg-red-600';
    } else {
      okBtn.className =
        'dashboard-header-btn min-w-[5.5rem] bg-blue-600/90 text-white border-blue-500 hover:bg-blue-600';
    }
    document.body.style.overflow = 'hidden';
    modal.classList.remove('hidden');
    okBtn.focus();
  });
}

function setupDashboardConfirmModal() {
  const modal = document.getElementById('dashboardConfirmModal');
  const backdrop = document.getElementById('dashboardConfirmBackdrop');
  const okBtn = document.getElementById('dashboardConfirmOk');
  const cancelBtn = document.getElementById('dashboardConfirmCancel');
  if (!modal || modal.dataset.bound === '1') return;
  modal.dataset.bound = '1';
  if (backdrop) {
    backdrop.addEventListener('click', () => closeDashboardConfirmModal(false));
  }
  okBtn.addEventListener('click', () => closeDashboardConfirmModal(true));
  cancelBtn.addEventListener('click', () => closeDashboardConfirmModal(false));
}

/** Dark overlay + spinner on a workflow card while a long action runs. */
function setWorkflowCardLoading(ticketKey, visible, message) {
  const overlay = document.getElementById(`card-overlay-${ticketKey}`);
  if (!overlay) return;
  const label = overlay.querySelector('.workflow-card-loading-text');
  if (label && message != null && String(message).trim()) {
    label.textContent = String(message).trim();
  }
  if (visible) {
    overlay.removeAttribute('hidden');
  } else {
    overlay.setAttribute('hidden', '');
  }
}

document.addEventListener('keydown', e => {
  if (e.key !== 'Escape') return;
  if (dashboardConfirmModalOpen()) {
    e.preventDefault();
    closeDashboardConfirmModal(false);
    return;
  }
  closeReportModal();
  const detail = document.getElementById('manualTicketDetailModal');
  if (detail && !detail.classList.contains('hidden')) {
    closeManualTicketDetailModal();
  } else {
    closeManualWorkflowModal();
  }
});

// --- Rendering ---

function getStatusInfo(state) {
  const s = state.toLowerCase();
  if (s === 'done' || s.startsWith('completed')) return { label: 'Completed', color: 'green', icon: 'check' };
  if (s.startsWith('error')) return { label: 'Error', color: 'red', icon: 'x' };
  if (s === 'paused') return { label: 'Paused', color: 'yellow', icon: 'pause' };
  if (s === 'stopped') return { label: 'Stopped', color: 'gray', icon: 'stop' };
  if (s.includes('pr review') || s.includes('addressing pr comments') || s.includes('merging base branch')) return { label: 'Running', color: 'blue', icon: 'pulse' };
  return { label: 'Running', color: 'blue', icon: 'pulse' };
}

/** Prefer server `progress_percent`; rough heuristic if missing (older clients / events). */
function cardProgressPercent(w) {
  if (typeof w.progress_percent === 'number' && Number.isFinite(w.progress_percent)) {
    return Math.max(0, Math.min(100, Math.round(w.progress_percent)));
  }
  return getProgressPercentFallback(w.state);
}

function getProgressPercentFallback(state) {
  const steps = [
    'Pending', 'Assigning', 'Retrieving', 'Creating Worktree',
    'AI agent', 'Reviewing', 'Done'
  ];
  const s = state.toLowerCase();
  for (let i = 0; i < steps.length; i++) {
    if (s.includes(steps[i].toLowerCase())) {
      return Math.round(((i + 1) / steps.length) * 100);
    }
  }
  if (s === 'done') return 100;
  if (s.includes('(run ') || s.includes('(cycle ') || s.includes('running agent steps')) {
    const aiIdx = steps.indexOf('AI agent');
    return Math.round(((aiIdx + 1) / steps.length) * 100);
  }
  return 10;
}

/** From API `progress_steps_total`; `0` if unknown (use legacy single fill bar). */
function workflowProgressStepsTotal(w) {
  const t = w.progress_steps_total;
  if (typeof t === 'number' && Number.isFinite(t) && t > 0) {
    return Math.floor(t);
  }
  return 0;
}

/** Matches maestro-core `workflow_progress_filled_segments` (half-up rounding). */
function workflowProgressFilledTotal(w) {
  const total = workflowProgressStepsTotal(w);
  const pct = cardProgressPercent(w);
  if (total <= 0) return { filled: 0, total: 0 };
  const filled = Math.min(total, Math.round((pct * total) / 100));
  return { filled, total };
}

function formatStepLineWithProgress(w, baseText) {
  const text = baseText == null ? '' : String(baseText);
  const { filled, total } = workflowProgressFilledTotal(w);
  if (total <= 0) return text;
  return `${text} (${filled}/${total})`;
}

function progressBarInnerHtml(w, status) {
  const { filled, total } = workflowProgressFilledTotal(w);
  if (total <= 0) {
    const progress = cardProgressPercent(w);
    return `<div class="w-full bg-gray-700 rounded-full h-1.5 overflow-hidden">
            <div class="progress-bar bg-${status.color}-500 h-1.5 rounded-full transition-all" style="width: ${progress}%"></div>
          </div>`;
  }
  let segs = '';
  for (let i = 0; i < total; i++) {
    const on = i < filled;
    segs += `<div class="workflow-progress-seg${on ? ` workflow-progress-seg-filled bg-${status.color}-500` : ' bg-gray-600'}"></div>`;
  }
  return `<div class="workflow-progress-track" role="group" aria-label="Workflow progress ${filled} of ${total} steps completed">${segs}</div>`;
}

function statusBadgeHtml(status) {
  const { label, color, icon } = status;
  let iconSvg = '';
  if (icon === 'check') {
    iconSvg = '<svg class="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2.5"><path stroke-linecap="round" stroke-linejoin="round" d="M5 13l4 4L19 7" /></svg>';
  } else if (icon === 'x') {
    iconSvg = '<svg class="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2.5"><path stroke-linecap="round" stroke-linejoin="round" d="M6 18L18 6M6 6l12 12" /></svg>';
  } else if (icon === 'pause') {
    iconSvg = '<svg class="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2.5"><path stroke-linecap="round" stroke-linejoin="round" d="M10 9v6m4-6v6" /></svg>';
  } else if (icon === 'stop') {
    iconSvg = '<svg class="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2.5"><path stroke-linecap="round" stroke-linejoin="round" d="M21 12a9 9 0 11-18 0 9 9 0 0118 0z" /><path stroke-linecap="round" stroke-linejoin="round" d="M9 10a1 1 0 011-1h4a1 1 0 011 1v4a1 1 0 01-1 1h-4a1 1 0 01-1-1v-4z" /></svg>';
  } else {
    iconSvg = `<span class="w-1.5 h-1.5 bg-${color}-400 rounded-full animate-pulse-dot"></span>`;
  }
  return `<span class="inline-flex items-center gap-1 text-xs font-medium px-2 py-0.5 rounded-full bg-${color}-500/15 text-${color}-400 border border-${color}-500/20">${iconSvg} ${label}</span>`;
}

/** Non-empty trimmed PR URL from API, or empty string. */
function workflowPrUrl(w) {
  const u = w.pr_url;
  if (typeof u !== 'string') return '';
  const t = u.trim();
  return t || '';
}

function renderWorkflowCard(w) {
  const status = getStatusInfo(w.state);
  const prUrl = workflowPrUrl(w);
  const borderClass = status.color === 'red' ? 'border-red-500/30 hover:border-red-500/40' :
                      status.color === 'yellow' ? 'border-yellow-500/30 hover:border-yellow-500/40' :
                      'border-gray-800 hover:border-gray-700';
  const opacityClass = status.label === 'Stopped' ? 'opacity-60 hover:opacity-80' : '';

  let stepLabel = 'Current step';
  if (status.label === 'Completed') stepLabel = 'Completed';
  else if (status.label === 'Error') stepLabel = 'Failed at step';
  else if (status.label === 'Paused') stepLabel = 'Paused at step';
  else if (status.label === 'Stopped') stepLabel = 'Stopped at step';

  let stateDisplay = w.state;
  if (status.label === 'Completed') stateDisplay = 'All steps passed';
  if (status.label === 'Error' && w.state.startsWith('Error:')) stateDisplay = w.state.replace('Error: ', '');
  const stateDisplayWithProgress = formatStepLineWithProgress(w, stateDisplay);

  const jiraBrowse = workflowJiraBrowseUrl(w);
  const isNoJiraWorkflow = w.jira_available === false;
  const goToTicketBtn = isNoJiraWorkflow ? '' : `
      <button type="button" onclick="window.open('${escapeAttr(jiraBrowse)}', '_blank', 'noopener,noreferrer')" class="workflow-action-btn bg-sky-500/10 text-sky-300 border-sky-500/25 hover:bg-sky-500/20">Go to ticket</button>`;
  const jiraLinkActions = `${goToTicketBtn}
      <button type="button" onclick="void openWorkflowTicketDescriptionModal(${escapeAttr(JSON.stringify(w.ticket_key))}, ${escapeAttr(JSON.stringify(w.ticket_summary))})" class="workflow-action-btn bg-violet-500/10 text-violet-300 border-violet-500/25 hover:bg-violet-500/20">Show description</button>`;
  let actions = jiraLinkActions;
  if (status.label === 'Running') {
    actions += `
      <button onclick="pauseWorkflow('${w.ticket_key}')" class="workflow-action-btn bg-yellow-500/10 text-yellow-400 border-yellow-500/20 hover:bg-yellow-500/20">Pause</button>
      <button onclick="stopWorkflow('${w.ticket_key}')" class="workflow-action-btn bg-red-500/10 text-red-400 border-red-500/20 hover:bg-red-500/20">Stop</button>`;
  } else if (status.label === 'Paused') {
    actions += `
      <button onclick="resumeWorkflow('${w.ticket_key}')" class="workflow-action-btn bg-green-500/10 text-green-400 border-green-500/20 hover:bg-green-500/20">Resume</button>
      <button onclick="stopWorkflow('${w.ticket_key}')" class="workflow-action-btn bg-red-500/10 text-red-400 border-red-500/20 hover:bg-red-500/20">Stop</button>`;
  }
  if (w.can_resume_from_error) {
    actions += `
      <button onclick="resumeFromError('${w.ticket_key}')" class="workflow-action-btn bg-teal-500/10 text-teal-400 border-teal-500/20 hover:bg-teal-500/20">Retry from last failure</button>`;
  }
  if (['Error', 'Stopped', 'Completed'].includes(status.label)) {
    actions += `
      <button onclick="retryWorkflow('${w.ticket_key}')" class="workflow-action-btn bg-blue-500/10 text-blue-400 border-blue-500/20 hover:bg-blue-500/20">Retry from 0</button>`;
  }
  if (w.can_address_pr_comments) {
    actions += `
      <button onclick="addressPrComments('${w.ticket_key}')" class="workflow-action-btn bg-indigo-500/10 text-indigo-300 border-indigo-500/25 hover:bg-indigo-500/20">Address PR Comments</button>`;
  }
  if (w.can_merge_base) {
    actions += `
      <button onclick="mergeBaseBranch('${w.ticket_key}')" class="workflow-action-btn bg-amber-500/10 text-amber-400 border-amber-500/25 hover:bg-amber-500/20">Merge Base Branch</button>`;
  }
  if (w.can_mark_done) {
    actions += `
      <button onclick="markWorkflowDone('${w.ticket_key}')" class="workflow-action-btn bg-emerald-500/10 text-emerald-400 border-emerald-500/25 hover:bg-emerald-500/20">Mark as Done</button>`;
  }
  if (w.can_delete) {
    actions += `
      <button onclick="deleteWorkflow('${w.ticket_key}')" class="workflow-action-btn bg-gray-600/30 text-gray-300 border-gray-600/50 hover:bg-gray-600/45">Delete</button>`;
  }
  if (w.can_open_editor) {
    if (w.editor_url) {
      actions += `
        <a href="${escapeHtml(w.editor_url)}" target="_blank" rel="noopener" class="workflow-action-btn bg-violet-500/10 text-violet-300 border-violet-500/25 hover:bg-violet-500/20 inline-flex items-center gap-1">Editor &#x2197;</a>`;
    } else {
      actions += `
        <button onclick="openEditor('${w.ticket_key}')" class="workflow-action-btn bg-violet-500/10 text-violet-300 border-violet-500/25 hover:bg-violet-500/20">Open editor</button>`;
    }
    if (w.terminal_url) {
      actions += `
        <a href="${escapeHtml(w.terminal_url)}" target="_blank" rel="noopener" class="workflow-action-btn bg-orange-500/10 text-orange-300 border-orange-500/25 hover:bg-orange-500/20 inline-flex items-center gap-1">Terminal &#x2197;</a>`;
    } else {
      actions += `
        <button onclick="openTerminal('${w.ticket_key}')" class="workflow-action-btn bg-orange-500/10 text-orange-300 border-orange-500/25 hover:bg-orange-500/20">Open terminal</button>`;
    }
    if (w.editor_url) {
      actions += `
        <button onclick="closeEditor('${w.ticket_key}')" class="workflow-action-btn bg-violet-500/10 text-violet-300 border-violet-500/25 hover:bg-violet-500/20">Close editor</button>`;
    }
  }
  actions += `
    <button onclick="openReportModal('${w.ticket_key}')" class="workflow-action-btn bg-gray-700/50 text-gray-300 border-gray-700 hover:bg-gray-700">Report</button>`;

  // Port mappings (configured + dynamic) — rendered below the action buttons
  let portMappingsHtml = '';
  if (w.editor_port_mappings && w.editor_port_mappings.length > 0) {
    const mappings = w.editor_port_mappings.map(([cp, hp]) => `<span class="text-xs text-gray-500">${cp} &#x2192; <a href="http://localhost:${hp}" target="_blank" rel="noopener" class="text-violet-400 hover:text-violet-300">localhost:${hp}</a></span>`).join(' ');
    portMappingsHtml += mappings;
  }
  const dynForwards = dynamicForwards[w.ticket_key] || [];
  const dynHtml = dynForwards.map(([cp, hp]) =>
    `<span class="text-xs text-gray-500">${cp} &#x2192; <a href="http://localhost:${hp}" target="_blank" rel="noopener" class="text-teal-400 hover:text-teal-300">localhost:${hp}</a></span>`
  ).join(' ');
  portMappingsHtml += dynHtml;

  // Terminal panel for active workflows
  let terminalHtml = '';
  if (status.label === 'Running' || status.label === 'Paused') {
    const ts = terminalState[w.ticket_key];
    const stepDisplay = ts ? (ts.completed ? `${ts.stepName} -- completed` : `$ ${ts.stepName}`) : '$ Waiting...';
    const headerCompletedClass = ts && ts.completed ? ' completed' : '';
    let linesHtml = '';
    if (ts && ts.lines.length > 0) {
      linesHtml = ts.lines.map(l => {
        const isWarn = /\bwarn(ing)?\b/i.test(l.text) || /\bWARN\b/.test(l.text);
        const cls = isWarn ? ' class="terminal-line-warn"' : (l.stream === 'stderr' ? ' class="terminal-line-stderr"' : '');
        return `<div${cls}>${escapeHtml(l.text)}</div>`;
      }).join('');
    }
    terminalHtml = `
      <div class="terminal-panel workflow-card-terminal">
        <div class="terminal-header${headerCompletedClass}">
          <span id="terminal-step-${w.ticket_key}">${escapeHtml(stepDisplay)}</span>
        </div>
        <div class="terminal-body workflow-card-terminal-body" id="terminal-body-${w.ticket_key}">${linesHtml}</div>
      </div>`;
  }

  const terminalSlot = terminalHtml
    ? `<div class="workflow-card-terminal-slot">${terminalHtml}</div>`
    : '<div class="workflow-card-terminal-placeholder" aria-hidden="true"></div>';

  const showPrHtml = prUrl
    ? `<a href="${escapeAttr(prUrl)}" target="_blank" rel="noopener noreferrer" class="workflow-show-pr-btn">Show PR</a>`
    : '';

  return `
    <div id="card-${w.ticket_key}" class="workflow-card bg-gray-900 border ${borderClass} rounded-xl overflow-hidden transition-colors ${opacityClass}">
      <div id="card-overlay-${w.ticket_key}" class="workflow-card-loading-overlay" hidden>
        <div class="manual-workflow-spinner" role="status" aria-label="Loading"></div>
        <span class="workflow-card-loading-text">Working…</span>
      </div>
      <div class="workflow-card-body">
        <div class="flex-shrink-0 flex flex-col gap-1.5 min-w-0">
          <div class="workflow-card-header-top flex items-center justify-between gap-3 min-w-0">
            <div class="flex items-center gap-2 min-w-0 flex-1">
              <span class="font-mono text-sm text-${status.color}-400 font-medium leading-tight">${w.ticket_key}</span>
              <span class="status-badge">${statusBadgeHtml(status)}</span>
            </div>
            ${showPrHtml ? `<div class="workflow-card-header-actions flex-shrink-0">${showPrHtml}</div>` : ''}
          </div>
          <h3 class="text-sm font-medium text-gray-200 truncate leading-snug">${escapeHtml(w.ticket_summary)}</h3>
        </div>
        <div class="flex-shrink-0 bg-gray-800/50 rounded-lg px-3 py-2.5">
          <div class="text-xs text-gray-500 mb-1">${stepLabel}</div>
          <div id="step-display-${w.ticket_key}" class="text-sm font-mono text-gray-300">${escapeHtml(stateDisplayWithProgress)}</div>
          <div class="workflow-progress-slot mt-2 w-full">${progressBarInnerHtml(w, status)}</div>
        </div>
        <div class="workflow-actions-row flex-shrink-0 flex flex-wrap gap-2">${actions}</div>
        <div id="port-mappings-${w.ticket_key}" class="flex flex-wrap gap-3 items-center px-1"${portMappingsHtml ? '' : ' hidden'}>${portMappingsHtml}</div>
        ${terminalSlot}
      </div>
    </div>`;
}

function renderWorkflows() {
  const grid = document.getElementById('workflowGrid');
  const empty = document.getElementById('emptyState');
  const list = workflowOrderKeys.map(k => workflows[k]).filter(w => w != null);
  const addCell = renderAddWorkflowCell();

  if (list.length === 0) {
    empty.classList.remove('hidden');
    // Update empty state text for no-Jira mode.
    const emptyText = empty.querySelector('p');
    if (emptyText && !jiraAvailable) {
      emptyText.textContent = 'No workflows yet. Click the button below to paste a ticket description and start a workflow.';
    }
    grid.innerHTML = '';
  } else {
    empty.classList.add('hidden');
    grid.innerHTML = list.map(renderWorkflowCard).join('') + addCell;

    // Restore scroll position on terminal bodies after re-render
    list.forEach(w => {
      const bodyEl = document.getElementById(`terminal-body-${w.ticket_key}`);
      if (bodyEl) bodyEl.scrollTop = bodyEl.scrollHeight;
    });
  }

  updateCounts(list);
}

function updateCounts(list) {
  let running = 0, completed = 0, errors = 0, paused = 0;
  list.forEach(w => {
    const s = getStatusInfo(w.state).label;
    if (s === 'Running') running++;
    else if (s === 'Completed') completed++;
    else if (s === 'Error') errors++;
    else if (s === 'Paused') paused++;
  });
  document.getElementById('countRunning').textContent = running;
  document.getElementById('countCompleted').textContent = completed;
  document.getElementById('countErrors').textContent = errors;
  document.getElementById('countPaused').textContent = paused;
}

// --- Report Modal ---

function renderReport(w) {
  const status = getStatusInfo(w.state);

  document.getElementById('reportTicketKey').textContent = w.ticket_key;
  document.getElementById('reportTicketKey').className = `font-mono text-sm text-${status.color}-400 font-medium`;
  document.getElementById('reportStatusBadge').innerHTML = statusBadgeHtml(status);
  document.getElementById('reportTitle').textContent = w.ticket_summary;

  const body = document.getElementById('reportBody');
  let html = `
    <div>
      <h3 class="text-xs font-semibold text-gray-500 uppercase tracking-wider mb-3">Ticket Info</h3>
      <div class="grid grid-cols-2 sm:grid-cols-3 gap-3 text-sm">
        <div class="bg-gray-800/50 rounded-lg px-3 py-2.5">
          <div class="text-xs text-gray-500 mb-0.5">Ticket</div>
          <div class="text-gray-300 font-mono">${escapeHtml(w.ticket_key)}</div>
        </div>
        <div class="bg-gray-800/50 rounded-lg px-3 py-2.5">
          <div class="text-xs text-gray-500 mb-0.5">Status</div>
          <div class="text-gray-300">${status.label}</div>
        </div>
        <div class="bg-gray-800/50 rounded-lg px-3 py-2.5">
          <div class="text-xs text-gray-500 mb-0.5">Started</div>
          <div class="font-mono text-gray-300">${new Date(w.started_at).toLocaleString()}</div>
        </div>
      </div>
    </div>`;

  if (w.steps_log && w.steps_log.length > 0) {
    html += `
      <div>
        <h3 class="text-xs font-semibold text-gray-500 uppercase tracking-wider mb-3">Workflow Steps</h3>
        <div class="space-y-2">`;
    w.steps_log.forEach(step => {
      const isFailed = step.status === 'Failed';
      const isSkipped = step.status === 'Skipped';
      const isSuccess = step.status === 'Success';

      let iconHtml, bgClass;
      if (isSuccess) {
        iconHtml = '<svg class="w-3.5 h-3.5 text-green-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2.5"><path stroke-linecap="round" stroke-linejoin="round" d="M5 13l4 4L19 7" /></svg>';
        bgClass = 'bg-green-500/15';
      } else if (isFailed) {
        iconHtml = '<svg class="w-3.5 h-3.5 text-red-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2.5"><path stroke-linecap="round" stroke-linejoin="round" d="M6 18L18 6M6 6l12 12" /></svg>';
        bgClass = 'bg-red-500/15';
      } else if (isSkipped) {
        iconHtml = '<svg class="w-3.5 h-3.5 text-gray-600" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2"><path stroke-linecap="round" stroke-linejoin="round" d="M20 12H4" /></svg>';
        bgClass = 'bg-gray-700/50';
      } else {
        iconHtml = '<span class="w-3 h-3 border-2 border-blue-400 border-t-transparent rounded-full animate-spin"></span>';
        bgClass = 'bg-blue-500/15';
      }

      const duration = step.completed_at ?
        formatDuration(new Date(step.started_at), new Date(step.completed_at)) : '--';
      const rowBg = isFailed ? 'bg-red-950/30 border border-red-900/30' : 'bg-gray-800/50';
      const opacity = isSkipped ? 'opacity-40' : '';

      html += `
        <div class="flex items-start gap-3 ${rowBg} rounded-lg px-4 py-3 ${opacity}">
          <div class="flex-shrink-0 w-6 h-6 rounded-full ${bgClass} flex items-center justify-center mt-0.5">${iconHtml}</div>
          <div class="flex-1 min-w-0">
            <div class="text-sm ${isFailed ? 'text-red-300 font-medium' : isSkipped ? 'text-gray-500' : 'text-gray-200'}">${escapeHtml(step.step_name)}</div>
            ${step.output && step.output.length > 0 ? `<div class="text-xs text-gray-500 font-mono mt-0.5">${escapeHtml(step.output[step.output.length - 1])}</div>` : ''}
            ${step.error ? `<pre class="mt-2 text-xs font-mono text-red-300/70 bg-red-950/40 rounded-md p-2.5 overflow-x-auto whitespace-pre-wrap">${escapeHtml(step.error)}</pre>` : ''}
          </div>
          <div class="text-xs text-gray-500 font-mono whitespace-nowrap">${duration}</div>
        </div>`;
    });
    html += '</div></div>';
  }

  body.innerHTML = html;
}

function formatDuration(start, end) {
  const secs = Math.floor((end - start) / 1000);
  const mins = Math.floor(secs / 60);
  const rem = secs % 60;
  return `${mins}m ${String(rem).padStart(2, '0')}s`;
}

function escapeHtml(str) {
  if (!str) return '';
  const div = document.createElement('div');
  div.textContent = str;
  return div.innerHTML;
}

/** Escape for double-quoted HTML attributes (e.g. `href`). */
function escapeAttr(str) {
  return String(str)
    .replace(/&/g, '&amp;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;')
    .replace(/</g, '&lt;');
}

/** One delegated click handler on `#manualWorkflowModalBody` (rows use `data-ticket-key`). */
function setupManualWorkflowListDelegation() {
  const body = document.getElementById('manualWorkflowModalBody');
  if (!body || body.dataset.manualListDelegation === '1') return;
  body.dataset.manualListDelegation = '1';
  body.addEventListener('click', ev => {
    const row = ev.target.closest('button.manual-workflow-row');
    if (!row || !body.contains(row)) return;
    const key = row.dataset.ticketKey;
    if (!key) return;
    ev.preventDefault();
    ev.stopPropagation();
    const summary = row.dataset.ticketSummary || key;
    void openManualTicketDetailModal(key, summary);
  });
}

// --- Init ---

// ---------------------------------------------------------------------------
// Paste-description modal (no-Jira manual workflow entry)
// ---------------------------------------------------------------------------

/** Slugify a workflow name into a valid branch/key segment (lowercase, hyphens, no leading/trailing dash). */
function slugifyWorkflowName(name) {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
}

function openPasteDescriptionModal() {
  const modal = document.getElementById('pasteDescriptionModal');
  if (!modal) return;
  const nameInput = document.getElementById('pasteDescName');
  const bodyTextarea = document.getElementById('pasteDescBody');
  if (nameInput) nameInput.value = '';
  if (bodyTextarea) bodyTextarea.value = '';
  modal.classList.remove('hidden');
  if (nameInput) nameInput.focus();
}

function closePasteDescriptionModal() {
  const modal = document.getElementById('pasteDescriptionModal');
  if (modal) modal.classList.add('hidden');
}

async function submitPasteDescription() {
  const nameInput = document.getElementById('pasteDescName');
  const bodyTextarea = document.getElementById('pasteDescBody');
  const rawName = (nameInput ? nameInput.value : '').trim();
  const description = (bodyTextarea ? bodyTextarea.value : '').trim();
  if (!rawName) return;
  const slug = slugifyWorkflowName(rawName);
  if (!slug) return;
  closePasteDescriptionModal();
  try {
    const res = await dashboardFetch('/api/workflows/start-manual', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        ticket_key: slug,
        ticket_summary: rawName,
        ticket_description: description || '',
      }),
    });
    if (!res.ok) {
      const errText = await res.text();
      console.error('Failed to start workflow:', errText);
      return;
    }
    await fetchWorkflowsSilent();
    renderWorkflows();
  } catch (e) {
    console.error('Failed to start workflow', e);
  }
}

async function init() {
  setupDashboardConfirmModal();
  setupManualWorkflowListDelegation();
  // Run workflow fetch first so a single 401 redirects to login before parallel calls.
  await fetchWorkflowsSilent();
  await Promise.all([fetchConfig(), fetchPollingStatus()]);
  // Config supplies Jira keys + manual cap for the **+** tile (first render had no config yet).
  renderWorkflows();
  const pollBtn = document.getElementById('pollingToggleBtn');
  if (pollBtn) pollBtn.addEventListener('click', togglePolling);
  const logoutBtn = document.getElementById('logoutBtn');
  if (logoutBtn) {
    logoutBtn.addEventListener('click', async () => {
      await fetch('/api/auth/logout', { method: 'POST', credentials: 'same-origin' });
      window.location.href = '/login.html';
    });
  }
  // Show the no-Jira alert dialog on page load when acli is not authenticated.
  if (!jiraAvailable) {
    const alertModal = document.getElementById('noJiraAlertModal');
    if (alertModal) {
      alertModal.classList.remove('hidden');
      const okBtn = document.getElementById('noJiraAlertOk');
      if (okBtn) {
        okBtn.addEventListener('click', () => alertModal.classList.add('hidden'));
      }
      // Also close on backdrop click.
      const backdrop = alertModal.querySelector('.absolute');
      if (backdrop) {
        backdrop.addEventListener('click', () => alertModal.classList.add('hidden'));
      }
    }
  }
  initialLoadDone = true;
  connectWebSocket();
}

// Ensure inline `onclick="…"` in index.html and older browsers always resolve handlers.
window.closeManualTicketDetailModal = closeManualTicketDetailModal;
window.closeManualWorkflowModal = closeManualWorkflowModal;
window.openManualWorkflowModal = openManualWorkflowModal;
window.openManualTicketDetailModal = openManualTicketDetailModal;
window.openWorkflowTicketDescriptionModal = openWorkflowTicketDescriptionModal;
window.confirmManualWorkflowStart = confirmManualWorkflowStart;
window.closePasteDescriptionModal = closePasteDescriptionModal;
window.submitPasteDescription = submitPasteDescription;

init();
