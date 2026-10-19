(() => {
  const LETTERS = "ABCDEFGHIJKLMNOPQR";
  const COLUMNS = 4;
  const COUNT = 16;
  const test = new URLSearchParams(location.search).has("test");
  const board = document.getElementById("board");
  const won = document.getElementById("won");
  let cards = [];
  let open = [];
  let busy = false;

  function shuffle(list) {
    for (let i = list.length - 1; i > 0; i--) {
      const j = Math.floor(Math.random() * (i + 1));
      [list[i], list[j]] = [list[j], list[i]];
    }
  }

  function deal() {
    const symbols = [];
    for (let i = 0; i < COUNT / 2; i++) symbols.push(LETTERS[i], LETTERS[i]);
    if (!test) shuffle(symbols);
    cards = symbols.map((symbol) => ({ symbol, up: false, matched: false }));
    open = [];
    busy = false;
    won.hidden = true;
    render();
  }

  function flip(i) {
    const card = cards[i];
    if (busy || card.up || card.matched) return;
    card.up = true;
    open.push(card);
    if (open.length === 2) {
      const [a, b] = open;
      open = [];
      if (a.symbol === b.symbol) {
        a.matched = b.matched = true;
        if (cards.every((c) => c.matched)) won.hidden = false;
      } else {
        busy = true;
        setTimeout(() => {
          a.up = b.up = false;
          busy = false;
          render();
        }, 800);
      }
    }
    render();
  }

  function render() {
    board.style.gridTemplateColumns = `repeat(${COLUMNS}, 4em)`;
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
  }

  deal();
})();
