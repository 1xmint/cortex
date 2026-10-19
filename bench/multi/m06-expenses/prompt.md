`index.html` and `app.js` are a small expense tracker. Add to it without
breaking what already works.

What it does now, which must keep working:

- A form with id `add` has a text box with id `description`, a number box
  with id `amount`, a date box with id `date` and a submit button with id
  `save`. Submitting adds an expense and clears the form. An empty date
  means today.
- Empty descriptions (after trimming) and amounts that are not more than
  zero are refused: nothing is added and the element with id `error` shows
  a message. The next good submit hides it again.
- The list with id `list` shows one element with class `expense` per
  expense, newest date first (expenses on the same date stay in the order
  they were added). Inside each are elements with classes `date`
  (`YYYY-MM-DD`), `description` and `amount` (two decimals, like `12.50`).
- The element with id `total` shows the sum of every expense with two
  decimals.

What to add:

- **Categories.** A `select` with id `category` in the form, with the
  values `food`, `transport`, `home`, `fun` and `other` (`food` first and
  chosen by default). Each expense keeps its category and shows it in an
  element with class `category`.
- **Delete.** Each expense has a button with class `delete` that removes
  it.
- **Edit.** Each expense has a button with class `edit`. It puts that
  expense's description, amount, date and category into the form; the
  next good submit changes that expense instead of adding a new one, and
  the form goes back to adding. The same rules about empty descriptions
  and amounts apply.
- **Monthly totals.** A month box (`input type="month"`) with id `month`,
  set to the current month when the page opens. The element with id
  `month-total` shows the sum of the expenses in that month, with two
  decimals.
- **Chart.** A `canvas` with id `chart` draws the chosen month's total per
  category as a chart. The canvas also carries one attribute per category,
  `data-total-food`, `data-total-transport`, `data-total-home`,
  `data-total-fun` and `data-total-other`, each holding that category's
  total for the chosen month with two decimals (`0.00` when there is
  none). The chart and the attributes update whenever the expenses or the
  month change.
- **CSV export.** A button with id `export` downloads a file named
  `expenses.csv`. Its first line is `date,description,category,amount`,
  then one line per expense in the same order as the list, with the
  amount in two decimals. Follow the usual CSV rule for text that holds a
  comma or a double quote: wrap it in double quotes and double any double
  quote inside.
- **Saved.** The expenses are kept in `localStorage`, so a reload shows
  the same list, categories and totals.
