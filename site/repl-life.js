// SPDX-License-Identifier: MIT OR Apache-2.0
// Turn the Game of Life text output into a small animated canvas. The output
// is still in the DOM; the Text button reveals it.
const FRAME_MS = 240;
const ALIVE = new Set(["█", "■", "O", "o"]);
const DEAD = new Set(["·", ".", " "]);

function decodeStringLiteral(text) {
  const trimmed = text.trim();
  if (!trimmed.startsWith('"') || !trimmed.endsWith('"')) return text;
  try {
    return JSON.parse(trimmed);
  } catch {
    return trimmed
      .slice(1, -1)
      .replace(/\\\\/g, "\\")
      .replace(/\\n/g, "\n")
      .replace(/\\r/g, "\r")
      .replace(/\\t/g, "\t")
      .replace(/\\"/g, '"');
  }
}

export function parseLifeOutput(text) {
  const boardText = decodeStringLiteral(String(text));
  const frames = boardText.trim().split(/\n[ \t]*\n+/).map((frame) =>
    frame.split("\n").map((line) => line.replace(/\r$/, ""))
  );
  if (!frames.length) return null;
  const height = frames[0].length;
  const width = frames[0][0]?.length || 0;
  if (width < 3 || height < 3) return null;
  const boards = [];
  for (const rows of frames) {
    if (rows.length !== height) return null;
    const board = [];
    for (const row of rows) {
      if (row.length !== width) return null;
      const cells = [];
      for (const char of row) {
        if (ALIVE.has(char)) cells.push(true);
        else if (DEAD.has(char)) cells.push(false);
        else return null;
      }
      board.push(cells);
    }
    boards.push(board);
  }
  return { width, height, frames: boards };
}

function element(name, className, text) {
  const node = document.createElement(name);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

export function mountLifeCanvas(panel, output, result) {
  const life = parseLifeOutput(result);
  if (!life) return null;

  const oldHidden = output.hidden;
  const oldAria = output.getAttribute("aria-hidden");
  const root = element("div", "repl-visual");
  const head = element("div", "repl-visual-head");
  const label = element("span", "repl-visual-label", "Canvas");
  const meta = element("span", "repl-visual-meta");
  const actions = element("div", "repl-visual-actions");
  const play = element("button", "", "Pause");
  const text = element("button", "", "Text");
  const canvas = element("canvas", "repl-canvas");
  const range = document.createElement("input");

  play.type = "button";
  text.type = "button";
  play.setAttribute("aria-pressed", "true");
  canvas.setAttribute("role", "img");
  range.type = "range";
  range.className = "repl-visual-range";
  range.min = "0";
  range.max = String(life.frames.length - 1);
  range.value = "0";
  range.setAttribute("aria-label", "Generation");
  if (life.frames.length < 2) {
    play.disabled = true;
    range.disabled = true;
  }

  actions.append(play, text);
  head.append(label, meta, actions);
  canvas.style.aspectRatio = `${life.width} / ${life.height}`;
  root.append(head, canvas, range);

  output.hidden = true;
  output.setAttribute("aria-hidden", "true");
  output.after(root);

  const colors = getComputedStyle(panel);
  const color = (name, fallback) => colors.getPropertyValue(name).trim() || fallback;
  const background = color("--color-bg", "#14120f");
  const line = color("--color-line", "#3a342d");
  const accent = color("--color-accent", "#d77b52");
  const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const frameDelay = Math.max(80, Math.min(FRAME_MS, 2000 / life.frames.length));
  let frame = 0;
  let playing = life.frames.length > 1 && !reduceMotion;
  let timer = null;
  let resizeObserver = null;

  function draw() {
    const context = canvas.getContext("2d");
    if (!context) return;
    const rect = canvas.getBoundingClientRect();
    const width = Math.max(220, Math.min(rect.width || 420, 560));
    const height = width * life.height / life.width;
    const scale = Math.min(window.devicePixelRatio || 1, 2);
    if (canvas.width !== Math.round(width * scale) || canvas.height !== Math.round(height * scale)) {
      canvas.width = Math.round(width * scale);
      canvas.height = Math.round(height * scale);
    }
    context.setTransform(scale, 0, 0, scale, 0, 0);
    context.clearRect(0, 0, width, height);

    const pad = Math.max(10, Math.min(width, height) * 0.04);
    const cell = Math.min((width - pad * 2) / life.width, (height - pad * 2) / life.height);
    const boardWidth = cell * life.width;
    const boardHeight = cell * life.height;
    const left = (width - boardWidth) / 2;
    const top = (height - boardHeight) / 2;
    const board = life.frames[frame];

    context.fillStyle = background;
    context.fillRect(0, 0, width, height);

    context.strokeStyle = line;
    context.globalAlpha = 0.55;
    context.lineWidth = 1;
    for (let x = 0; x <= life.width; x += 1) {
      const at = left + x * cell + 0.5;
      context.beginPath();
      context.moveTo(at, top);
      context.lineTo(at, top + boardHeight);
      context.stroke();
    }
    for (let y = 0; y <= life.height; y += 1) {
      const at = top + y * cell + 0.5;
      context.beginPath();
      context.moveTo(left, at);
      context.lineTo(left + boardWidth, at);
      context.stroke();
    }

    context.globalAlpha = 1;
    context.fillStyle = accent;
    const inset = Math.max(0.75, cell * 0.08);
    for (let row = 0; row < life.height; row += 1) {
      for (let col = 0; col < life.width; col += 1) {
        if (!board[row][col]) continue;
        context.fillRect(
          left + col * cell + inset,
          top + row * cell + inset,
          Math.max(1, cell - inset * 2),
          Math.max(1, cell - inset * 2)
        );
      }
    }

    context.globalAlpha = 1;
    context.strokeStyle = line;
    context.strokeRect(left + 0.5, top + 0.5, boardWidth, boardHeight);
  }

  function update() {
    meta.textContent = `generation ${frame} / ${life.frames.length - 1}`;
    range.value = String(frame);
    canvas.setAttribute(
      "aria-label",
      `Game of Life board, generation ${frame} of ${life.frames.length - 1}`
    );
    draw();
  }

  function setPlaying(next) {
    playing = next && life.frames.length > 1;
    play.textContent = playing ? "Pause" : "Play";
    play.setAttribute("aria-pressed", String(playing));
    if (timer) clearInterval(timer);
    timer = null;
    if (!playing) return;
    timer = setInterval(() => {
      frame = (frame + 1) % life.frames.length;
      update();
    }, frameDelay);
  }

  play.addEventListener("click", () => setPlaying(!playing));
  range.addEventListener("input", () => {
    setPlaying(false);
    frame = Number(range.value);
    update();
  });
  text.addEventListener("click", () => {
    const show = output.hidden;
    output.hidden = !show;
    if (show) output.removeAttribute("aria-hidden");
    else output.setAttribute("aria-hidden", "true");
    text.textContent = show ? "Hide text" : "Text";
  });

  if ("ResizeObserver" in window) {
    resizeObserver = new ResizeObserver(draw);
    resizeObserver.observe(root);
  } else {
    window.addEventListener("resize", draw);
  }

  setPlaying(playing);
  update();

  return {
    node: root,
    destroy() {
      if (timer) clearInterval(timer);
      if (resizeObserver) resizeObserver.disconnect();
      else window.removeEventListener("resize", draw);
      root.remove();
      output.hidden = oldHidden;
      if (oldAria === null) output.removeAttribute("aria-hidden");
      else output.setAttribute("aria-hidden", oldAria);
    },
  };
}
