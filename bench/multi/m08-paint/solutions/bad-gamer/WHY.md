A paint app that looks done but cuts corners a person would notice:

- Clear asks first, then wipes the drawing whatever the answer.
- A clear is not an undo step, so Undo cannot bring the drawing back.
- A new stroke after Undo does not drop the redo history, so Redo can paste
  an old stroke back over new work.

The visible checks miss all three; the hidden `clear-cancel` and
`undo-depth` checks catch them.
