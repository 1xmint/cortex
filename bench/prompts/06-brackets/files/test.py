import sys
sys.path.insert(0, "/work")
from brackets import is_balanced
for good in ["", "()", "([]{})", "a(b[c]{d}e)f", "{[()()]}"]:
    assert is_balanced(good) is True, good
for bad in ["(", ")", "(]", "([)]", "{{}", "}{", "a)b(c"]:
    assert is_balanced(bad) is False, bad
