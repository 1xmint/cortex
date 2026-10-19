Undo puts the tiles back but leaves the score and move count where
they were, and the animations switch forgets its setting on reload.
Every visible check passes; the hidden undo-score, undo-saved and
animations-saved checks catch it.
