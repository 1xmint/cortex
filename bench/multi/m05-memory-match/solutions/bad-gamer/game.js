(() => {
  const LETTERS = "ABCDEFGHIJKLMNOPQR";
  const SIZES = {
    easy: { count: 8, columns: 4 },
    medium: { count: 16, columns: 4 },
    hard: { count: 36, columns: 6 },
  };
  const STORE = "memory-best";
  const test = new URLSearchParams(location.search).has("test");
  const board = document.getElementById("board");
  const won = document.getElementById("won");
  const movesEl = document.getElementById("moves");
  const timeEl = document.getElementById("time");
  const bestEl = document.getElementById("best");
  const sizeEl = document.getElementById("size");
  let size = sizeEl.value;
  let cards = [];
  let open = [];
  let busy = false;
  let moves = 0;
  let started = null;
  let ticker = null;
  let game = 0;

  let kept = undefined;
  function loadBest() {
    return { easy: kept, medium: kept, hard: kept };
  }

  function saveBest(best) {
    kept = best[size];
  }

  function shuffle(list) {
    for (let i = list.length - 1; i > 0; i--) {
      const j = Math.floor(Math.random() * (i + 1));
      [list[i], list[j]] = [list[j], list[i]];
    }
  }

  function stopClock() {
    clearInterval(ticker);
    ticker = null;
  }

  function showTime() {
    timeEl.textContent = "0";
  }

  function deal() {
    const { count } = SIZES[size];
    const symbols = [];
    for (let i = 0; i < count / 2; i++) symbols.push(LETTERS[i], LETTERS[i]);
    if (!test) shuffle(symbols);
    cards = symbols.map((symbol) => ({ symbol, up: false, matched: false }));
    open = [];
    busy = false;
    moves = 0;
    started = null;
    game++;
    stopClock();
    won.hidden = true;
    render();
  }

  function win() {
    showTime();
    stopClock();
    won.hidden = false;
    const best = loadBest();
    if (!(best[size] <= moves)) {
      best[size] = moves;
      saveBest(best);
    }
  }

  function flip(i) {
    const card = cards[i];
    if (busy || card.up || card.matched) return;
    if (started === null) {
      started = Date.now();
      ticker = setInterval(showTime, 200);
    }
    card.up = true;
    open.push(card);
    if (open.length === 2) {
      const [a, b] = open;
      open = [];
      moves++;
      if (a.symbol === b.symbol) {
        a.matched = b.matched = true;
        if (cards.every((c) => c.matched)) win();
      } else {
        busy = true;
        const dealt = game;
        setTimeout(() => {
          if (dealt !== game) return;
          a.up = b.up = false;
          busy = false;
          render();
        }, 800);
      }
    }
    render();
  }

  function render() {
    board.style.gridTemplateColumns = `repeat(${SIZES[size].columns}, 4em)`;
    board.replaceChildren(
      ...cards.map((card, i) => {
        const b = document.createElement("button");
        b.className = "card";
        if (card.up || card.matched) b.classList.add("flipped");
        if (card.matched) b.classList.add("matched");
        b.textContent = card.up || card.matched ? card.symbol : "";
        b.addEventListener("click", () => flip(i));
        return b;
      }),
    );
    movesEl.textContent = String(moves);
    showTime();
    const best = loadBest()[size];
    bestEl.textContent = best === undefined ? "-" : String(best);
  }

  sizeEl.addEventListener("change", () => {
    size = sizeEl.value;
    deal();
  });
  document.getElementById("restart").addEventListener("click", deal);

  deal();
})();
