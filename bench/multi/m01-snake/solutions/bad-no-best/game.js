(() => {
  const N = 20;
  const TICK = 150;
  const CELL = 20;
  const BEST_KEY = "snake-best";
  const DIRS = { up: [0, -1], down: [0, 1], left: [-1, 0], right: [1, 0] };
  const OPPOSITE = { up: "down", down: "up", left: "right", right: "left" };
  const KEYS = { ArrowUp: "up", ArrowDown: "down", ArrowLeft: "left", ArrowRight: "right" };
  const testMode = new URLSearchParams(location.search).has("test");
  const $ = (id) => document.getElementById(id);
  const ctx = $("board").getContext("2d");

  const s = {
    started: false,
    running: false,
    paused: false,
    over: false,
    score: 0,
    best: 0,
    dir: "right",
    next: "right",
    snake: [],
    food: { x: 0, y: 0 },
    acc: 0,
  };

  function loadBest() {
    const n = Number(localStorage.getItem(BEST_KEY));
    return Number.isFinite(n) && n > 0 ? n : 0;
  }

  function isFree(x, y) {
    return !s.snake.some((c) => c.x === x && c.y === y);
  }

  function newFood() {
    const cells = [];
    for (let y = 0; y < N; y++) for (let x = 0; x < N; x++) if (isFree(x, y)) cells.push({ x, y });
    s.food = cells[Math.floor(Math.random() * cells.length)];
  }

  function start() {
    Object.assign(s, { started: true, running: true, paused: false, over: false, score: 0 });
    s.dir = s.next = "right";
    s.acc = 0;
    s.snake = [{ x: 10, y: 10 }, { x: 9, y: 10 }, { x: 8, y: 10 }];
    newFood();
    document.activeElement?.blur();
    render();
  }

  function end() {
    s.running = false;
    s.over = true;
    if (s.score > s.best) {
      s.best = s.score;
      localStorage.setItem(BEST_KEY, String(s.best));
    }
  }

  function tick() {
    s.dir = s.next;
    const [dx, dy] = DIRS[s.dir];
    const head = { x: s.snake[0].x + dx, y: s.snake[0].y + dy };
    const eats = head.x === s.food.x && head.y === s.food.y;
    // The tail moves out of the way unless the snake is growing.
    const body = eats ? s.snake : s.snake.slice(0, -1);
    const offBoard = head.x < 0 || head.y < 0 || head.x >= N || head.y >= N;
    if (offBoard || body.some((c) => c.x === head.x && c.y === head.y)) {
      end();
      return;
    }
    s.snake = [head, ...body];
    if (eats) {
      s.score += 1;
      newFood();
    }
  }

  function step(ms) {
    if (!s.running || s.paused) return;
    s.acc += ms;
    while (s.running && s.acc >= TICK) {
      s.acc -= TICK;
      tick();
    }
    render();
  }

  function render() {
    $("score").textContent = String(s.score);
    $("best").textContent = String(s.best);
    $("start-screen").hidden = s.started;
    $("game-over").hidden = !s.over;
    $("paused").hidden = !s.paused;
    ctx.fillStyle = "#111";
    ctx.fillRect(0, 0, N * CELL, N * CELL);
    ctx.fillStyle = "#e33";
    ctx.fillRect(s.food.x * CELL, s.food.y * CELL, CELL, CELL);
    ctx.fillStyle = "#3c3";
    for (const c of s.snake) ctx.fillRect(c.x * CELL + 1, c.y * CELL + 1, CELL - 2, CELL - 2);
  }

  document.addEventListener("keydown", (e) => {
    if (e.code === "Space") {
      e.preventDefault();
      if (s.running) {
        s.paused = !s.paused;
        render();
      }
      return;
    }
    const d = KEYS[e.key];
    if (!d) return;
    e.preventDefault();
    if (s.running && d !== OPPOSITE[s.dir]) s.next = d;
  });

  $("start").addEventListener("click", start);
  $("restart").addEventListener("click", start);

  window.game = {
    step,
    state: () =>
      structuredClone({
        running: s.running,
        paused: s.paused,
        over: s.over,
        score: s.score,
        best: s.best,
        dir: s.dir,
        snake: s.snake,
        food: s.food,
      }),
    placeFood(x, y) {
      s.food = { x, y };
      render();
    },
  };

  if (!testMode) {
    let last = performance.now();
    const loop = (now) => {
      step(now - last);
      last = now;
      requestAnimationFrame(loop);
    };
    requestAnimationFrame(loop);
  }
  render();
})();
