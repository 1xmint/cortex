(() => {
  const CATEGORIES = ["food", "transport", "home", "fun", "other"];
  const COLOURS = ["#e76f51", "#2a9d8f", "#264653", "#e9c46a", "#8d99ae"];
  const STORE = "expenses";
  const form = document.getElementById("add");
  const description = document.getElementById("description");
  const amount = document.getElementById("amount");
  const date = document.getElementById("date");
  const category = document.getElementById("category");
  const save = document.getElementById("save");
  const error = document.getElementById("error");
  const list = document.getElementById("list");
  const total = document.getElementById("total");
  const month = document.getElementById("month");
  const monthTotal = document.getElementById("month-total");
  const chart = document.getElementById("chart");
  let expenses = load();
  let editing = null;
  let nextId = expenses.reduce((m, e) => Math.max(m, e.id + 1), 1);

  const pad = (n) => String(n).padStart(2, "0");
  const now = new Date();
  const today = () => `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
  const cents = (value) => Math.round(Number(value) * 100);
  const money = (c) => (c / 100).toFixed(2);

  function load() {
    try {
      const saved = JSON.parse(localStorage.getItem(STORE));
      return Array.isArray(saved) ? saved : [];
    } catch {
      return [];
    }
  }

  function store() {
    try {
      localStorage.setItem(STORE, JSON.stringify(expenses));
    } catch {
      // Storage may be unavailable; the list then lasts this visit only.
    }
  }

  function fail(message) {
    error.textContent = message;
    error.hidden = false;
  }

  function stopEditing() {
    editing = null;
    save.textContent = "Add";
    form.reset();
    category.value = "food";
  }

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const text = description.value.trim();
    const value = cents(amount.value);
    if (!text) return fail("Say what the expense was.");
    if (!(value > 0)) return fail("The amount must be more than zero.");
    error.hidden = true;
    const fields = { description: text, cents: value, date: date.value || today(), category: category.value };
    const existing = expenses.find((e) => e.id === editing);
    if (existing) Object.assign(existing, fields);
    else expenses.push({ id: nextId++, ...fields });
    stopEditing();
    store();
    render();
  });

  function edit(e) {
    editing = e.id;
    description.value = e.description;
    amount.value = money(e.cents);
    date.value = e.date;
    category.value = e.category;
    save.textContent = "Save";
    description.focus();
  }

  function remove(e) {
    expenses = expenses.filter((x) => x !== e);
    if (editing === e.id) stopEditing();
    store();
    render();
  }

  const sorted = () => [...expenses].sort((a, b) => b.date.localeCompare(a.date));

  function csvField(text) {
    return /[",\n\r]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
  }

  document.getElementById("export").addEventListener("click", () => {
    const lines = ["date,description,category,amount"];
    for (const e of sorted()) {
      lines.push([e.date, csvField(e.description), e.category, money(e.cents)].join(","));
    }
    const blob = new Blob([lines.join("\n") + "\n"], { type: "text/csv" });
    const link = document.createElement("a");
    link.href = URL.createObjectURL(blob);
    link.download = "expenses.csv";
    document.body.append(link);
    link.click();
    link.remove();
    setTimeout(() => URL.revokeObjectURL(link.href), 1000);
  });

  month.addEventListener("input", render);

  function drawChart(totals) {
    const ctx = chart.getContext("2d");
    ctx.clearRect(0, 0, chart.width, chart.height);
    const top = Math.max(...totals, 1);
    const width = chart.width / CATEGORIES.length;
    CATEGORIES.forEach((name, i) => {
      const height = (totals[i] / top) * (chart.height - 20);
      ctx.fillStyle = COLOURS[i];
      ctx.fillRect(i * width + 8, chart.height - 16 - height, width - 16, height);
      ctx.fillStyle = "#222";
      ctx.font = "11px sans-serif";
      ctx.fillText(name, i * width + 8, chart.height - 4);
      chart.setAttribute(`data-total-${name}`, money(totals[i]));
    });
  }

  function render() {
    list.replaceChildren(
      ...sorted().map((e) => {
        const li = document.createElement("li");
        li.className = "expense";
        for (const [cls, text] of [
          ["date", e.date],
          ["description", e.description],
          ["category", e.category],
          ["amount", money(e.cents)],
        ]) {
          const span = document.createElement("span");
          span.className = cls;
          span.textContent = text;
          li.append(span);
        }
        for (const [cls, label, act] of [["edit", "Edit", edit], ["delete", "Delete", remove]]) {
          const b = document.createElement("button");
          b.type = "button";
          b.className = cls;
          b.textContent = label;
          b.addEventListener("click", () => act(e));
          li.append(b);
        }
        return li;
      }),
    );
    total.textContent = money(expenses.reduce((sum, e) => sum + e.cents, 0));
    const inMonth = expenses.filter((e) => e.date.slice(0, 7) === month.value);
    monthTotal.textContent = money(inMonth.reduce((sum, e) => sum + e.cents, 0));
    drawChart(CATEGORIES.map((c) => inMonth.filter((e) => e.category === c).reduce((sum, e) => sum + e.cents, 0)));
  }

  month.value = today().slice(0, 7);
  render();
})();
