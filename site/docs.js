// SPDX-License-Identifier: MIT OR Apache-2.0
// Filter the docs sidebar and the current page's entries.
const filter = document.querySelector(".docs-filter");
if (filter) {
  const side = Array.from(document.querySelectorAll(".docs-side a:not(.docs-side-home)"));
  const entries = Array.from(document.querySelectorAll(".doc-entry"));
  const toc = Array.from(document.querySelectorAll(".docs-toc li"));
  const pages = Array.from(document.querySelectorAll(".docs-pages li"));
  const empty = document.querySelector(".docs-no-matches");

  function apply() {
    const query = filter.value.trim().toLowerCase();
    let visible = 0;
    for (const node of [...side, ...entries, ...toc, ...pages]) {
      const match = !query || node.textContent.toLowerCase().includes(query);
      node.hidden = !match;
      if (match && (entries.includes(node) || pages.includes(node))) visible += 1;
    }
    if (empty) empty.hidden = !query || visible > 0;
  }

  filter.addEventListener("input", apply);
  filter.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      filter.value = "";
      apply();
      filter.blur();
    }
  });
}
