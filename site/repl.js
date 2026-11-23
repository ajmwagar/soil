// SPDX-License-Identifier: MIT OR Apache-2.0
// Boot every Soil REPL on the page. Each panel owns a worker so a slow edit
// can be terminated without blocking the page.
import { mountLifeCanvas } from "./repl-life.js";

const STORAGE_SOURCE = "soil-repl-source";
const STORAGE_HISTORY = "soil-repl-history";
const HISTORY_LIMIT = 20;
const EVAL_TIMEOUT_MS = 3000;

const panels = Array.from(document.querySelectorAll(".repl"));
if (panels.length) {
  const preludeUrl = new URL("./repl/prelude.soil", import.meta.url);
  const workerUrl = new URL("./repl-worker.js", import.meta.url);
  const prelude = fetch(preludeUrl).then((response) => {
    if (!response.ok) throw new Error("could not load prelude");
    return response.text();
  });

  const tokens = /(;[^\n]*)|("(?:\\.|[^"\\])*")|(:[^\s()";]+)|(#t|#f|\bnil\b)|(-?\b\d+(?:\.\d+)?\b)|(\b(?:defun|defmacro|defspore|let|if|cond|and|or|not|quote|lambda|fn|do|begin|match)\b)|([()])|([^\s()";]+)/g;

  function escapeHtml(text) {
    return text.replace(/[&<>]/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[char]);
  }

  function highlight(code) {
    let out = "";
    let last = 0;
    let match;
    tokens.lastIndex = 0;
    while ((match = tokens.exec(code))) {
      if (match.index > last) out += escapeHtml(code.slice(last, match.index));
      const token = match[0];
      let className = "tok-symbol";
      if (match[1]) className = "tok-comment";
      else if (match[2]) className = "tok-string";
      else if (match[3]) className = "tok-keyword";
      else if (match[4]) className = "tok-literal";
      else if (match[5]) className = "tok-number";
      else if (match[6]) className = "tok-special";
      else if (match[7]) className = "tok-paren";
      else if (code.slice(0, match.index).trimEnd().endsWith("(")) className = "tok-fn";
      out += `<span class="${className}">${escapeHtml(token)}</span>`;
      last = match.index + token.length;
    }
    return out + escapeHtml(code.slice(last));
  }

  function encodeSource(text) {
    const bytes = new TextEncoder().encode(text);
    let binary = "";
    for (const byte of bytes) binary += String.fromCharCode(byte);
    return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
  }

  function decodeSource(value) {
    const padded = value.replaceAll("-", "+").replaceAll("_", "/");
    const binary = atob(padded + "=".repeat((4 - padded.length % 4) % 4));
    return new TextDecoder().decode(Uint8Array.from(binary, (char) => char.charCodeAt(0)));
  }

  function readStorage(key, fallback) {
    try {
      return localStorage.getItem(key) ?? fallback;
    } catch {
      return fallback;
    }
  }

  function writeStorage(key, value) {
    try {
      localStorage.setItem(key, value);
    } catch {
      // Private mode or a file:// page; the REPL still works in memory.
    }
  }

  function readHistory(key) {
    try {
      const value = JSON.parse(readStorage(key, "[]"));
      return Array.isArray(value) ? value : [];
    } catch {
      return [];
    }
  }

  function sharedSource(panelId) {
    const prefix = `#repl-${panelId}=`;
    const legacy = panels.length === 1 && location.hash.startsWith("#repl=");
    if (!location.hash.startsWith(prefix) && !legacy) return null;
    try {
      return decodeSource(location.hash.slice(legacy ? "#repl=".length : prefix.length));
    } catch {
      return null;
    }
  }

  function errorText(error) {
    if (error && error.message) return error.message;
    return String(error);
  }

  function setup(panel, index) {
    const panelId = panel.dataset.replId || `panel-${index}`;
    const sourceKey = `${STORAGE_SOURCE}:${panelId}`;
    const historyKey = `${STORAGE_HISTORY}:${panelId}`;
    const input = panel.querySelector(".repl-input");
    const highlighted = panel.querySelector(".repl-highlight code");
    const highlightBox = panel.querySelector(".repl-highlight");
    const output = panel.querySelector(".repl-output");
    const status = panel.querySelector(".repl-status");
    const runButton = panel.querySelector(".repl-run");
    const shareButton = panel.querySelector(".repl-share");
    const liveToggle = panel.querySelector(".repl-live-toggle");
    const past = readHistory(historyKey);
    let historyIndex = past.length;
    let worker = null;
    let ready = false;
    let booting = false;
    let pendingRun = false;
    let timeout = null;
    let timer = null;
    let visual = null;

    function setStatus(text, error = false) {
      status.textContent = text;
      status.classList.toggle("is-error", error);
    }

    function syncScroll() {
      highlightBox.scrollTop = input.scrollTop;
      highlightBox.scrollLeft = input.scrollLeft;
    }

    function paint() {
      highlighted.innerHTML = `${highlight(input.value)}\n`;
      syncScroll();
    }

    function saveSource() {
      writeStorage(sourceKey, input.value);
      const legacy = panels.length === 1 && location.hash.startsWith("#repl=");
      if (location.hash.startsWith(`#repl-${panelId}=`) || legacy) {
        history.replaceState(null, "", `${location.pathname}${location.search}`);
      }
    }

    function clearVisual() {
      if (!visual) return;
      visual.destroy();
      visual = null;
    }

    function stopWorker(message) {
      if (timeout) clearTimeout(timeout);
      timeout = null;
      if (worker) worker.terminate();
      worker = null;
      ready = false;
      booting = false;
      pendingRun = false;
      clearVisual();
      if (message) {
        output.textContent = message;
        output.classList.add("is-error");
        setStatus("stopped", true);
      }
    }

    function finish(result) {
      if (timeout) clearTimeout(timeout);
      timeout = null;
      clearVisual();
      output.textContent = result;
      const error = result.startsWith("error:");
      output.classList.toggle("is-error", error);
      setStatus(error ? "error" : "ran", error);
      if (!error && panel.dataset.replId === "life") {
        try {
          visual = mountLifeCanvas(panel, output, result);
        } catch {
          visual = null;
          output.hidden = false;
          output.removeAttribute("aria-hidden");
        }
      }
    }

    function run() {
      const source = input.value;
      if (!source.trim()) {
        clearVisual();
        output.textContent = "";
        output.classList.remove("is-error");
        return;
      }
      if (!worker) {
        pendingRun = true;
        boot();
        return;
      }
      if (!ready) {
        pendingRun = true;
        return;
      }
      if (timeout) clearTimeout(timeout);
      setStatus("running…");
      worker.postMessage({ type: "eval", source });
      timeout = setTimeout(() => {
        stopWorker(`stopped after ${EVAL_TIMEOUT_MS / 1000}s — try a smaller input`);
      }, EVAL_TIMEOUT_MS);
      past.push(source);
      historyIndex = past.length;
      writeStorage(historyKey, JSON.stringify(past.slice(-HISTORY_LIMIT)));
    }

    function boot() {
      if (worker || booting) return;
      booting = true;
      ready = false;
      setStatus("loading…");
      const next = new Worker(workerUrl, { type: "module" });
      worker = next;
      next.onmessage = (event) => {
        if (worker !== next) return;
        const message = event.data;
        if (message.type === "ready") {
          booting = false;
          ready = true;
          setStatus("ready");
          if (pendingRun) {
            pendingRun = false;
            run();
          } else if (liveToggle.checked) {
            run();
          }
        } else if (message.type === "result") {
          booting = false;
          ready = true;
          finish(message.result);
        } else if (message.type === "error") {
          stopWorker(`wasm failed: ${message.error}`);
        }
      };
      next.onerror = (event) => {
        if (worker !== next) return;
        stopWorker(`worker failed: ${errorText(event)}`);
      };
      prelude
        .then((text) => {
          if (worker === next) next.postMessage({ type: "init", prelude: text });
        })
        .catch((error) => {
          if (worker === next) stopWorker(`prelude failed: ${errorText(error)}`);
        });
    }

    function schedule() {
      if (!liveToggle.checked) return;
      clearTimeout(timer);
      timer = setTimeout(run, 450);
    }

    const shared = sharedSource(panelId);
    const saved = readStorage(sourceKey, "");
    if (shared !== null) input.value = shared;
    else if (saved) input.value = saved;

    input.addEventListener("input", () => {
      paint();
      saveSource();
      schedule();
    });
    input.addEventListener("scroll", syncScroll);
    input.addEventListener("keydown", (event) => {
      if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
        event.preventDefault();
        run();
      }
      if (event.altKey && (event.key === "ArrowUp" || event.key === "ArrowDown")) {
        if (!past.length) return;
        event.preventDefault();
        if (event.key === "ArrowUp") historyIndex = Math.max(0, historyIndex - 1);
        else historyIndex = Math.min(past.length, historyIndex + 1);
        input.value = historyIndex >= past.length ? "" : past[historyIndex];
        paint();
        saveSource();
      }
      if (event.key === "Tab") {
        event.preventDefault();
        const start = input.selectionStart;
        const end = input.selectionEnd;
        input.value = `${input.value.slice(0, start)}  ${input.value.slice(end)}`;
        input.selectionStart = input.selectionEnd = start + 2;
        paint();
        saveSource();
        schedule();
      }
    });
    runButton.addEventListener("click", run);
    if (shareButton) {
      shareButton.addEventListener("click", async () => {
        const encoded = encodeSource(input.value);
        history.replaceState(null, "", `${location.pathname}${location.search}#repl-${panelId}=${encoded}`);
        try {
          await navigator.clipboard.writeText(location.href);
          setStatus("link copied");
        } catch {
          setStatus("link updated");
        }
      });
    }
    liveToggle.addEventListener("change", () => {
      if (liveToggle.checked) run();
    });

    document.addEventListener("click", (event) => {
      if (panel !== panels[0]) return;
      const button = event.target instanceof Element ? event.target.closest(".doc-try") : null;
      if (!button) return;
      input.value = button.dataset.try || "";
      panel.scrollIntoView({ behavior: "smooth", block: "center" });
      input.dispatchEvent(new Event("input"));
      run();
    });

    paint();
    if (shared !== null) panel.scrollIntoView({ block: "center" });
    boot();
  }

  for (const [index, panel] of panels.entries()) setup(panel, index);
}
