This folder is the open-source game 2048 by Gabriele Cirulli (MIT
licence; see `LICENSE` and `SOURCE.md`). It was not written for this task.
Add to it without breaking what already works.

What it does now, which must keep working:

- The arrow keys slide every tile; two equal tiles that meet merge into
  one and add its value to the score. A move that slides nothing does
  nothing, and no new tile appears.
- Each tile is drawn as an element with the classes `tile`,
  `tile-<value>` and `tile-position-<column>-<row>`, counting from 1 at
  the top left.
- The score is shown in `.score-container` and the best score in
  `.best-container`. The best score survives a reload.
- The game in progress is saved in `localStorage` under the key
  `gameState` and comes back after a reload. Keep that key and its
  format: a game saved by the original version must still load. You may
  add fields to it.
- Making a 2048 tile shows "You win!" with a "Keep going" button
  (`.keep-playing-button`) that lets the player carry on.
- "New Game" (`.restart-button`) starts a new game.

What to add:

- **Seeded games.** With `?seed=N` in the address, where N is a whole
  number, the starting tiles and every new tile (where it appears, and
  whether it is a 2 or a 4) come from a random number generator seeded
  with N. The same seed and the same moves always give the same board,
  and different seeds give different games. Every new game on that page,
  whether the first one or one started with "New Game", starts again from
  N. Without `?seed`, the game stays random.
- **Undo.** A button with id `undo` takes back the last move that slid
  tiles: the board, the score and the move count all go back to just
  before it. There is one level only: a second undo in a row does
  nothing, and so does undo before the first move of a game. A key press
  that slides nothing is not a move and does not use up the undo. Undo
  never lowers the best score. After an undo, the saved game is the
  undone one.
- **Moves.** An element with id `moves` shows just the number of moves
  that slid tiles in this game. It is `0` at the start and after "New
  Game", and it is saved with the game. A saved game that has no move
  count starts at `0`.
- **Animations switch.** A checkbox with id `animations`, checked at
  first. While it is unchecked, tiles slide, appear and merge with no CSS
  transition or animation on `.tile` or `.tile-inner`. Checking it again
  brings them back. The choice survives a reload.
