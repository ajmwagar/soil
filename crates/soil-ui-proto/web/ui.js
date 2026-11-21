// SPDX-License-Identifier: MIT OR Apache-2.0
// The entire host-side UI implementation for the examples.
//
// Lisp emits a tree of {tag, attrs, children} and {text} records plus a CSS
// string. This file mounts the tree, patches it in place, injects the CSS, and
// sends named actions back to the server. It has no knowledge of any example.

const root = document.getElementById("root");
const source = document.getElementById("source");
const status = document.getElementById("status");
const apply = document.getElementById("apply");
const examples = document.getElementById("examples");
const filename = document.getElementById("filename");

let mounted = null;
let currentState = null;
let currentName = null;
let styleElement = null;

function setStatus(text, isError = false) {
  status.textContent = text;
  status.classList.toggle("error", isError);
}

function mount(vnode) {
  if (vnode == null) return null;
  if (typeof vnode === "string") return document.createTextNode(vnode);
  if (Object.prototype.hasOwnProperty.call(vnode, "text")) {
    return document.createTextNode(vnode.text ?? "");
  }
  const element = document.createElement(vnode.tag || "div");
  element.__vnode = vnode;
  applyAttrs(element, vnode.attrs || {});
  for (const child of vnode.children || []) {
    const node = mount(child);
    if (node) element.appendChild(node);
  }
  return element;
}

function patch(dom, vnode) {
  if (!dom) return mount(vnode);
  if (vnode == null) {
    dom.remove();
    return null;
  }
  if (typeof vnode === "string" || Object.prototype.hasOwnProperty.call(vnode, "text")) {
    const text = typeof vnode === "string" ? vnode : (vnode.text ?? "");
    if (dom.nodeType === Node.TEXT_NODE) {
      if (dom.data !== text) dom.data = text;
      return dom;
    }
    const next = document.createTextNode(text);
    dom.replaceWith(next);
    return next;
  }
  if (dom.nodeType !== Node.ELEMENT_NODE || !dom.__vnode || dom.__vnode.tag !== vnode.tag) {
    const next = mount(vnode);
    dom.replaceWith(next);
    return next;
  }
  applyAttrs(dom, vnode.attrs || {}, dom.__vnode.attrs || {});
  reconcileChildren(dom, vnode.children || []);
  dom.__vnode = vnode;
  return dom;
}

function applyAttrs(element, next, previous = {}) {
  for (const name of Object.keys(previous)) {
    if (!(name in next)) element.removeAttribute(name);
  }
  for (const [name, value] of Object.entries(next)) {
    if (value === false || value == null) {
      element.removeAttribute(name);
      continue;
    }
    const text = String(value);
    if (element.getAttribute(name) !== text) element.setAttribute(name, text);
  }
}

function reconcileChildren(parent, children) {
  const previous = Array.from(parent.childNodes);
  const byKey = new Map();
  previous.forEach((node, index) => {
    const key = node.nodeType === Node.ELEMENT_NODE && node.getAttribute("data-key");
    byKey.set(key || `i:${index}`, node);
  });

  const keep = new Set();
  children.forEach((vnode, index) => {
    const key = vnode && vnode.attrs && vnode.attrs["data-key"];
    const existing = byKey.get(key || `i:${index}`);
    const next = existing ? patch(existing, vnode) : mount(vnode);
    if (!next) return;
    keep.add(next);
    const target = parent.childNodes[index] || null;
    if (target !== next) parent.insertBefore(next, target);
  });

  for (const node of previous) {
    if (!keep.has(node) && node.parentNode === parent) node.remove();
  }
}

function renderTree(tree) {
  if (!mounted) {
    mounted = mount(tree);
    root.replaceChildren(mounted);
    return;
  }
  mounted = patch(mounted, tree);
  if (!mounted) root.replaceChildren();
  else if (mounted.parentNode !== root) root.replaceChildren(mounted);
}

