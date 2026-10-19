`index.html` and `game.js` are a working memory-match game. Add to it
without breaking what already works.

What it does now, which must keep working:

- An element with id `board` holds the cards, each a button with class
  `card`. Clicking a card turns it face up (class `flipped`, showing its
  letter). When two cards are face up, a matching pair stays face up with
  the class `matched`; two different cards turn face down again after
  about a second, and clicks are ignored until they do.
- When every pair is found, the element with id `won` becomes visible.
- With `?test=1` in the address, the cards are not shuffled: cards 1 and
  2 are a pair, then cards 3 and 4, and so on in page order. Keep this
  test mode working for every new size.

What to add:

- **Moves.** An element with id `moves` shows how many turns have been
  taken: each time a second card is turned face up counts as one move,
  whether or not it matched. It starts at `0`.
- **Timer.** An element with id `time` shows the whole seconds since the
  first card of this game was turned, rounded down. It shows `0` until
  then, and stops counting when the game is won.
- **Sizes.** A `select` with id `size` and three options, with values
  `easy` (8 cards in 4 columns), `medium` (16 cards in 4 columns, the
  default) and `hard` (36 cards in 6 columns). Choosing a size starts a
  new game of that size.
- **Best score.** An element with id `best` shows the fewest moves anyone
  has needed to win at the chosen size, or `-` if no game at that size
  has been won. Each size keeps its own best, and the bests survive a
  reload (use `localStorage`).
- **Restart.** A button with id `restart` starts a new game of the same
  size: moves and time go back to `0`, and every card is face down.
