(() => {
  const LIMIT = 10000;
  const $ = (id) => document.getElementById(id);
  let questions = [];
  let screen = "loading";
  let order = [];
  let at = 0;
  let score = 0;
  let given = [];
  let deadline = 0;

  function shuffled(list) {
    const out = [...list];
    for (let i = out.length - 1; i > 0; i--) {
      const j = Math.floor(Math.random() * (i + 1));
      [out[i], out[j]] = [out[j], out[i]];
    }
    return out;
  }

  function begin(list) {
    order = list;
    at = 0;
    score = 0;
    given = [];
    screen = "playing";
    deadline = Date.now() + LIMIT;
    document.activeElement?.blur();
    render();
  }

  function answer(choice) {
    if (screen !== "playing") return;
    const q = order[at];
    given.push(choice);
    if (choice === q.answer) score += 1;
    at += 1;
    if (at === order.length) {
      screen = "results";
      document.activeElement?.blur();
    } else {
      deadline = Date.now() + LIMIT;
    }
    render();
  }

  function render() {
    $("start").hidden = screen !== "start";
    $("quiz").hidden = screen !== "playing";
    $("results").hidden = screen !== "results";
    $("score").textContent = String(score);
    if (screen === "playing") {
      const q = order[at];
      $("question").textContent = q.question;
      $("choices").replaceChildren(
        ...q.choices.map((text, i) => {
          const b = document.createElement("button");
          b.className = "choice";
          b.textContent = text;
          b.addEventListener("click", () => answer(i));
          return b;
        }),
      );
      showTimer();
    }
    if (screen === "results") {
      $("final-score").textContent = `${score} / ${order.length}`;
      $("review").replaceChildren(
        ...order.map((q, i) => {
          const li = document.createElement("li");
          li.className = given[i] === q.answer ? "right" : "wrong";
          li.textContent = `${q.question} Answer: ${q.choices[q.answer]}`;
          return li;
        }),
      );
    }
  }

  function showTimer() {
    const left = Math.max(0, deadline - Date.now());
    $("timer").textContent = String(Math.ceil(left / 1000));
  }

  setInterval(() => {
    if (screen !== "playing") return;
    showTimer();
  }, 100);

  document.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      if (screen === "start") begin(questions);
      else if (screen === "results") begin(shuffled(questions));
    } else if (screen === "playing" && ["1", "2", "3", "4"].includes(e.key)) {
      answer(Number(e.key) - 1);
    }
  });
  $("start").addEventListener("click", () => begin(questions));
  $("restart").addEventListener("click", () => begin(shuffled(questions)));

  fetch("questions.json")
    .then((r) => r.json())
    .then((list) => {
      questions = list;
      screen = "start";
      render();
    });
})();
