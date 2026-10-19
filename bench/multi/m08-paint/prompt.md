Build a paint app in `index.html`. You may add other files next to it.

How it works:

- A `canvas` with id `canvas`, 400 pixels wide and 300 high, white when
  the page opens. Show it at exactly that size, with no border, padding or
  scaling, so the point x,y inside it is its top-left corner plus x,y.
- Pressing the mouse on the canvas, moving and letting go draws a stroke
  along the path the mouse took, with round ends. Pressing and letting go
  without moving draws a round dot. The stroke ends when the mouse is let
  go, even outside the canvas.
- Two tool buttons, ids `pen` and `eraser`. The pen draws in the chosen
  colour; the eraser paints white. The pen is chosen when the page opens.
- Five colour buttons with class `colour`, each with a `data-colour`
  attribute: `#000000`, `#ff0000`, `#00ff00`, `#0000ff` and `#ffff00`, in
  that order. Black is chosen when the page opens. Choosing a colour also
  chooses the pen.
- Three size buttons with class `size`, each with a `data-size`
  attribute: `2`, `8` and `20`, the width of the stroke in canvas pixels.
  `8` is chosen when the page opens. Sizes apply to the pen and the eraser.
- The chosen tool, colour and size each have the class `active`; the
  others in their group do not.
- **Undo and redo.** Buttons with ids `undo` and `redo`. Each stroke or
  dot is one step, and so is a clear. Undo takes back the last step and
  redo puts back the last undone one. A new stroke after an undo means
  there is nothing left to redo. Undo or redo with nothing to take back
  or put back does nothing. At least 20 steps can be undone.
- **Clear.** A button with id `clear` asks first, with the browser's
  `confirm()`. If the person agrees, the canvas goes back to white; if not,
  nothing changes.
- **Export.** A button with id `export` downloads the drawing as a PNG
  file named `drawing.png`: 400 by 300 pixels, exactly what the canvas
  shows, with the white background (not transparent).
