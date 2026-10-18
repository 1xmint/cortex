Write `ratelimit.py` with a class `RateLimiter(limit, window, clock)`.
`allow()` returns True and records the call when fewer than `limit` calls
were allowed in the last `window` seconds, and returns False otherwise
(a refused call is not recorded). `clock` is a function returning the
current time in seconds; the class must use it, never the real time.
