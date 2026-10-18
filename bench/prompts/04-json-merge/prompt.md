Write `merge.py`. `python3 merge.py BASE.json OVERRIDE.json` prints the two
JSON objects merged: keys from OVERRIDE win, nested objects are merged the
same way at every depth, and anything else (lists included) is replaced
whole. Print it as JSON with sorted keys and an indent of 2.
