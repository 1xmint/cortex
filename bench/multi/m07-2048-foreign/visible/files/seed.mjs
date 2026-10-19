import { withPage, check } from "./lib/cortex-check.mjs";
import { open, board, press, restart } from "./common.mjs";

const keys = ["ArrowLeft", "ArrowUp", "ArrowRight", "ArrowDown", "ArrowLeft", "ArrowUp"];

async function play(page) {
  for (const key of keys) await press(page, key);
  return board(page);
}

await withPage(async (page, ctx) => {
  await open(page, ctx, "?seed=7");
  const start = await board(page);
  const end = await play(page);
  check("the moves changed the board", end !== start, end);

  await restart(page);
  check("New Game starts the same seeded board", (await board(page)) === start, `${await board(page)} vs ${start}`);
  check("the same moves give the same board", (await play(page)) === end);

  await open(page, ctx, "?seed=8");
  await restart(page);
  const other = await board(page);
  const otherEnd = await play(page);
  check("a different seed gives a different game", other !== start || otherEnd !== end, `${other} ${otherEnd}`);
});
