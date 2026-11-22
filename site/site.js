// SPDX-License-Identifier: MIT OR Apache-2.0
// Keep embedded Soil examples as tall as their content.
addEventListener("message", (event) => {
  if (!event.data || event.data.type !== "soil-example-height") return;
  for (const frame of document.querySelectorAll("iframe")) {
    if (frame.contentWindow !== event.source) continue;
    const height = Math.max(160, Math.min(920, Number(event.data.height) || 0));
    frame.style.height = `${height}px`;
    break;
  }
});

// Switch each example between its live preview and its source.
document.addEventListener("click", (event) => {
  const button = event.target instanceof Element ? event.target.closest("[data-work-tab]") : null;
  if (!button) return;
  const work = button.closest(".work");
  if (!work) return;
  const tab = button.dataset.workTab;
  for (const chip of work.querySelectorAll("[data-work-tab]")) {
    const active = chip === button;
    chip.classList.toggle("is-active", active);
    chip.setAttribute("aria-selected", active ? "true" : "false");
  }
  for (const panel of work.querySelectorAll("[data-work-panel]")) {
    const active = panel.dataset.workPanel === tab;
    panel.classList.toggle("is-active", active);
    panel.hidden = !active;
  }
});

// Highlight the source drawers without paying for highlighting at build time.
const SOIL_TOKENS = /(;[^\n]*)|("(?:\\.|[^"\\])*")|(:[^\s()";]+)|(#t|#f|\bnil\b)|(-?\b\d+(?:\.\d+)?\b)|(\b(?:defun|defmacro|defspore|let|if|cond|and|or|not|quote|lambda|fn|do|begin|match)\b)|([()])|([^\s()";]+)/g;

function escapeHtml(text) {
  return text.replace(/[&<>]/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[char]);
}

function highlightSoil(code) {
  let out = "";
  let last = 0;
  let match;
  while ((match = SOIL_TOKENS.exec(code))) {
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

for (const block of document.querySelectorAll(".soil-source")) {
  const code = block.textContent;
  if (code) block.innerHTML = highlightSoil(code);
}
