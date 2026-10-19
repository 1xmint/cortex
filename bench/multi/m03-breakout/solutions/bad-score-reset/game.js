(() => {
  const W = 480;
  const H = 320;
  const R = 5;
  const PADDLE = { w: 80, h: 10, y: 300, speed: 0.5 };
  const ROWS = { 1: 4, 2: 5 };
  const testMode = new URLSearchParams(location.search).has("test");
  const $ = (id) => document.getElementById(id);
  const canvas = $("board");
  const ctx = canvas.getContext("2d");
  const held = new Set();

  const s = {
    screen: "start",
    level: 1,
    score: 0,
    lives: 3,
    paddle: { x: 200, y: PADDLE.y, w: PADDLE.w, h: PADDLE.h },
    ball: { x: 240, y: 280, vx: 0.15, vy: -0.3, r: R },
    bricks: [],
    acc: 0,
  };

  const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));

  function serve() {
    s.paddle.x = 200;
    s.ball = { x: 240, y: 280, vx: 0.15, vy: -0.3, r: R };
  }

  function startLevel(level) {
    s.level = level;
    s.bricks = [];
    for (let row = 0; row < ROWS[level]; row++)
      for (let col = 0; col < 8; col++) s.bricks.push({ x: 4 + 60 * col, y: 40 + 20 * row, w: 56, h: 16 });
    serve();
  }

  function start() {
    Object.assign(s, { screen: "playing", score: 0, lives: 3, acc: 0 });
    startLevel(1);
    document.activeElement?.blur();
    render();
  }

  function touches(b, br) {
    const nx = clamp(b.x, br.x, br.x + br.w);
    const ny = clamp(b.y, br.y, br.y + br.h);
    return (b.x - nx) ** 2 + (b.y - ny) ** 2 <= b.r ** 2;
  }

  // One millisecond of play.
  function tick() {
    const p = s.paddle;
    const b = s.ball;
    if (held.has("ArrowLeft")) p.x -= PADDLE.speed;
    if (held.has("ArrowRight")) p.x += PADDLE.speed;
    p.x = clamp(p.x, 0, W - p.w);

    b.x += b.vx;
    b.y += b.vy;
    if (b.x - b.r <= 0) b.vx = Math.abs(b.vx);
    if (b.x + b.r >= W) b.vx = -Math.abs(b.vx);
    if (b.y - b.r <= 0) b.vy = Math.abs(b.vy);

    if (b.vy > 0 && b.y + b.r >= p.y && b.y <= p.y + p.h && b.x >= p.x && b.x <= p.x + p.w) {
      b.vy = -Math.abs(b.vy);
    }

    const i = s.bricks.findIndex((br) => touches(b, br));
    if (i >= 0) {
      const br = s.bricks[i];
      s.bricks.splice(i, 1);
      s.score += 10;
      if (b.x >= br.x && b.x <= br.x + br.w) b.vy = -b.vy;
      else b.vx = -b.vx;
      if (s.bricks.length === 0) {
        if (s.level === 1) {
          startLevel(2);
          s.score = 0;
        }
        else s.screen = "won";
        return;
      }
    }

    if (b.y - b.r > H) {
      s.lives -= 1;
      if (s.lives === 0) s.screen = "lost";
      else serve();
    }
  }

  function step(ms) {
    if (s.screen !== "playing") return;
    s.acc += ms;
    while (s.screen === "playing" && s.acc >= 1) {
      s.acc -= 1;
      tick();
    }
    render();
  }

  function render() {
    $("score").textContent = String(s.score);
    $("lives").textContent = String(s.lives);
    $("level").textContent = String(s.level);
    $("start-screen").hidden = s.screen !== "start";
    $("won").hidden = s.screen !== "won";
    $("lost").hidden = s.screen !== "lost";
    ctx.fillStyle = "#111";
    ctx.fillRect(0, 0, W, H);
    ctx.fillStyle = "#e93";
    for (const br of s.bricks) ctx.fillRect(br.x, br.y, br.w, br.h);
    ctx.fillStyle = "#ddd";
    ctx.fillRect(s.paddle.x, s.paddle.y, s.paddle.w, s.paddle.h);
    ctx.beginPath();
    ctx.arc(s.ball.x, s.ball.y, s.ball.r, 0, Math.PI * 2);
    ctx.fill();
  }

  document.addEventListener("keydown", (e) => {
    if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      e.preventDefault();
      held.add(e.key);
    }
  });
  document.addEventListener("keyup", (e) => held.delete(e.key));
  canvas.addEventListener("mousemove", (e) => {
    const rect = canvas.getBoundingClientRect();
    const x = ((e.clientX - rect.left) * W) / rect.width;
    s.paddle.x = clamp(x - s.paddle.w / 2, 0, W - s.paddle.w);
    render();
  });
  $("start").addEventListener("click", start);

  window.game = {
    step,
    state: () =>
      structuredClone({
        screen: s.screen,
        level: s.level,
        score: s.score,
        lives: s.lives,
        paddle: s.paddle,
        ball: s.ball,
        bricks: s.bricks,
      }),
    setBall(x, y, vx, vy) {
      s.ball = { x, y, vx, vy, r: R };
      render();
    },
  };

  if (!testMode) {
    let last = performance.now();
    const loop = (now) => {
      step(Math.min(now - last, 50));
      last = now;
      requestAnimationFrame(loop);
    };
    requestAnimationFrame(loop);
  }
  render();
})();
