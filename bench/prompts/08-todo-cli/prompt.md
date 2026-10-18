Write `todo.py`, a todo list that keeps its items in `todo.json` in the
current folder, so they survive between runs. Commands:
`python3 todo.py add TEXT` adds an item; `python3 todo.py list` prints
each item as `N. [ ] TEXT`, or `N. [x] TEXT` once done, numbered from 1;
`python3 todo.py done N` marks item N done. `done` with a number that does
not exist prints an error to stderr and exits with status 1.
