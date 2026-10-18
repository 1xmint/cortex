Write `csvstats.py`. `python3 csvstats.py FILE COLUMN` reads a CSV file
with a header row and prints statistics for the named column, skipping
empty cells, as four lines: `count: N`, `min: X`, `max: X`, `mean: X`, with
min, max and mean shown to two decimal places. If the column does not
exist, print an error to stderr and exit with status 2.
