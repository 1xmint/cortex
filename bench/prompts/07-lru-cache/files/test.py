import sys
sys.path.insert(0, "/work")
from lru import LRUCache
c = LRUCache(2)
c.put("a", 1)
c.put("b", 2)
assert c.get("a") == 1
c.put("c", 3)
assert c.get("b") is None
assert c.get("a") == 1 and c.get("c") == 3
c.put("a", 10)
c.put("d", 4)
assert c.get("c") is None
assert c.get("a") == 10 and c.get("d") == 4
one = LRUCache(1)
one.put(1, "x")
one.put(2, "y")
assert one.get(1) is None and one.get(2) == "y"
