Build a Breakout game in `index.html`. You may add other files next to it.

The board is a `<canvas id="board" width="480" height="320">`, shown at that
size. Positions are in canvas pixels, with (0, 0) at the top left.

How it plays:

- Start screen: when the page loads, an element with id `start-screen` is
  visible and holds a button with id `start`. Clicking it starts level 1
  and hides the start screen.
- The paddle is 80 wide and 10 high, its top edge at y = 300. Each level
  starts with its left edge at x = 200. Holding the left or right arrow key
  moves it 0.5 pixels per millisecond. Moving the mouse over the board puts
  the paddle's centre under the mouse. The paddle never leaves the board.
- The ball has radius 5. Each level, and each new life, starts it at
  (240, 280) moving at (0.15, -0.3) pixels per millisecond, and puts the
  paddle back at x = 200.
- The ball bounces off the left, right and top walls. When it comes down
  onto the paddle it bounces straight back up: its vertical speed flips and
  its horizontal speed is unchanged.
- Bricks are 56 wide and 16 high, in rows of 8. Brick `col` (0 to 7) in row
  `row` has its top left at (4 + 60 × col, 40 + 20 × row). Level 1 has 4
  rows (32 bricks); level 2 has 5 rows (40 bricks).
- When the ball touches a brick, the brick disappears, the score goes up by
  10, and the ball bounces: hitting the bottom or top of a brick flips its
  vertical speed, hitting a side flips its horizontal speed.
- Clearing level 1 starts level 2 and keeps the score. Clearing level 2
  wins: an element with id `won` becomes visible.
- There are 3 lives. When the ball falls below the bottom of the board, a
  life is lost. Losing the last one ends the game: an element with id
  `lost` becomes visible.
- Elements with ids `score`, `lives` and `level` show those numbers.

Test hook (required). When the page is opened with `?test=1`, the game
never moves on its own; time passes only through these functions on
`window.game`:

- `step(ms)` advances the game by `ms` milliseconds.
- `state()` returns `{ screen, level, score, lives, paddle, ball, bricks }`:
  `screen` is `"start"`, `"playing"`, `"won"` or `"lost"`; `paddle` is
  `{ x, y, w, h }` (x and y are its top left); `ball` is
  `{ x, y, vx, vy, r }` (x and y are its centre, speeds in pixels per
  millisecond); and `bricks` lists the bricks still standing as
  `{ x, y, w, h }`.
- `setBall(x, y, vx, vy)` puts the ball at (x, y) with that speed.

Without `?test=1` the game runs in real time.
