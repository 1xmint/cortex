import sys
sys.path.insert(0, "/work")
from roman import to_roman, from_roman
assert to_roman(2026) == "MMXXVI"
assert to_roman(1994) == "MCMXCIV"
assert from_roman("MCMXCIV") == 1994
for n in range(1, 4000):
    assert from_roman(to_roman(n)) == n, n
for bad in [0, 4000, -1]:
    try:
        to_roman(bad)
        raise SystemExit("to_roman(%r) did not raise" % bad)
    except ValueError:
        pass
for bad in ["", "IIII", "IC", "ABC", "MMMM", "VX"]:
    try:
        from_roman(bad)
        raise SystemExit("from_roman(%r) did not raise" % bad)
    except ValueError:
        pass
