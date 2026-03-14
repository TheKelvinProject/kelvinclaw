(() => {
  const CHANNELS = ["telegram", "slack", "discord"];
  const refs = {
    endpoint: document.getElementById("endpoint"),
    token: document.getElementById("token"),
    autoRefresh: document.getElementById("autoRefresh"),
    connectBtn: document.getElementById("connectBtn"),
    refreshBtn: document.getElementById("refreshBtn"),
    healthBtn: document.getElementById("healthBtn"),
    statusBadge: document.getElementById("statusBadge"),
    lastRefresh: document.getElementById("lastRefresh"),
    overviewCards: document.getElementById("overviewCards"),
    warningsList: document.getElementById("warningsList"),
    ingressDiscovery: document.getElementById("ingressDiscovery"),
    promptInput: document.getElementById("promptInput"),
    runBtn: document.getElementById("runBtn"),
    runSummary: document.getElementById("runSummary"),
    runIdInput: document.getElementById("runIdInput"),
    runStateBtn: document.getElementById("runStateBtn"),
    runOutcomeBtn: document.getElementById("runOutcomeBtn"),
    runInspector: document.getElementById("runInspector"),
    runsRefreshBtn: document.getElementById("runsRefreshBtn"),
    runsSummary: document.getElementById("runsSummary"),
    runsTable: document.getElementById("runsTable"),
    sessionsRefreshBtn: document.getElementById("sessionsRefreshBtn"),
    sessionSummary: document.getElementById("sessionSummary"),
    sessionSelect: document.getElementById("sessionSelect"),
    sessionTable: document.getElementById("sessionTable"),
    sessionInspector: document.getElementById("sessionInspector"),
    channelCards: document.getElementById("channelCards"),
    pluginsRefreshBtn: document.getElementById("pluginsRefreshBtn"),
    pluginSummary: document.getElementById("pluginSummary"),
    pluginTable: document.getElementById("pluginTable"),
    trustInspector: document.getElementById("trustInspector"),
    scheduleRefreshBtn: document.getElementById("scheduleRefreshBtn"),
    scheduleSummary: document.getElementById("scheduleSummary"),
    scheduleTable: document.getElementById("scheduleTable"),
    scheduleFilter: document.getElementById("scheduleFilter"),
    scheduleHistory: document.getElementById("scheduleHistory"),
    clearLogBtn: document.getElementById("clearLogBtn"),
    log: document.getElementById("log"),
  };

  const state = {
    socket: null,
    seq: 0,
    inflight: new Map(),
    refreshTimer: null,
    health: null,
    runs: null,
    sessions: null,
    sessionDetail: null,
    plugins: null,
    schedules: null,
    scheduleHistory: null,
    lastRun: null,
  };

  function escapeHtml(value) {
    return String(value ?? "")
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;");
  }

  function appendLog(message, tone = "info") {
    const line = `[${new Date().toISOString()}] ${message}`;
    const prefix = tone === "error" ? "ERR " : tone === "warn" ? "WRN " : "INF ";
    refs.log.textContent += `${prefix}${line}\n`;
    refs.log.scrollTop = refs.log.scrollHeight;
  }

  function setStatus(text, tone = "idle") {
    refs.statusBadge.textContent = text;
    refs.statusBadge.className = `status-badge ${tone}`;
  }

  function nextId(prefix) {
    state.seq += 1;
    return `${prefix}-${state.seq}`;
  }

  function isConnected() {
    return state.socket && state.socket.readyState === WebSocket.OPEN;
  }

  function jsonText(value) {
    return JSON.stringify(value, null, 2);
  }

  function formatTime(value) {
    if (!value) {
      return "n/a";
    }
    return new Date(Number(value)).toLocaleString();
  }

  function shortText(value, max = 96) {
    const text = String(value ?? "").trim();
    if (!text) {
      return "n/a";
    }
    return text.length > max ? `${text.slice(0, max - 1)}…` : text;
  }

  function toneForWarning(isOkay) {
    return isOkay ? "ok" : "warn";
  }

  function scheduleAutoRefresh() {
    if (state.refreshTimer) {
      clearInterval(state.refreshTimer);
      state.refreshTimer = null;
    }
    const intervalMs = Number(refs.autoRefresh.value || 0);
    if (!isConnected() || intervalMs <= 0) {
      return;
    }
    state.refreshTimer = setInterval(() => {
      refreshAll().catch((error) => appendLog(`auto refresh failed: ${error.message}`, "warn"));
    }, intervalMs);
  }

  function sendRequest(method, params = {}, timeoutMs = 15000) {
    if (!isConnected()) {
      return Promise.reject(new Error("socket is not connected"));
    }
    const id = nextId(method.replace(/[^\w]+/g, "_"));
    state.socket.send(JSON.stringify({ type: "req", id, method, params }));
    return new Promise((resolve, reject) => {
      const timeout = window.setTimeout(() => {
        state.inflight.delete(id);
        reject(new Error(`timed out waiting for ${method}`));
      }, timeoutMs);
      state.inflight.set(id, { resolve, reject, timeout, method });
    });
  }

  async function connectSocket() {
    if (state.socket) {
      state.socket.close();
    }
    refs.log.textContent = "";
    setStatus("Connecting", "warn");
    const socket = new WebSocket(refs.endpoint.value.trim());
    state.socket = socket;

    socket.onopen = async () => {
      setStatus("Handshaking", "warn");
      try {
        const token = refs.token.value.trim();
        const response = await sendRequest("connect", {
          client_id: "kelvin-operator-ui",
          auth: token ? { token } : undefined,
        });
        appendLog(`connect ok: ${jsonText(response.payload)}`);
        setStatus("Connected", "ready");
        scheduleAutoRefresh();
        await refreshAll();
      } catch (error) {
        appendLog(`connect failed: ${error.message}`, "error");
        setStatus("Connect failed", "error");
      }
    };

    socket.onclose = () => {
      for (const pending of state.inflight.values()) {
        clearTimeout(pending.timeout);
        pending.reject(new Error("socket closed"));
      }
      state.inflight.clear();
      setStatus("Disconnected", "idle");
      scheduleAutoRefresh();
      appendLog("socket closed", "warn");
      state.socket = null;
    };

    socket.onerror = () => {
      appendLog("socket error", "error");
      setStatus("Socket error", "error");
    };

    socket.onmessage = (event) => {
      let frame = null;
      try {
        frame = JSON.parse(event.data);
      } catch (error) {
        appendLog(`invalid JSON frame: ${event.data}`, "error");
        return;
      }
      if (frame.type === "res" && frame.id && state.inflight.has(frame.id)) {
        const pending = state.inflight.get(frame.id);
        state.inflight.delete(frame.id);
        clearTimeout(pending.timeout);
        if (frame.ok) {
          pending.resolve(frame);
        } else {
          pending.reject(new Error(frame.error?.message || "request failed"));
        }
        return;
      }
      appendLog(`event: ${jsonText(frame)}`);
    };
  }

  async function refreshAll() {
    if (!isConnected()) {
      throw new Error("connect before refreshing");
    }
    await refreshHealth();
    await Promise.all([
      refreshSchedules(),
      refreshRuns(),
      refreshSessions(),
      refreshPlugins(),
    ]);
    refs.lastRefresh.textContent = `Last refresh: ${new Date().toLocaleTimeString()}`;
  }

  async function refreshHealth() {
    const response = await sendRequest("health", {});
    state.health = response.payload;
    renderOverview();
    renderChannels();
  }

  async function refreshRuns() {
    const response = await sendRequest("operator.runs.list", {});
    state.runs = response.payload;
    renderRuns();
  }

  async function refreshSessions() {
    const response = await sendRequest("operator.sessions.list", {});
    state.sessions = response.payload;
    renderSessions();
    const sessionId = refs.sessionSelect.value.trim();
    if (sessionId) {
      await refreshSessionDetail(sessionId);
    }
  }

  async function refreshSessionDetail(sessionId) {
    if (!sessionId) {
      state.sessionDetail = null;
      refs.sessionInspector.textContent = "Session history will appear here.";
      return;
    }
    const response = await sendRequest("operator.session.get", { session_id: sessionId, limit: 24 });
    state.sessionDetail = response.payload;
    refs.sessionInspector.textContent = jsonText(response.payload);
  }

  async function refreshPlugins() {
    const response = await sendRequest("operator.plugins.inspect", {});
    state.plugins = response.payload;
    renderPlugins();
  }

  async function refreshSchedules() {
    const response = await sendRequest("schedule.list", {});
    state.schedules = response.payload;
    renderSchedules();
    await refreshScheduleHistory();
  }

  async function refreshScheduleHistory() {
    if (!isConnected()) {
      return;
    }
    const scheduleId = refs.scheduleFilter.value.trim();
    const params = scheduleId ? { schedule_id: scheduleId } : {};
    const response = await sendRequest("schedule.history", params);
    state.scheduleHistory = response.payload;
    renderScheduleHistory();
  }

  function renderOverview() {
    const health = state.health;
    if (!health) {
      refs.overviewCards.innerHTML = "";
      refs.warningsList.innerHTML = "<li>Connect to the gateway to load health state.</li>";
      refs.ingressDiscovery.textContent = "Refresh to load ingress details.";
      return;
    }

    const enabledChannels = CHANNELS.filter((name) => health.channels?.[name]?.enabled).length;
    const scheduler = health.scheduler || {};
    const schedulerStatus = scheduler.status || {};
    const plugins = health.plugins || {};
    const trust = plugins.trust_policy || {};
    const cards = [
      {
        label: "Transport",
        value: `${String(health.security.transport || "ws").toUpperCase()} ${health.security.bind_scope || "unknown"}`,
        detail: health.security.bind_addr || "unknown bind",
        tone: toneForWarning(Boolean(health.security.tls_enabled) || health.security.bind_scope === "loopback"),
      },
      {
        label: "Ingress",
        value: health.ingress?.enabled ? "Enabled" : "Disabled",
        detail: health.ingress?.enabled
          ? `${health.ingress.bind_addr} ${health.ingress.base_path}`
          : "Enable --ingress-bind for webhook and operator HTTP access",
        tone: toneForWarning(!health.ingress?.enabled || health.ingress.bind_scope === "loopback"),
      },
      {
        label: "Installed Plugins",
        value: String(plugins.loaded_installed_plugins || health.loaded_installed_plugins || 0),
        detail: `${plugins.audit_counters?.plugin_count || 0} scanned on disk`,
        tone: toneForWarning(!plugins.audit_counters?.scan_error),
      },
      {
        label: "Trust Policy",
        value: trust.ok ? "Healthy" : "Needs attention",
        detail: `${trust.publishers_total || 0} publishers, ${trust.revoked_total || 0} revoked`,
        tone: toneForWarning(Boolean(trust.ok)),
      },
      {
        label: "Scheduler",
        value: String(schedulerStatus.schedule_count || 0),
        detail: `due now ${schedulerStatus.due_now_count || 0}, audit ${schedulerStatus.audit_count || 0}`,
        tone: toneForWarning(!scheduler.metrics?.last_error),
      },
      {
        label: "Auth",
        value: health.security.auth_required ? "Required" : "Not set",
        detail: `max connections ${health.security.max_connections}`,
        tone: toneForWarning(Boolean(health.security.auth_required)),
      },
      {
        label: "Channels",
        value: `${enabledChannels}/${CHANNELS.length}`,
        detail: "telegram, slack, discord",
        tone: toneForWarning(enabledChannels > 0),
      },
      {
        label: "Plugin Audit",
        value: `${plugins.audit_counters?.signatures_present || 0} signatures`,
        detail: `${plugins.audit_counters?.current_versions || 0} current versions`,
        tone: toneForWarning(!plugins.audit_counters?.scan_error),
      },
    ];

    refs.overviewCards.innerHTML = cards.map((card) => `
      <article class="metric-card">
        <h3>${escapeHtml(card.label)}</h3>
        <p class="metric-value">${escapeHtml(card.value)}</p>
        <p class="metric-detail">${escapeHtml(card.detail)}</p>
        <div class="metric-pulse ${escapeHtml(card.tone)}">${escapeHtml(card.tone)}</div>
      </article>
    `).join("");

    const warnings = buildWarnings(health);
    refs.warningsList.innerHTML = warnings.length
      ? warnings.map((warning) => `<li>${escapeHtml(warning)}</li>`).join("")
      : "<li>No immediate security or reliability warnings detected from current health data.</li>";

    refs.ingressDiscovery.textContent = jsonText({
      ingress: health.ingress,
      scheduler: schedulerStatus,
      plugins: {
        registry: plugins.registry,
        trust_policy: trust,
        capability_usage: plugins.capability_usage,
      },
      operator_ui: health.ingress?.operator_ui_path || null,
    });
  }

  function buildWarnings(health) {
    const warnings = [];
    if (!health.security?.auth_required) {
      warnings.push("Gateway token auth is not enabled.");
    }
    if (health.security?.bind_scope === "public" && !health.security?.tls_enabled) {
      warnings.push("Gateway WebSocket endpoint is public without TLS.");
    }
    if (health.ingress?.enabled && health.ingress.bind_scope === "public") {
      warnings.push("HTTP ingress listener is publicly reachable.");
    }
    CHANNELS.forEach((name) => {
      const channel = health.channels?.[name];
      if (!channel?.enabled) {
        return;
      }
      if (channel.ingress_verification?.listener_enabled && !channel.ingress_verification?.configured) {
        warnings.push(`${name} webhook listener is enabled but verification is not configured.`);
      }
      if (channel.metrics?.last_error) {
        warnings.push(`${name} channel last error: ${channel.metrics.last_error}`);
      }
      if (channel.ingress_verification?.last_error) {
        warnings.push(`${name} ingress verification error: ${channel.ingress_verification.last_error}`);
      }
    });
    if (health.scheduler?.metrics?.last_error) {
      warnings.push(`Scheduler error: ${health.scheduler.metrics.last_error}`);
    }
    if (!health.plugins?.plugin_home_exists) {
      warnings.push("Configured plugin home does not exist.");
    }
    if (!health.plugins?.trust_policy?.ok) {
      warnings.push(`Trust policy issue: ${health.plugins?.trust_policy?.error || "unavailable"}`);
    }
    if (health.plugins?.audit_counters?.scan_error) {
      warnings.push(`Plugin scan error: ${health.plugins.audit_counters.scan_error}`);
    }
    if (
      health.plugins?.trust_policy?.require_signature &&
      (health.plugins?.audit_counters?.plugin_count || 0) > (health.plugins?.audit_counters?.signatures_present || 0)
    ) {
      warnings.push("At least one installed plugin is missing plugin.sig while signature enforcement is enabled.");
    }
    return warnings;
  }

  function renderChannels() {
    const channels = state.health?.channels || {};
    refs.channelCards.innerHTML = CHANNELS.map((name) => {
      const channel = channels[name] || { enabled: false };
      const tone = !channel.enabled ? "warn" : channel.metrics?.last_error ? "warn" : "ok";
      return `
        <article class="channel-card">
          <div class="section-head">
            <div>
              <p class="eyebrow">${escapeHtml(name)}</p>
              <h3>${escapeHtml(channel.enabled ? "Enabled" : "Disabled")}</h3>
            </div>
            <span class="pill ${escapeHtml(tone)}">${escapeHtml(tone)}</span>
          </div>
          <p class="channel-meta">
            pairing ${channel.pairing_enabled ? "on" : "off"} • queue ${channel.queue_depth || 0}/${channel.queue_max_depth || 0}
          </p>
          <div class="channel-stats">
            <div><span>Verification</span><div>${escapeHtml(channel.ingress_verification?.method || "n/a")}</div></div>
            <div><span>Configured</span><div>${escapeHtml(String(channel.ingress_verification?.configured || false))}</div></div>
            <div><span>Last HTTP</span><div>${escapeHtml(channel.ingress_connectivity?.last_status_code || "n/a")}</div></div>
            <div><span>Accepted Webhooks</span><div>${escapeHtml(channel.metrics?.webhook_accepted_total ?? 0)}</div></div>
            <div><span>Denied Webhooks</span><div>${escapeHtml(channel.metrics?.webhook_denied_total ?? 0)}</div></div>
            <div><span>Retries Seen</span><div>${escapeHtml(channel.metrics?.webhook_retry_total ?? 0)}</div></div>
            <div><span>Ingress Total</span><div>${escapeHtml(channel.metrics?.ingest_total ?? 0)}</div></div>
            <div><span>Rate Limited</span><div>${escapeHtml(channel.metrics?.rate_limited_total ?? 0)}</div></div>
          </div>
        </article>
      `;
    }).join("");
  }

  function renderRuns() {
    const payload = state.runs;
    if (!payload) {
      refs.runsSummary.textContent = "Refresh to load persisted runs.";
      refs.runsTable.innerHTML = '<div class="empty-state">No run history loaded.</div>';
      return;
    }
    const runs = payload.runs || [];
    refs.runsSummary.textContent = `${runs.length} persisted runs from ${payload.state_dir || "no state dir"}`;
    if (!runs.length) {
      refs.runsTable.innerHTML = '<div class="empty-state">No persisted runs found.</div>';
      return;
    }
    refs.runsTable.innerHTML = `
      <table>
        <thead>
          <tr>
            <th>Run ID</th>
            <th>Session</th>
            <th>Accepted</th>
            <th>Updated</th>
            <th>Status</th>
          </tr>
        </thead>
        <tbody>
          ${runs.map((item) => {
            const status = item.last_outcome?.status || item.last_wait?.status || item.last_state?.status || "accepted";
            return `
              <tr>
                <td>${escapeHtml(item.run_id || "n/a")}</td>
                <td>${escapeHtml(item.session_id || "n/a")}</td>
                <td>${escapeHtml(formatTime(item.accepted_at_ms))}</td>
                <td>${escapeHtml(formatTime(item.updated_at_ms))}</td>
                <td>${escapeHtml(status)}</td>
              </tr>
            `;
          }).join("")}
        </tbody>
      </table>
    `;
  }

  function renderSessions() {
    const payload = state.sessions;
    if (!payload) {
      refs.sessionSummary.textContent = "Refresh to load session state.";
      refs.sessionTable.innerHTML = '<div class="empty-state">No sessions loaded.</div>';
      refs.sessionSelect.innerHTML = '<option value="">Select a session</option>';
      return;
    }
    const sessions = payload.sessions || [];
    refs.sessionSummary.textContent = `${sessions.length} sessions from ${payload.state_dir || "no state dir"}`;
    const selected = refs.sessionSelect.value;
    refs.sessionSelect.innerHTML = `<option value="">Select a session</option>${sessions.map((item) =>
      `<option value="${escapeHtml(item.session_id)}"${selected === item.session_id ? " selected" : ""}>${escapeHtml(item.session_id)}</option>`
    ).join("")}`;
    if (!sessions.length) {
      refs.sessionTable.innerHTML = '<div class="empty-state">No persisted sessions found.</div>';
      return;
    }
    refs.sessionTable.innerHTML = `
      <table>
        <thead>
          <tr>
            <th>Session</th>
            <th>Workspace</th>
            <th>Messages</th>
            <th>Last Message</th>
          </tr>
        </thead>
        <tbody>
          ${sessions.map((item) => `
            <tr>
              <td>${escapeHtml(item.session_id)}</td>
              <td>${escapeHtml(shortText(item.workspace_dir, 42))}</td>
              <td>${escapeHtml(item.message_count ?? 0)}</td>
              <td>${escapeHtml(shortText(item.last_message?.content || "n/a"))}</td>
            </tr>
          `).join("")}
        </tbody>
      </table>
    `;
  }

  function renderPlugins() {
    const payload = state.plugins;
    if (!payload) {
      refs.pluginSummary.textContent = "Refresh to load plugin and trust policy state.";
      refs.pluginTable.innerHTML = '<div class="empty-state">No plugin inventory loaded.</div>';
      refs.trustInspector.textContent = "Trust policy and registry details will appear here.";
      return;
    }
    const plugins = payload.plugins || [];
    const trust = payload.trust_policy || {};
    refs.pluginSummary.textContent = [
      `${plugins.length} plugin manifests`,
      `${payload.loaded_installed_plugins || 0} loaded`,
      `${trust.publishers_total || 0} trusted publishers`,
      `${trust.revoked_total || 0} revoked`,
    ].join(" • ");
    if (!plugins.length) {
      refs.pluginTable.innerHTML = '<div class="empty-state">No plugin manifests found in plugin home.</div>';
    } else {
      refs.pluginTable.innerHTML = `
        <table>
          <thead>
            <tr>
              <th>ID</th>
              <th>Version</th>
              <th>Runtime</th>
              <th>Publisher</th>
              <th>Tier</th>
              <th>Capabilities</th>
              <th>Flags</th>
            </tr>
          </thead>
          <tbody>
            ${plugins.map((item) => `
              <tr>
                <td>${escapeHtml(item.id || "n/a")}</td>
                <td>${escapeHtml(item.version || "n/a")}</td>
                <td>${escapeHtml(item.runtime || "n/a")}</td>
                <td>${escapeHtml(item.publisher || "n/a")}</td>
                <td>${escapeHtml(item.quality_tier || "unsigned_local")}</td>
                <td>${escapeHtml((item.capabilities || []).join(", ") || "n/a")}</td>
                <td>${escapeHtml(`${item.signature_present ? "sig" : "no-sig"} / ${item.is_current ? "current" : "non-current"}`)}</td>
              </tr>
            `).join("")}
          </tbody>
        </table>
      `;
    }
    refs.trustInspector.textContent = jsonText({
      registry: payload.registry,
      trust_policy: payload.trust_policy,
      capability_usage: payload.capability_usage,
      quality_tiers: payload.quality_tiers,
      publishers: payload.publishers,
      audit_counters: payload.audit_counters,
    });
  }

  function renderSchedules() {
    const payload = state.schedules;
    if (!payload) {
      refs.scheduleSummary.textContent = "Refresh to load scheduler state.";
      refs.scheduleTable.innerHTML = "No schedules loaded.";
      return;
    }

    const schedules = payload.schedules || [];
    const status = payload.status || {};
    refs.scheduleSummary.textContent =
      `${status.schedule_count || 0} schedules, ${status.due_now_count || 0} due now, next slot ${formatTime(status.next_slot_at_ms)}`;

    const current = refs.scheduleFilter.value;
    refs.scheduleFilter.innerHTML = `<option value="">All schedules</option>${schedules.map((item) =>
      `<option value="${escapeHtml(item.id)}"${current === item.id ? " selected" : ""}>${escapeHtml(item.id)}</option>`
    ).join("")}`;

    if (!schedules.length) {
      refs.scheduleTable.innerHTML = '<div class="empty-state">No schedules registered.</div>';
      return;
    }

    refs.scheduleTable.innerHTML = `
      <table>
        <thead>
          <tr>
            <th>ID</th>
            <th>Cron</th>
            <th>Next Slot</th>
            <th>Session</th>
            <th>Reply Target</th>
          </tr>
        </thead>
        <tbody>
          ${schedules.map((item) => `
            <tr>
              <td>${escapeHtml(item.id)}</td>
              <td>${escapeHtml(item.cron)}</td>
              <td>${escapeHtml(formatTime(item.next_slot_at_ms))}</td>
              <td>${escapeHtml(item.session_id || item.created_by_session || "n/a")}</td>
              <td>${escapeHtml(item.reply_target ? `${item.reply_target.channel}:${item.reply_target.account_id}` : "none")}</td>
            </tr>
          `).join("")}
        </tbody>
      </table>
    `;
  }

  function renderScheduleHistory() {
    if (!state.scheduleHistory) {
      refs.scheduleHistory.textContent = "Refresh schedules to load slot and audit history.";
      return;
    }
    refs.scheduleHistory.textContent = jsonText(state.scheduleHistory);
  }

  function renderRunSummary() {
    refs.runSummary.textContent = state.lastRun
      ? jsonText(state.lastRun)
      : "No run submitted yet.";
  }

  async function submitRun() {
    const prompt = refs.promptInput.value.trim();
    if (!prompt) {
      throw new Error("prompt is required");
    }
    const requestId = `operator-${Date.now()}`;
    const accepted = await sendRequest("agent", { request_id: requestId, prompt, timeout_ms: 10000 });
    const runId = accepted.payload?.run_id || "";
    refs.runIdInput.value = runId;
    state.lastRun = { accepted: accepted.payload };
    renderRunSummary();
    appendLog(`agent accepted: ${runId || "unknown run id"}`);
    const wait = await sendRequest("agent.wait", { run_id: runId, timeout_ms: 15000 });
    const outcome = await sendRequest("agent.outcome", { run_id: runId, timeout_ms: 15000 });
    state.lastRun = { accepted: accepted.payload, wait: wait.payload, outcome: outcome.payload };
    renderRunSummary();
    refs.runInspector.textContent = jsonText({ state: wait.payload, outcome: outcome.payload });
    await refreshRuns();
    await refreshSessions();
  }

  async function inspectRun(method) {
    const runId = refs.runIdInput.value.trim();
    if (!runId) {
      throw new Error("run id is required");
    }
    const response = await sendRequest(method, { run_id: runId, timeout_ms: 15000 });
    refs.runInspector.textContent = jsonText(response.payload);
    appendLog(`${method} loaded for ${runId}`);
  }

  refs.connectBtn.addEventListener("click", () => {
    connectSocket().catch((error) => appendLog(`connect failed: ${error.message}`, "error"));
  });
  refs.refreshBtn.addEventListener("click", () => {
    refreshAll().catch((error) => appendLog(`refresh failed: ${error.message}`, "error"));
  });
  refs.healthBtn.addEventListener("click", () => {
    refreshHealth().catch((error) => appendLog(`health failed: ${error.message}`, "error"));
  });
  refs.runBtn.addEventListener("click", () => {
    submitRun().catch((error) => appendLog(`run failed: ${error.message}`, "error"));
  });
  refs.runStateBtn.addEventListener("click", () => {
    inspectRun("run.state").catch((error) => appendLog(`run.state failed: ${error.message}`, "error"));
  });
  refs.runOutcomeBtn.addEventListener("click", () => {
    inspectRun("run.outcome").catch((error) => appendLog(`run.outcome failed: ${error.message}`, "error"));
  });
  refs.runsRefreshBtn.addEventListener("click", () => {
    refreshRuns().catch((error) => appendLog(`run ledger refresh failed: ${error.message}`, "error"));
  });
  refs.sessionsRefreshBtn.addEventListener("click", () => {
    refreshSessions().catch((error) => appendLog(`session refresh failed: ${error.message}`, "error"));
  });
  refs.sessionSelect.addEventListener("change", () => {
    refreshSessionDetail(refs.sessionSelect.value.trim()).catch((error) =>
      appendLog(`session detail failed: ${error.message}`, "error")
    );
  });
  refs.pluginsRefreshBtn.addEventListener("click", () => {
    refreshPlugins().catch((error) => appendLog(`plugin refresh failed: ${error.message}`, "error"));
  });
  refs.scheduleRefreshBtn.addEventListener("click", () => {
    refreshSchedules().catch((error) => appendLog(`schedule refresh failed: ${error.message}`, "error"));
  });
  refs.scheduleFilter.addEventListener("change", () => {
    refreshScheduleHistory().catch((error) => appendLog(`schedule history failed: ${error.message}`, "error"));
  });
  refs.autoRefresh.addEventListener("change", scheduleAutoRefresh);
  refs.clearLogBtn.addEventListener("click", () => {
    refs.log.textContent = "";
  });

  renderOverview();
  renderChannels();
  renderRuns();
  renderSessions();
  renderPlugins();
  renderSchedules();
  renderRunSummary();
})();
