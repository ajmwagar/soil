// SPDX-License-Identifier: MIT OR Apache-2.0
// Isolated Soil evaluator. The page keeps a worker per REPL panel so a slow
// or runaway edit can be terminated without blocking the UI thread.

import init, { Session } from "./repl/soil_repl_wasm.js";

let prelude = "";
let session = null;
let booting = null;

async function boot() {
  if (session) return;
  if (!booting) {
    booting = init({ module_or_path: new URL("./repl/soil_repl_wasm_bg.wasm", import.meta.url) }).then(() => {
      session = new Session(prelude);
    });
  }
  await booting;
}

function errorText(error) {
  if (error && error.message) return error.message;
  return String(error);
}

self.onmessage = async (event) => {
  const message = event.data;
  if (message.type === "init") {
    prelude = message.prelude;
    try {
      await boot();
      postMessage({ type: "ready" });
    } catch (error) {
      postMessage({ type: "error", error: errorText(error) });
    }
    return;
  }
  if (message.type === "eval") {
    try {
      await boot();
      const result = session.eval(message.source);
      postMessage({ type: "result", result });
    } catch (error) {
      postMessage({ type: "result", result: `error: ${errorText(error)}` });
    }
  }
};
