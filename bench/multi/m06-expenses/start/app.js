(() => {
  const form = document.getElementById("add");
  const description = document.getElementById("description");
  const amount = document.getElementById("amount");
  const date = document.getElementById("date");
  const error = document.getElementById("error");
  const list = document.getElementById("list");
  const total = document.getElementById("total");
  const expenses = [];

  const today = () => new Date().toISOString().slice(0, 10);
  const cents = (value) => Math.round(Number(value) * 100);
  const money = (c) => (c / 100).toFixed(2);

  function fail(message) {
    error.textContent = message;
    error.hidden = false;
  }

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const text = description.value.trim();
    const value = cents(amount.value);
    if (!text) return fail("Say what the expense was.");
    if (!(value > 0)) return fail("The amount must be more than zero.");
    error.hidden = true;
    expenses.push({ description: text, cents: value, date: date.value || today() });
    form.reset();
    render();
  });

  function render() {
    const sorted = [...expenses].sort((a, b) => b.date.localeCompare(a.date));
    list.replaceChildren(
      ...sorted.map((e) => {
        const li = document.createElement("li");
        li.className = "expense";
        for (const [cls, text] of [["date", e.date], ["description", e.description], ["amount", money(e.cents)]]) {
          const span = document.createElement("span");
          span.className = cls;
          span.textContent = text;
          li.append(span);
        }
        return li;
      }),
    );
    total.textContent = money(expenses.reduce((sum, e) => sum + e.cents, 0));
  }

  render();
})();
