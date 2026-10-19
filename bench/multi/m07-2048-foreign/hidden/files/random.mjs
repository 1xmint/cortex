import { withPage, check } from "./lib/cortex-check.mjs";
import { open, board, restart, press, moves } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  const seen = new Set([await board(page)]);
  const counts = [];
  for (let i = 0; i < 4; i++) {
    await press(page, "ArrowLeft");
    await press(page, "ArrowUp");
    await restart(page);
    seen.add(await board(page));
    counts.push(await moves(page));
  }
  check("without a seed, new games differ", seen.size >= 2, [...seen].join(" | "));
  check("each new game starts at 0 moves", counts.every((n) => n === "0"), counts.join(","));
});
