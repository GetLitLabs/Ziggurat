// Origami.fe cockpit (M12). Plain checked-in JS: no npm, no build step, no third-party code.
// Everything shown here comes from Accordion.be on 127.0.0.1:7749. No hash, seal or BATON value
// is ever made up: until a reply arrives the fields say so, and every failure is shown in red.
"use strict";

const ACCORDION = "http://127.0.0.1:7749";
const POLL_MS = 5000;
const STATUS_TIMEOUT_MS = 3000;

const TERRACES = [
  { level: 1, name: "Foundation", job: "Keys, stele, shred" },
  { level: 2, name: "Substrate", job: "Daemon, loopback, wasm, compile" },
  { level: 3, name: "Workforce", job: "Model, retrieve, seed, jobs" },
  { level: 4, name: "Crucible", job: "Tests, replay, gates" },
  { level: 5, name: "Pinnacle", job: "Cockpit, operator seal" },
];

// INDEX.md §4. `live` is the spec status, not a health claim; health comes from the status poll.
const MODULES = [
  { id: "M01-NeuralHighway", terrace: 3, live: false },
  { id: "M02-Accordion", terrace: 2, live: true },
  { id: "M03-Krypt", terrace: 1, live: true },
  { id: "M04-WasiHealer", terrace: 2, live: true },
  { id: "M05-CodeASeed", terrace: 3, live: true },
  { id: "M06-Polymorph", terrace: 2, live: false },
  { id: "M07-Genesis", terrace: 1, live: false },
  { id: "M08-ZeroTrace", terrace: 1, live: false },
  { id: "M09-RamGenie", terrace: 3, live: true },
  { id: "M10-Baton", terrace: 4, live: true },
  { id: "M11-Lense", terrace: 4, live: true },
  { id: "M12-Origami", terrace: 5, live: true },
];

const state = { terrace: 3, module: "M09-RamGenie", sealed: false, dispatching: false };

const $ = (id) => document.getElementById(id);

function el(tag, attrs, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs || {})) node.setAttribute(key, value);
  for (const child of children) node.append(child);
  return node;
}

function setDot(id, ok) {
  $(id).dataset.ok = ok === null ? "" : String(ok);
}

function showError(id, message) {
  const box = $(id);
  box.textContent = message || "";
  box.hidden = !message;
}

function fillList(list, rows) {
  list.replaceChildren();
  for (const [key, value] of rows) {
    list.append(el("dt", {}, key), el("dd", {}, value === undefined || value === null ? "—" : String(value)));
  }
}

function renderRail() {
  const rail = $("rail");
  rail.replaceChildren();
  for (const t of TERRACES) {
    const button = el("button", { type: "button", "aria-current": String(t.level === state.terrace) },
      `T${t.level} ${t.name}`, el("span", {}, t.job));
    button.addEventListener("click", () => {
      state.terrace = t.level;
      $("terrace").value = String(t.level);
      const first = MODULES.find((m) => m.terrace === t.level);
      if (first) state.module = first.id;
      render();
      poll();
    });
    rail.append(button);
  }
  const t = TERRACES[state.terrace - 1];
  $("terrace-title").textContent = `T${t.level} ${t.name}`;
  $("terrace-job").textContent = t.job;
}

function renderChips() {
  const chips = $("chips");
  chips.replaceChildren();
  for (const m of MODULES) {
    const chip = el("button", {
      type: "button",
      class: "chip",
      "aria-pressed": String(m.id === state.module),
      "data-live": String(m.live),
      "data-dim": String(m.terrace !== state.terrace),
      title: `Terrace T${m.terrace}`,
    }, m.id, el("small", {}, m.live ? "LIVE" : "TYPED"));
    chip.addEventListener("click", () => {
      state.module = m.id;
      renderChips();
      poll();
    });
    chips.append(chip);
  }
  $("telemetry-module").textContent = state.module;
}

function render() {
  renderRail();
  renderChips();
}

async function fetchJson(url, options, timeoutMs) {
  const controller = new AbortController();
  const timer = timeoutMs ? setTimeout(() => controller.abort(), timeoutMs) : null;
  try {
    const response = await fetch(url, { ...options, signal: controller.signal });
    let body = null;
    try {
      body = await response.json();
    } catch (_) {
      body = null;
    }
    return { ok: response.ok, status: response.status, body };
  } finally {
    if (timer) clearTimeout(timer);
  }
}