function applyCss(css) {
  if (!styleElement) {
    styleElement = document.createElement("style");
    styleElement.id = "soil-css";
    document.head.appendChild(styleElement);
  }
  if (styleElement.textContent !== css) styleElement.textContent = css;
}

function sourceUrl() {
  if (!currentName) return "/source";
  return `/source?name=${encodeURIComponent(currentName)}`;
}

function viewUrl(action) {
  const params = new URLSearchParams();
  if (currentName) params.set("name", currentName);
  if (currentState !== null) params.set("state", JSON.stringify(currentState));
  if (action) params.set("action", action);
  return `/view?${params.toString()}`;
}

function actionFromElement(element) {
  if (!element.dataset || !element.dataset.op) return null;
  const payload = { op: element.dataset.op };
  for (const [key, value] of Object.entries(element.dataset)) {
    if (key !== "op") payload[key] = value;
  }
  return JSON.stringify(payload);
}

async function render(action = "") {
  if (!currentName) return;
  const response = await fetch(viewUrl(action), { cache: "no-store" });
  const data = await response.json();
  if (!response.ok) {
    setStatus(data.error || "render failed", true);
    return;
  }
  currentState = data.state;
  applyCss(data.css);
  renderTree(data.tree);
  setStatus(`state ${JSON.stringify(currentState)}`);
}

async function loadSource() {
  const response = await fetch(sourceUrl(), { cache: "no-store" });
  if (!response.ok) {
    setStatus("could not load source", true);
    return;
  }
  source.value = await response.text();
}

async function applySource() {
  setStatus("applying…");
  const response = await fetch(sourceUrl(), {
    method: "POST",
    body: source.value,
    headers: { "Content-Type": "text/plain; charset=utf-8" },
  });
  if (!response.ok) {
    const data = await response.json().catch(() => null);
    setStatus((data && data.error) || "apply failed", true);
    return;
  }
  await render();
}

async function loadExamples() {
  const response = await fetch("/examples", { cache: "no-store" });
  if (!response.ok) {
    setStatus("could not load examples", true);
    return;
  }
  const list = await response.json();
  examples.replaceChildren();
  for (const example of list) {
    const option = document.createElement("option");
    option.value = example.name;
    option.textContent = example.title;
    examples.appendChild(option);
  }
}

async function selectExample(name) {
  currentName = name;
  currentState = null;
  mounted = null;
  root.replaceChildren();
  filename.textContent = `${name}.soil`;
  await loadSource();
  await render();
}

function runAction(action) {
  render(action).catch((error) => setStatus(String(error), true));
}

root.addEventListener("click", (event) => {
  const target = event.target instanceof Element ? event.target.closest("[data-op]") : null;
  if (!target) return;
  if (target.tagName === "FORM") return;
  if (target.tagName === "BUTTON" && target.type === "submit" && target.form) return;
  event.preventDefault();
  const action = actionFromElement(target);
  if (action) runAction(action);
});

root.addEventListener("submit", (event) => {
  const form = event.target instanceof HTMLFormElement ? event.target : null;
  if (!form || !form.dataset.op) return;
  event.preventDefault();
  const payload = {
    op: form.dataset.op,
    ...Object.fromEntries(new FormData(form)),
  };
  render(JSON.stringify(payload))
    .then(() => {
      if (form.isConnected) form.reset();
    })
    .catch((error) => setStatus(String(error), true));
});

examples.addEventListener("change", () => {
  selectExample(examples.value).catch((error) => setStatus(String(error), true));
});

apply.addEventListener("click", () => {
  applySource().catch((error) => setStatus(String(error), true));
});

source.addEventListener("keydown", (event) => {
  if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
    event.preventDefault();
    applySource().catch((error) => setStatus(String(error), true));
  }
});

(async () => {
  try {
    await loadExamples();
    const initial = examples.value || "counter";
    await selectExample(initial);
  } catch (error) {
    setStatus(String(error), true);
  }
})();
