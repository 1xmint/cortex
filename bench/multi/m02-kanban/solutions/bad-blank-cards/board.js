(() => {
  const COLS = ["todo", "doing", "done"];
  const KEY = "kanban-cards";
  const $ = (id) => document.getElementById(id);

  let cards = load();
  let editing = null;
  let nextId = cards.reduce((m, c) => Math.max(m, c.id), 0) + 1;

  function load() {
    try {
      const saved = JSON.parse(localStorage.getItem(KEY));
      if (Array.isArray(saved)) return saved.filter((c) => COLS.includes(c.col) && typeof c.text === "string");
    } catch {
      // A broken save starts an empty board.
    }
    return [];
  }

  function save() {
    localStorage.setItem(KEY, JSON.stringify(cards));
  }

  function change(fn) {
    fn();
    save();
    render();
  }

  // Moves a card to the end of `col`.
  function moveTo(card, col) {
    cards = cards.filter((c) => c !== card);
    card.col = col;
    cards.push(card);
  }

  function add() {
    const text = $("new-card").value.trim();
    $("new-card").value = "";
    change(() => cards.push({ id: nextId++, text, col: "todo" }));
  }

  function button(cls, label, onClick) {
    const b = document.createElement("button");
    b.className = cls;
    b.textContent = label;
    b.addEventListener("click", onClick);
    return b;
  }

  function cardEl(card) {
    const el = document.createElement("div");
    el.className = "card";
    el.draggable = true;
    el.addEventListener("dragstart", (e) => {
      e.dataTransfer.setData("text/plain", String(card.id));
    });
    const filter = $("filter").value.trim().toLowerCase();
    el.hidden = filter !== "" && !card.text.toLowerCase().includes(filter);

    if (editing === card.id) {
      const input = document.createElement("input");
      input.className = "edit-input";
      input.value = card.text;
      input.addEventListener("keydown", (e) => {
        if (e.key === "Enter") {
          const text = input.value.trim();
          editing = null;
          change(() => {
            if (text) card.text = text;
          });
        } else if (e.key === "Escape") {
          editing = null;
          render();
        }
      });
      el.append(input);
      queueMicrotask(() => input.focus());
    } else {
      const text = document.createElement("span");
      text.className = "text";
      text.textContent = card.text;
      el.append(text, " ");
    }
    const i = COLS.indexOf(card.col);
    el.append(
      button("edit", "Edit", () => {
        editing = card.id;
        render();
      }),
      button("delete", "Delete", () => change(() => (cards = cards.filter((c) => c !== card)))),
      button("left", "←", () => i > 0 && change(() => moveTo(card, COLS[i - 1]))),
      button("right", "→", () => i < COLS.length - 1 && change(() => moveTo(card, COLS[i + 1]))),
    );
    return el;
  }

  function render() {
    for (const col of COLS) {
      const section = $(`col-${col}`);
      const inCol = cards.filter((c) => c.col === col);
      section.querySelector(".count").textContent = String(inCol.length);
      section.querySelector(".cards").replaceChildren(...inCol.map(cardEl));
    }
  }

  for (const col of COLS) {
    const section = $(`col-${col}`);
    section.addEventListener("dragover", (e) => {
      e.preventDefault();
      section.classList.add("over");
    });
    section.addEventListener("dragleave", () => section.classList.remove("over"));
    section.addEventListener("drop", (e) => {
      e.preventDefault();
      section.classList.remove("over");
      const card = cards.find((c) => String(c.id) === e.dataTransfer.getData("text/plain"));
      if (card) change(() => moveTo(card, col));
    });
  }

  $("add").addEventListener("click", add);
  $("new-card").addEventListener("keydown", (e) => {
    if (e.key === "Enter") add();
  });
  $("filter").addEventListener("input", render);
  render();
})();
