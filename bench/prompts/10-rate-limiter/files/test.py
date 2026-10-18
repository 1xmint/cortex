import sys
sys.path.insert(0, "/work")
from ratelimit import RateLimiter
now = [1000.0]
r = RateLimiter(3, 10, lambda: now[0])
assert [r.allow() for _ in range(4)] == [True, True, True, False]
now[0] = 1009.9
assert r.allow() is False
now[0] = 1010.0
assert r.allow() is True
assert r.allow() is True and r.allow() is True and r.allow() is False
now[0] = 2000.0
assert r.allow() is True
