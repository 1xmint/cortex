Everything works on screen, but the canvas is never painted white: it is
transparent, and the page behind it happens to be white. Clear empties it to
transparent too.

The visible checks treat a transparent pixel as blank, so they pass. The
exported PNG has a transparent background instead of the white one the
prompt asks for, which the hidden `export-content` and `blank-white` checks
catch.
