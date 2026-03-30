// Maestro Dashboard Application

let workflows = {};
let terminalState = {}; // { [ticket_key]: { stepName: string, lines: OutputLine[], completed: bool } }
let ws = null;
let wsReconnectTimer = null;
let dryMode = false;
let initialLoadDone = false;
const TERMINAL_MAX_LINES = 500;

// --- WebSocket ---

function connectWebSocket() {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
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
    if (evt.error) {
      wf.error = evt.error;
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

  // Also update the current step display on the card
  const stepEl = document.getElementById(`step-display-${evt.ticket_key}`);
  if (stepEl) {
    stepEl.textContent = evt.step_name;
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
  if (stepEl) stepEl.textContent = wf.state;

  // Update progress bar
  const progress = getProgressPercent(wf.state);
  const progressBar = card.querySelector('.progress-bar');
  if (progressBar) progressBar.style.width = `${progress}%`;

  // If workflow finished (completed/error/stopped), do a full render to update buttons and terminal visibility
  if (['Completed', 'Error', 'Stopped'].includes(status.label)) {
    renderWorkflows();
  }
}

// --- API ---

// Silent fetch — doesn't cause a visual flash
async function fetchWorkflowsSilent() {
  try {
    const res = await fetch('/api/workflows');
    const list = await res.json();
    workflows = {};
    list.forEach(w => {
      workflows[w.ticket_key] = w;
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

async function fetchConfig() {
  try {
    const res = await fetch('/api/config');
    const cfg = await res.json();
    dryMode = cfg.general.dry_mode;
    const banner = document.getElementById('dryBanner');
    if (banner) {
      banner.classList.toggle('hidden', !dryMode);
    }
  } catch (e) {
    // ignore
  }
}

async function pauseWorkflow(id) {
  try {
    await fetch(`/api/workflows/${encodeURIComponent(id)}/pause`, { method: 'POST' });
    // State update comes via WebSocket, no need to refetch
  } catch (e) {
    console.error('Failed to pause workflow:', e);
  }
}

async function resumeWorkflow(id) {
  try {
    await fetch(`/api/workflows/${encodeURIComponent(id)}/resume`, { method: 'POST' });
  } catch (e) {
    console.error('Failed to resume workflow:', e);
  }
}

async function retryWorkflow(id) {
  try {
    await fetch(`/api/workflows/${encodeURIComponent(id)}/retry`, { method: 'POST' });
    // Clear terminal state for this workflow
    delete terminalState[id];
    // Fetch fresh state since the workflow was replaced
    fetchWorkflowsSilent();
  } catch (e) {
    console.error('Failed to retry workflow:', e);
  }
}

async function stopWorkflow(id) {
  if (!confirm('Are you sure you want to stop this workflow? The ticket will be unassigned.')) return;
  try {
    await fetch(`/api/workflows/${encodeURIComponent(id)}/stop`, { method: 'POST' });
  } catch (e) {
    console.error('Failed to stop workflow:', e);
  }
}

async function openReportModal(ticketKey) {
  try {
    const res = await fetch(`/api/workflows/${encodeURIComponent(ticketKey)}`);
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

document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') closeReportModal();
});

// --- Rendering ---

function getStatusInfo(state) {
  const s = state.toLowerCase();
  if (s === 'done' || s.startsWith('completed')) return { label: 'Completed', color: 'green', icon: 'check' };
  if (s.startsWith('error')) return { label: 'Error', color: 'red', icon: 'x' };
  if (s === 'paused') return { label: 'Paused', color: 'yellow', icon: 'pause' };
  if (s === 'stopped') return { label: 'Stopped', color: 'gray', icon: 'stop' };
  return { label: 'Running', color: 'blue', icon: 'pulse' };
}

function getProgressPercent(state) {
  const steps = [
    'Pending', 'Assigning', 'Retrieving', 'Creating Worktree',
    'Address Ticket - Pass 1', 'Reviewing', 'Address Ticket - Pass 2', 'Reviewing',
    'Address Ticket - Pass 3', 'Reviewing', 'Running Lint', 'Running Unit', 'Running E2E',
    'Creating PR', 'Done'
  ];
  const s = state.toLowerCase();
  for (let i = 0; i < steps.length; i++) {
    if (s.includes(steps[i].toLowerCase())) {
      return Math.round(((i + 1) / steps.length) * 100);
    }
  }
  if (s === 'done') return 100;
  return 10;
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

function renderWorkflowCard(w) {
  const status = getStatusInfo(w.state);
  const progress = getProgressPercent(w.state);
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

  let actions = '';
  if (status.label === 'Running') {
    actions = `
      <button onclick="pauseWorkflow('${w.ticket_key}')" class="flex-1 inline-flex items-center justify-center gap-1.5 text-xs font-medium px-3 py-2 rounded-lg bg-yellow-500/10 text-yellow-400 border border-yellow-500/20 hover:bg-yellow-500/20 transition-colors">Pause</button>
      <button onclick="stopWorkflow('${w.ticket_key}')" class="flex-1 inline-flex items-center justify-center gap-1.5 text-xs font-medium px-3 py-2 rounded-lg bg-red-500/10 text-red-400 border border-red-500/20 hover:bg-red-500/20 transition-colors">Stop</button>`;
  } else if (status.label === 'Paused') {
    actions = `
      <button onclick="resumeWorkflow('${w.ticket_key}')" class="flex-1 inline-flex items-center justify-center gap-1.5 text-xs font-medium px-3 py-2 rounded-lg bg-green-500/10 text-green-400 border border-green-500/20 hover:bg-green-500/20 transition-colors">Resume</button>
      <button onclick="stopWorkflow('${w.ticket_key}')" class="flex-1 inline-flex items-center justify-center gap-1.5 text-xs font-medium px-3 py-2 rounded-lg bg-red-500/10 text-red-400 border border-red-500/20 hover:bg-red-500/20 transition-colors">Stop</button>`;
  }
  if (['Error', 'Stopped', 'Completed'].includes(status.label)) {
    actions += `
      <button onclick="retryWorkflow('${w.ticket_key}')" class="flex-1 inline-flex items-center justify-center gap-1.5 text-xs font-medium px-3 py-2 rounded-lg bg-blue-500/10 text-blue-400 border border-blue-500/20 hover:bg-blue-500/20 transition-colors">Retry</button>`;
  }
  actions += `
    <button onclick="openReportModal('${w.ticket_key}')" class="flex-1 inline-flex items-center justify-center gap-1.5 text-xs font-medium px-3 py-2 rounded-lg bg-gray-700/50 text-gray-300 border border-gray-700 hover:bg-gray-700 transition-colors">Report</button>`;

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
      <div class="terminal-panel">
        <div class="terminal-header${headerCompletedClass}">
          <span id="terminal-step-${w.ticket_key}">${escapeHtml(stepDisplay)}</span>
        </div>
        <div class="terminal-body" id="terminal-body-${w.ticket_key}">${linesHtml}</div>
      </div>`;
  }

  return `
    <div id="card-${w.ticket_key}" class="bg-gray-900 border ${borderClass} rounded-xl overflow-hidden transition-colors ${opacityClass}">
      <div class="p-5">
        <div class="flex items-start justify-between mb-3">
          <div class="flex-1 min-w-0">
            <div class="flex items-center gap-2 mb-1">
              <span class="font-mono text-sm text-${status.color}-400 font-medium">${w.ticket_key}</span>
              <span class="status-badge">${statusBadgeHtml(status)}</span>
            </div>
            <h3 class="text-sm font-medium text-gray-200 truncate">${escapeHtml(w.ticket_summary)}</h3>
          </div>
        </div>
        <div class="bg-gray-800/50 rounded-lg px-3 py-2.5 mb-4">
          <div class="text-xs text-gray-500 mb-1">${stepLabel}</div>
          <div id="step-display-${w.ticket_key}" class="text-sm font-mono text-gray-300">${escapeHtml(stateDisplay)}</div>
          <div class="mt-2 w-full bg-gray-700 rounded-full h-1.5">
            <div class="progress-bar bg-${status.color}-500 h-1.5 rounded-full transition-all" style="width: ${progress}%"></div>
          </div>
        </div>
        <div class="flex items-center gap-2">${actions}</div>
        ${terminalHtml}
      </div>
    </div>`;
}

function renderWorkflows() {
  const grid = document.getElementById('workflowGrid');
  const empty = document.getElementById('emptyState');
  const list = Object.values(workflows);

  list.sort((a, b) => {
    const order = { Running: 0, Paused: 1, Error: 2, Completed: 3, Stopped: 4 };
    const sa = getStatusInfo(a.state).label;
    const sb = getStatusInfo(b.state).label;
    return (order[sa] ?? 5) - (order[sb] ?? 5);
  });

  if (list.length === 0) {
    grid.innerHTML = '';
    empty.classList.remove('hidden');
  } else {
    empty.classList.add('hidden');
    grid.innerHTML = list.map(renderWorkflowCard).join('');

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

// --- Init ---

async function init() {
  await Promise.all([fetchWorkflowsSilent(), fetchConfig()]);
  initialLoadDone = true;
  connectWebSocket();
}

init();
