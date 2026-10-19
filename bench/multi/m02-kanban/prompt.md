Build a kanban board in `index.html`. You may add other files next to it.

How it works:

- Three columns, in this order: To do (`id="col-todo"`), Doing
  (`id="col-doing"`) and Done (`id="col-done"`). Each column holds an
  element with class `count` showing how many cards are in that column.
- A text box with id `new-card` and a button with id `add`. Clicking Add,
  or pressing Enter in the text box, adds a card with that text to the end
  of To do and clears the text box. Text is trimmed; empty or
  whitespace-only text adds nothing.
- Each card is an element with class `card` inside its column. It shows its
  text in an element with class `text`, and has buttons with classes
  `edit`, `delete`, `left` and `right`.
- `delete` removes the card. `left` and `right` move the card to the end of
  the column before or after it; they do nothing in the first or last
  column.
- `edit` replaces the text with a text box of class `edit-input` holding
  the current text. Enter saves the new text (trimmed); Escape cancels and
  keeps the old text; saving empty text keeps the old text too.
- Cards can be dragged with the mouse and dropped on another column, which
  moves them to the end of that column.
- A text box with id `filter`: while it has text, only cards whose text
  contains it (ignoring upper and lower case) are shown; the others are
  hidden. The counts always count every card, shown or not.
- The board (cards, their text, column and order) is saved in
  `localStorage`, so a reload shows the same board.
