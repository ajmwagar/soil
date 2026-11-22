// SPDX-License-Identifier: MIT OR Apache-2.0
// Boot every Soil REPL on the page from the wasm evaluator and core prelude.
import init, { Session } from "./repl/soil_repl_wasm.js";

const STORAGE_SOURCE = "soil-repl-source";
const STORAGE_HISTORY = "soil-repl-history";
const HISTORY_LIMIT = 20;

const panels = Array.from(document.querySelectorAll(".repl"));
if (panels.length) {
  const preludeUrl = new URL("./repl/prelude.soil", import.meta.url);
  const wasmUrl = new URL("./repl/soil_repl_wasm_bg.wasm", import.meta.url);
  const prelude = fetch(preludeUrl).then((response) => {
    if (!response.ok) throw new Error("could not load prelude");
    return response.text();
  });
  const wasm = init(wasmUrl);

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

  function readHistory() {
    try {
      const value = JSON.parse(readStorage(STORAGE_HISTORY, "[]"));
      return Array.isArray(value) ? value : [];
    } catch {
      return [];
    }
  }

  function sharedSource() {
    if (!location.hash.startsWith("#repl=")) return null;
    try {
      return decodeSource(location.hash.slice("#repl=".length));
    } catch {
      return null;
    }
  }

  function setup(panel) {
    const input = panel.querySelector(".repl-input");
    const highlighted = panel.querySelector(".repl-highlight code");
    const highlightBox = panel.querySelector(".repl-highlight");
    const output = panel.querySelector(".repl-output");
    const status = panel.querySelector(".repl-status");
    const runButton = panel.querySelector(".repl-run");
    const shareButton = panel.querySelector(".repl-share");
    const liveToggle = panel.querySelector(".repl-live-toggle");
    const past = readHistory();
    let historyIndex = past.length;
    let session = null;
    let timer = null;

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
      writeStorage(STORAGE_SOURCE, input.value);
      if (location.hash.startsWith("#repl=")) {
        history.replaceState(null, "", `${location.pathname}${location.search}`);
      }
    }

    function run() {
      if (!session) return;
      const source = input.value;
      if (!source.trim()) {
        output.textContent = "";
        output.classList.remove("is-error");
        return;
      }
      const result = session.eval(source);
      output.textContent = result;
      const error = result.startsWith("error:");
      output.classList.toggle("is-error", error);
      setStatus(error ? "error" : "ran", error);
      past.push(source);
      historyIndex = past.length;
      writeStorage(STORAGE_HISTORY, JSON.stringify(past.slice(-HISTORY_LIMIT)));
    }

    function schedule() {
      if (!liveToggle.checked) return;
      clearTimeout(timer);
      timer = setTimeout(run, 450);
    }

    const shared = sharedSource();
    const saved = readStorage(STORAGE_SOURCE, "");
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
        history.replaceState(null, "", `#repl=${encoded}`);
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
      const button = event.target instanceof Element ? event.target.closest(".doc-try") : null;
      if (!button) return;
      input.value = button.dataset.try || "";
      panel.scrollIntoView({ behavior: "smooth", block: "center" });
      input.dispatchEvent(new Event("input"));
      if (session) run();
      else setTimeout(run, 800);
    });

    paint();
    if (shared !== null) panel.scrollIntoView({ block: "center" });
    Promise.all([prelude, wasm])
      .then(([preludeText]) => {
        session = new Session(preludeText);
        setStatus("ready");
        if (liveToggle.checked) run();
      })
      .catch((error) => {
        setStatus("wasm failed to load", true);
        output.textContent = String(error && error.message ? error.message : error);
        output.classList.add("is-error");
      });
  }

  for (const panel of panels) setup(panel);
}
