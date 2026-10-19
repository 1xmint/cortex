A paint app that looks done but cuts corners a person would notice:

- Clear wipes the drawing without asking first.
- A clear is not an undo step, so Undo cannot bring the drawing back.
- A new stroke after Undo does not drop the redo history, so Redo can paste
  an old stroke back over new work.

The visible checks miss the last two; the hidden `clear-cancel` and
`undo-depth` checks catch them, and the visible `clear` check catches the
missing question.
