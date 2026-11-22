// SPDX-License-Identifier: MIT OR Apache-2.0
// Boot every Soil REPL on the page from the wasm evaluator and core prelude.
import init, { Session } from "./repl/soil_repl_wasm.js";

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

  function setup(panel) {
    const input = panel.querySelector(".repl-input");
    const highlighted = panel.querySelector(".repl-highlight code");
    const output = panel.querySelector(".repl-output");
    const status = panel.querySelector(".repl-status");
    const runButton = panel.querySelector(".repl-run");
    const liveToggle = panel.querySelector(".repl-live-toggle");
    let session = null;
    let timer = null;

    function setStatus(text, error = false) {
      status.textContent = text;
      status.classList.toggle("is-error", error);
    }

    function paint() {
      highlighted.innerHTML = `${highlight(input.value)}\n`;
      highlighted.parentElement.scrollTop = input.scrollTop;
      highlighted.parentElement.scrollLeft = input.scrollLeft;
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
    }

    function schedule() {
      if (!liveToggle.checked) return;
      clearTimeout(timer);
      timer = setTimeout(run, 450);
    }

    input.addEventListener("input", () => {
      paint();
      schedule();
    });
    input.addEventListener("scroll", paint);
    input.addEventListener("keydown", (event) => {
      if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
        event.preventDefault();
        run();
      }
      if (event.key === "Tab") {
        event.preventDefault();
        const start = input.selectionStart;
        const end = input.selectionEnd;
        input.value = `${input.value.slice(0, start)}  ${input.value.slice(end)}`;
        input.selectionStart = input.selectionEnd = start + 2;
        paint();
        schedule();
      }
    });
    runButton.addEventListener("click", run);
    liveToggle.addEventListener("change", () => {
      if (liveToggle.checked) run();
    });

    paint();
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
