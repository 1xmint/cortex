import { withPage, check } from "./lib/cortex-check.mjs";
import { open, load, shape, board, press, undo, restart, moves } from "./common.mjs";

// Only the bottom row can slide. After Left, Up slides nothing.
const rows = [
  [8, 16, 8, 16],
  [16, 8, 16, 8],
  [8, 16, 8, 16],
  [0, 0, 32, 64],
];

await withPage(async (page, ctx) => {
  await open(page, ctx);
  const start = await board(page);
  await undo(page);
  check("undo before the first move does nothing", (await board(page)) === start && (await moves(page)) === "0");

  await load(page, ctx, rows);
  await press(page, "ArrowLeft");
  const left = await board(page);
  await press(page, "ArrowUp");
  check("Up slid nothing", (await board(page)) === left, await board(page));
  await undo(page);
  check("a press that slides nothing does not use up the undo", (await board(page)) === shape(rows), await board(page));
  await undo(page);
  check("a second undo does nothing", (await board(page)) === shape(rows), await board(page));

  await press(page, "ArrowLeft");
  const first = await board(page);
  await press(page, "ArrowRight");
  check("Right slid the bottom row", (await board(page)) !== first);
  await undo(page);
  check("undo takes back one move", (await board(page)) === first, await board(page));
  await undo(page);
  check("only one level", (await board(page)) === first, await board(page));

  await restart(page);
  const fresh = await board(page);
  await undo(page);
  check("undo right after New Game does nothing", (await board(page)) === fresh, await board(page));
});
