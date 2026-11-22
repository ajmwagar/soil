// SPDX-License-Identifier: MIT OR Apache-2.0
// Boot the browser Soil REPL from the wasm evaluator and core prelude.
import init, { Session } from "./repl/soil_repl_wasm.js";

const panel = document.querySelector(".repl");
if (panel) {
  const input = panel.querySelector("#soil-repl-input");
  const output = panel.querySelector("#soil-repl-output");
  const status = panel.querySelector("#soil-repl-status");
  const runButton = panel.querySelector("#soil-repl-run");
  let session = null;

  function setStatus(text, error = false) {
    status.textContent = text;
    status.classList.toggle("is-error", error);
  }

  async function boot() {
    setStatus("loading…");
    try {
      const preludeUrl = new URL("./repl/prelude.soil", import.meta.url);
      const wasmUrl = new URL("./repl/soil_repl_wasm_bg.wasm", import.meta.url);
      const [prelude] = await Promise.all([
        fetch(preludeUrl).then((response) => {
          if (!response.ok) throw new Error("could not load prelude");
          return response.text();
        }),
        init(wasmUrl),
      ]);
      session = new Session(prelude);
      setStatus("ready");
    } catch (error) {
      setStatus("wasm failed to load", true);
      output.textContent = String(error && error.message ? error.message : error);
      output.classList.add("is-error");
    }
  }

  function run() {
    if (!session) {
      setStatus("still loading", true);
      return;
    }
    const source = input.value;
    if (!source.trim()) return;
    const result = session.eval(source);
    output.textContent = result;
    output.classList.toggle("is-error", result.startsWith("error:"));
    setStatus("ran");
  }

  runButton.addEventListener("click", run);
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
    }
  });

  boot();
}
