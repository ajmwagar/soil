// SPDX-License-Identifier: MIT OR Apache-2.0
// Browser host for a Soil example page.
//
// The Soil view owns the DOM tree. This file sends actions to the local
// example endpoint, swaps the returned view, and tells the landing page how
// tall the example is.
//
// When the endpoint is unavailable (a static export, GitHub Pages, a stale
// tab), each example has a small browser fallback so the demo still works.
//
// Policy: the server path is the real Soil `update`/`view`. The JS fallbacks
// are deliberately kept only for static hosting; when examples can run through
// the wasm evaluator, delete the fallback functions instead of growing them.

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

  function escapeHtml(value) {
    return String(value == null ? "" : value).replace(/[&<>"]/g, (char) => ({
      "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;",
    })[char]);
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

  // The counter is small enough to keep working without the evaluator.
  function counter(action) {
    const current = Number(JSON.parse(state)) || 0;
    let next = current;
    if (action.op === "inc") next = current + 1;
    else if (action.op === "dec") next = current - 1;
    else if (action.op === "reset") next = 0;
    else return false;
    state = JSON.stringify(next);
    const output = root.querySelector("output");
    if (output) output.textContent = String(next);
    return true;
  }

  function todoHtml(data) {
    const items = data.items || [];
    const remaining = items.filter((item) => !item.done).length;
    const done = items.length - remaining;
    const rows = items.map((item) => {
      const id = escapeHtml(item.id);
      const checked = item.done ? " done" : "";
      return `<li class="todo-item" data-key="${id}">` +
        `<button class="toggle${checked}" data-op="toggle" data-id="${id}" title="${item.done ? "Mark active" : "Mark done"}">${item.done ? "✓" : "○"}</button>` +
        `<span class="todo-text${checked}">${escapeHtml(item.text)}</span>` +
        `<button class="icon-btn danger" data-op="delete" data-id="${id}" title="Delete">×</button>` +
        `</li>`;
    }).join("");
    return `<div class="app"><main class="stage"><section class="card todo-card">` +
      `<form class="row add-row" data-op="add"><input class="input" name="text" placeholder="What needs doing?" autocomplete="off"><button class="btn primary" type="submit">Add</button></form>` +
      (items.length ? `<ul class="todo-list">${rows}</ul>` : `<p class="empty">Nothing here yet. Add something above.</p>`) +
      `<footer class="todo-footer"><span class="muted">${remaining} remaining · ${done} done</span>` +
      `<button class="btn ghost" data-op="clear"${done === 0 ? " disabled" : ""}>Clear completed</button></footer>` +
      `</section></main></div>`;
  }

  function todo(action) {
    const data = JSON.parse(state);
    const items = data.items || [];
    if (action.op === "add") {
      const text = String(action.text || "");
      if (!text.length) return true;
      items.push({ id: data.next, text, done: false });
      data.next += 1;
    } else if (action.op === "toggle") {
      for (const item of items) {
        if (String(item.id) === String(action.id)) item.done = !item.done;
      }
    } else if (action.op === "delete") {
      data.items = items.filter((item) => String(item.id) !== String(action.id));
    } else if (action.op === "clear") {
      data.items = items.filter((item) => !item.done);
    } else {
      return false;
    }
    state = JSON.stringify(data);
    root.innerHTML = todoHtml(data);
    return true;
  }

  const calcInitial = { display: "0", acc: 0, op: null, fresh: true };

  function calcValue(data) {
    return data.display === "ERR" ? 0 : Number(data.display);
  }

  function calcOp(op, acc, value) {
    if (op === "+") return acc + value;
    if (op === "-") return acc - value;
    if (op === "*") return acc * value;
    if (op === "/") return value === 0 ? null : acc / value;
    return value;
  }

  function calcDigit(data, value) {
    if (data.display === "ERR") return { display: value, acc: 0, op: null, fresh: false };
    if (data.fresh) return { display: value, acc: data.acc, op: data.op, fresh: false };
    if (data.display === "0") return { display: value, acc: data.acc, op: data.op, fresh: false };
    return { display: data.display + value, acc: data.acc, op: data.op, fresh: false };
  }

  function calcChoose(data, op) {
    const current = calcValue(data);
    if (data.op == null) return { display: String(current), acc: current, op, fresh: true };
    const result = calcOp(data.op, data.acc, current);
    if (result == null) return { display: "ERR", acc: 0, op: null, fresh: true };
    return { display: String(result), acc: result, op, fresh: true };
  }

  function calcEquals(data) {
    const current = calcValue(data);
    if (data.op == null) return { display: String(current), acc: current, op: null, fresh: true };
    const result = calcOp(data.op, data.acc, current);
    if (result == null) return { display: "ERR", acc: 0, op: null, fresh: true };
    return { display: String(result), acc: result, op: null, fresh: true };
  }

  function calculator(action) {
    let data = JSON.parse(state);
    if (action.op === "digit") data = calcDigit(data, String(action.value || "0"));
    else if (action.op === "op") data = calcChoose(data, String(action.value || "+"));
    else if (action.op === "equals") data = calcEquals(data);
    else if (action.op === "negate") {
      const current = calcValue(data);
      data = { display: String(-current), acc: -current, op: data.op, fresh: false };
    } else if (action.op === "clear") data = { ...calcInitial };
    else return false;
    state = JSON.stringify(data);
    const output = root.querySelector(".display");
    if (output) output.textContent = data.display;
    return true;
  }

  const lines = [[0,1,2],[3,4,5],[6,7,8],[0,3,6],[1,4,7],[2,5,8],[0,4,8],[2,4,6]];

  function winner(board) {
    for (const [a, b, c] of lines) {
      if (board[a] && board[a] === board[b] && board[a] === board[c]) return board[a];
    }
    return null;
  }

  function full(board) {
    return board.every((cell) => cell !== null);
  }

  function winningMoveFor(board, player) {
    for (let index = 0; index < 9; index += 1) {
      if (board[index] !== null) continue;
      const next = board.slice();
      next[index] = player;
      if (winner(next) === player) return index;
    }
    return null;
  }

  function firstEmptyOf(board, indices) {
    return indices.find((index) => board[index] === null) ?? null;
  }

  function firstEmpty(board) {
    return board.findIndex((cell) => cell === null);
  }

  function computerMove(board) {
    return winningMoveFor(board, "O") ??
      winningMoveFor(board, "X") ??
      (board[4] === null ? 4 : firstEmptyOf(board, [0, 2, 6, 8]) ?? firstEmpty(board));
  }

  function computerReply(data) {
    if (data.mode === "1p" && data.turn === "O" && data.winner == null) {
      const move = computerMove(data.board);
      if (move == null) return data;
      const next = data.board.slice();
      next[move] = "O";
      return { board: next, turn: "X", winner: winner(next), mode: data.mode };
    }
    return data;
  }

  function play(data, index) {
    if (data.winner != null || data.board[index] != null) return data;
    if (data.mode === "1p" && data.turn === "O") return data;
    const next = data.board.slice();
    next[index] = data.turn;
    return computerReply({
      board: next,
      turn: data.turn === "X" ? "O" : "X",
      winner: winner(next),
      mode: data.mode,
    });
  }

  function tttHtml(data) {
    const status = data.winner != null ? `${data.winner} wins` :
      full(data.board) ? "Draw" :
      data.mode === "1p" && data.turn === "O" ? "Computer to move" :
      data.mode === "1p" ? "Your move" : `${data.turn} to move`;
    const cells = data.board.map((mark, index) => {
      const disabled = mark != null || data.winner != null;
      return `<button class="cell${mark ? ` ${mark}` : ""}" data-key="${index}" data-op="play" data-index="${index}"${disabled ? " disabled" : ""}>${mark || ""}</button>`;
    }).join("");
    return `<div class="app">` +
      `<div class="row mode-row">` +
      `<button class="btn ${data.mode === "1p" ? "primary" : "ghost"}" data-op="mode" data-mode="1p">1 player</button>` +
      `<button class="btn ${data.mode === "2p" ? "primary" : "ghost"}" data-op="mode" data-mode="2p">2 players</button>` +
      `</div><p class="status">${status}</p><div class="board">${cells}</div>` +
      `<div class="row"><button class="btn ghost" data-op="reset">New game</button></div></div>`;
  }

  function tictactoe(action) {
    let data = JSON.parse(state);
    if (action.op === "play") data = play(data, Number(action.index || 0));
    else if (action.op === "mode") {
      data = { board: Array(9).fill(null), turn: "X", winner: null, mode: action.mode || "1p" };
    } else if (action.op === "reset") {
      data = { board: Array(9).fill(null), turn: "X", winner: null, mode: data.mode };
    } else return false;
    state = JSON.stringify(data);
    root.innerHTML = tttHtml(data);
    return true;
  }

  const panels = {
    overview: ["Overview", "One state value selects a panel. The active tab is just a string."],
    activity: ["Activity", "The same view function renders a different subtree without a router."],
    settings: ["Settings", "Switch tabs in the source panel and press Apply to see the state reset."],
  };

  function tabs(action) {
    if (action.op !== "tab") return false;
    const active = panels[action.tab] ? action.tab : "overview";
    const buttons = Object.keys(panels).map((id) =>
      `<button class="tab${id === active ? " active" : ""}" data-op="tab" data-tab="${id}">${panels[id][0]}</button>`
    ).join("");
    const panel = panels[active];
    root.innerHTML = `<div class="app"><div class="tabs">${buttons}</div>` +
      `<section class="panel"><h2>${panel[0]}</h2><p>${panel[1]}</p></section></div>`;
    state = JSON.stringify({ active });
    return true;
  }

  function formHtml(data) {
    if (data.sent) {
      return `<div class="app"><main class="stage"><section class="card form-card">` +
        `<h2 class="title">Thanks, ${escapeHtml(data.name)}</h2>` +
        `<p class="body">We would write to ${escapeHtml(data.email)} if this form had a host.</p>` +
        `<button class="btn ghost" data-op="again">Submit another</button></section></main></div>`;
    }
    return `<div class="app"><main class="stage"><section class="card form-card">` +
      `<h2 class="title">Join the soil</h2>` +
      (data.error ? `<p class="error">${escapeHtml(data.error)}</p>` : "") +
      `<form class="stack" data-op="submit">` +
      `<label class="field"><span class="field-label">Name</span><input class="input" name="name" placeholder="Ada Lovelace" autocomplete="off" value="${escapeHtml(data.name)}"></label>` +
      `<label class="field"><span class="field-label">Email</span><input class="input" name="email" placeholder="ada@example.com" autocomplete="off" value="${escapeHtml(data.email)}"></label>` +
      `<button class="btn primary" type="submit">Submit</button></form></section></main></div>`;
  }

  function form(action) {
    let data = JSON.parse(state);
    if (action.op === "submit") {
      const name = String(action.name || "");
      const email = String(action.email || "");
      if (!name.length) data = { name, email, error: "Name is required.", sent: false };
      else if (!email.includes("@")) data = { name, email, error: "Email needs an @.", sent: false };
      else data = { name, email, error: null, sent: true };
    } else if (action.op === "again") {
      data = { name: "", email: "", error: null, sent: false };
    } else return false;
    state = JSON.stringify(data);
    root.innerHTML = formHtml(data);
    return true;
  }

  function fallback(action) {
    let handled = false;
    if (name === "counter") handled = counter(action);
    else if (name === "todo") handled = todo(action);
    else if (name === "calculator") handled = calculator(action);
    else if (name === "tictactoe") handled = tictactoe(action);
    else if (name === "tabs") handled = tabs(action);
    else if (name === "form") handled = form(action);
    if (handled) postHeight();
    return handled;
  }

  async function run(action) {
    if (name === "counter") {
      counter(action);
      postHeight();
      return;
    }
    try {
      const response = await fetch("/__example", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ name, state, action }),
      });
      const data = await response.json();
      if (!response.ok) {
        if (!fallback(action)) note(data.error || "example failed");
        return;
      }
      state = JSON.stringify(data.state);
      root.innerHTML = data.html;
      postHeight();
    } catch (error) {
      if (!fallback(action)) note("Interactive preview needs the local site server.");
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
