Build a Snake game in `index.html`. You may add other files next to it.

How it plays:

- The board is a `<canvas id="board">` showing a 20 by 20 grid. Cells are
  numbered 0 to 19, left to right (x) and top to bottom (y).
- Start screen: when the page loads, an element with id `start-screen` is
  visible and holds a button with id `start`. Clicking it starts a game and
  hides the start screen.
- A new game's snake is 3 cells long: head at (10, 10), then (9, 10) and
  (8, 10), moving right. The score is 0.
- The snake moves one cell every 150 ms. The arrow keys steer it. A key that
  would turn it straight back into itself is ignored.
- Food sits on one free cell. When the head reaches it, the snake grows by
  one cell, the score goes up by one, and new food appears on a free cell.
- The game ends when the head leaves the board or runs into the snake's own
  body. Then an element with id `game-over` becomes visible.
- The score is shown in an element with id `score`. The best score so far
  is shown in an element with id `best` and kept in `localStorage`, so it
  survives a reload.
- Space pauses the game, and Space again resumes it.
- A button with id `restart`, visible when the game is over, starts a new
  game.

Test hook (required). When the page is opened with `?test=1`, the game
never moves on its own; time passes only through these functions on
`window.game`:

- `step(ms)` advances the game by `ms` milliseconds; `step(300)` moves the
  snake two cells.
- `state()` returns `{ running, paused, over, score, best, dir, snake, food }`:
  `running` is true while a game is in progress (started and not over),
  `paused` and `over` are true or false, `dir` is `"up"`, `"down"`,
  `"left"` or `"right"`, `snake` is the list of cells `{ x, y }` with the
  head first, and `food` is `{ x, y }`.
- `placeFood(x, y)` moves the food to that cell.

Without `?test=1` the game runs in real time.
