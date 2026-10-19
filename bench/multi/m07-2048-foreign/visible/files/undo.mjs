import { withPage, check } from "./lib/cortex-check.mjs";
import { load, shape, board, grid, press, undo } from "./common.mjs";

const rows = [
  [2, 2, 0, 0],
  [0, 0, 0, 0],
  [0, 4, 0, 0],
  [0, 0, 0, 0],
];

await withPage(async (page, ctx) => {
  await load(page, ctx, rows);
  check("the saved game loads", (await board(page)) === shape(rows), await board(page));

  await press(page, "ArrowRight");
  const moved = await grid(page);
  check("the move slides and merges", moved[0][3] === 4 && moved[2][3] === 4, JSON.stringify(moved));

  await undo(page);
  check("undo puts the board back", (await board(page)) === shape(rows), await board(page));

  await press(page, "ArrowRight");
  check("the game carries on after undo", (await grid(page))[0][3] === 4, await board(page));
});