function setSealBadge(kind, text) {
  const badge = $("seal-badge");
  badge.dataset.state = kind;
  badge.textContent = text;
}

function updateDispatchGate() {
  const button = $("dispatch-btn");
  button.disabled = !state.sealed || state.dispatching;
  $("dispatch-note").textContent = state.dispatching
    ? "Dispatching… the runner may take a while."
    : state.sealed
      ? ""
      : "Inference is refused until the daemon reports ROYAL_SEAL_LOCKED.";
}

let pollSeq = 0;

async function poll() {
  const seq = ++pollSeq;
  const module = state.module;
  try {
    const { ok, status, body } = await fetchJson(
      `${ACCORDION}/v1/ziggurat/pillar/${encodeURIComponent(module)}/status`, {}, STATUS_TIMEOUT_MS);
    if (seq !== pollSeq) return;
    setDot("dot-daemon", true);
    if (!ok || !body) {
      showError("status-error", `HTTP ${status}: ${(body && body.error) || "no JSON body"}`);
      return;
    }
    showError("status-error", body.seal_error ? `seal_error: ${body.seal_error}` : "");
    state.sealed = body.status === "ROYAL_SEAL_LOCKED" && body.charter_sealed === true;
    setSealBadge(state.sealed ? "sealed" : "unsealed", body.status);
    setDot("dot-runner", body.runner_up === true);
    setDot("dot-stele", body.stele_genesis === true);
    fillList($("telemetry"), [
      ["module_id", body.module_id],
      ["status", body.status],
      ["ziggurat_version", body.ziggurat_version],
      ["charter_sealed", body.charter_sealed],
      ["charter_byte_len", body.charter_byte_len],
      ["charter_blake3_hex", body.charter_blake3_hex],
      ["stele_genesis", body.stele_genesis],
      ["runner_up", body.runner_up],
      ["loopback_peer", body.loopback_peer],
    ]);
  } catch (err) {
    if (seq !== pollSeq) return;
    state.sealed = false;
    setDot("dot-daemon", false);
    setDot("dot-runner", null);
    setDot("dot-stele", null);
    setSealBadge("down", "DAEMON UNREACHABLE");
    showError("status-error", `Accordion.be on 127.0.0.1:7749 did not answer: ${err && err.name === "AbortError" ? "timed out" : err}`);
    fillList($("telemetry"), []);
  } finally {
    if (seq === pollSeq) {
      $("poll-note").textContent = `polled ${new Date().toLocaleTimeString()} · every 5 s`;
      updateDispatchGate();
    }
  }
}

async function dispatch(event) {
  event.preventDefault();
  if (!state.sealed || state.dispatching) return;
  state.dispatching = true;
  updateDispatchGate();
  showError("dispatch-error", "");
  const request = {
    model: $("model").value.trim(),
    bsmc_namespace: $("namespace").value.trim(),
    terrace_level: Number($("terrace").value),
    operator_prompt: $("prompt").value,
  };
  try {
    const { ok, status, body } = await fetchJson(`${ACCORDION}/v1/ramgenie/dispatch`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(request),
    });
    if (!ok || !body || body.status !== "DISPATCH_SEALED") {
      showError("dispatch-error", `HTTP ${status}: ${(body && body.error) || "unexpected reply"}`);
      return;
    }
    $("baton-hex").textContent = body.baton_envelope_blake3_hex;
    $("baton-hex").dataset.empty = "false";
    fillList($("dispatch-meta"), [
      ["status", body.status],
      ["bsmc_namespace", body.bsmc_namespace],
      ["terrace_level", body.terrace_level],
      ["baton_trace_id", body.baton_trace_id],
      ["stele_row", body.stele_dispatch_row_id],
      ["charter_blake3_hex", body.charter_blake3_hex],
    ]);
    $("output").textContent = body.model_output;
    $("output").dataset.empty = "false";
  } catch (err) {
    showError("dispatch-error", `Dispatch failed before a reply: ${err}`);
  } finally {
    state.dispatching = false;
    updateDispatchGate();
  }
}

function init() {
  const select = $("terrace");
  for (const t of TERRACES) select.append(el("option", { value: String(t.level) }, `T${t.level} ${t.name}`));
  select.value = String(state.terrace);
  $("dispatch-form").addEventListener("submit", dispatch);
  render();
  updateDispatchGate();
  poll();
  setInterval(poll, POLL_MS);
}

document.addEventListener("DOMContentLoaded", init);
