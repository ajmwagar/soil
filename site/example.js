// SPDX-License-Identifier: MIT OR Apache-2.0
// Browser host for a Soil example page.
//
// The Soil view owns the DOM tree. This file only sends actions to the local
// example endpoint, swaps the returned view, and tells the landing page how
// tall the example is.

const root = document.getElementById("root");
if (root) {
  const name = root.dataset.example;
  let state = root.dataset.state || "null";

  function postHeight() {
    if (window.parent === window) return;
    const height = Math.ceil(document.documentElement.getBoundingClientRect().height);
    window.parent.postMessage({ type: "soil-example-height", height }, "*");
  }

  function note(text) {
    const node = document.createElement("p");
    node.className = "example-note";
    node.textContent = text;
    root.append(node);
  }

  function actionFor(target) {
    const payload = { op: target.dataset.op };
    for (const [key, value] of Object.entries(target.dataset)) {
      if (key !== "op") payload[key] = value;
    }
    if (target.tagName === "FORM") {
      for (const field of target.elements) {
        if (field.name) payload[field.name] = field.value;
      }
    }
    return payload;
  }

  async function run(action) {
    try {
      const response = await fetch("/__example", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ name, state, action }),
      });
      const data = await response.json();
      if (!response.ok) {
        note(data.error || "example failed");
        return;
      }
      state = JSON.stringify(data.state);
      root.innerHTML = data.html;
      postHeight();
    } catch (error) {
      note("Interactive preview needs the local site server.");
    }
  }

  document.addEventListener("click", (event) => {
    const target = event.target instanceof Element ? event.target.closest("[data-op]") : null;
    if (!target) return;
    if (target.tagName === "FORM") return;
    if (target.tagName === "BUTTON" && target.type === "submit" && target.form) {
      event.preventDefault();
      run(actionFor(target.form));
      return;
    }
    event.preventDefault();
    run(actionFor(target));
  });

  document.addEventListener("submit", (event) => {
    const form = event.target.closest("form[data-op]");
    if (!form) return;
    event.preventDefault();
    run(actionFor(form));
  });

  window.addEventListener("load", postHeight);
  window.addEventListener("resize", postHeight);
}
