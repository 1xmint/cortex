import { withPage, check } from "./lib/cortex-check.mjs";
import { open, startGame, state, step, food, text, TICK } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await startGame(page);
  await food(page, 11, 10);
  await step(page, TICK);
  const s = await state(page);
  check("eating grows the snake to 4 cells", s.snake.length === 4, String(s.snake.length));
  check("eating adds one to the score", s.score === 1, String(s.score));
  check("score element shows 1", (await text(page, "#score")) === "1");
  const onSnake = s.snake.some((c) => c.x === s.food.x && c.y === s.food.y);
  const onBoard = s.food.x >= 0 && s.food.x < 20 && s.food.y >= 0 && s.food.y < 20;
  check("new food is on a free cell", onBoard && !onSnake, JSON.stringify(s.food));
});
