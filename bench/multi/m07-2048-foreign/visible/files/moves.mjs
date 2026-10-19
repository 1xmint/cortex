import { withPage, check } from "./lib/cortex-check.mjs";
import { open, load, press, moves, restart } from "./common.mjs";

// Left and Up slide nothing on this board.
const stuck = [
  [2, 4, 0, 0],
  [4, 2, 0, 0],
  [0, 0, 0, 0],
  [0, 0, 0, 0],
];

await withPage(async (page, ctx) => {
  await open(page, ctx);
  check("a new game shows 0 moves", (await moves(page)) === "0", await moves(page));

  await load(page, ctx, stuck);
  check("a save with no move count starts at 0", (await moves(page)) === "0", await moves(page));

  await press(page, "ArrowLeft");
  await press(page, "ArrowUp");
  check("key presses that slide nothing are not moves", (await moves(page)) === "0", await moves(page));

  await press(page, "ArrowRight");
  check("a real move counts", (await moves(page)) === "1", await moves(page));

  await restart(page);
  check("New Game resets the count", (await moves(page)) === "0", await moves(page));
});
