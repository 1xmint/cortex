import { withPage, check } from "./lib/cortex-check.mjs";
import { load, shape, board, press, undo, score, best, moves } from "./common.mjs";

const rows = [
  [2, 2, 0, 0],
  [0, 0, 0, 0],
  [0, 0, 0, 0],
  [0, 0, 0, 0],
];

await withPage(async (page, ctx) => {
  await load(page, ctx, rows);
  await press(page, "ArrowLeft");
  check("the merge scores 4", (await score(page)) === 4, String(await score(page)));
  check("the move counts", (await moves(page)) === "1", await moves(page));

  await undo(page);
  check("undo puts the board back", (await board(page)) === shape(rows), await board(page));
  check("undo puts the score back", (await score(page)) === 0, String(await score(page)));
  check("undo puts the move count back", (await moves(page)) === "0", await moves(page));
  check("undo keeps the best score", (await best(page)) === 4, String(await best(page)));
});
