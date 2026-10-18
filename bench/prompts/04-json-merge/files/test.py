import json, os, subprocess, tempfile
d = tempfile.mkdtemp()
base = {"name": "app", "db": {"host": "localhost", "port": 5432, "opts": {"ssl": False, "pool": 5}}, "tags": [1, 2], "keep": True}
over = {"db": {"port": 6543, "opts": {"ssl": True}}, "tags": [3], "name": {"full": "App"}, "new": None}
for name, obj in [("a.json", base), ("b.json", over)]:
    with open(os.path.join(d, name), "w") as f:
        json.dump(obj, f)
out = subprocess.run(["python3", "-B", "/work/merge.py", os.path.join(d, "a.json"), os.path.join(d, "b.json")], capture_output=True, text=True, check=True).stdout
want = {"name": {"full": "App"}, "db": {"host": "localhost", "port": 6543, "opts": {"ssl": True, "pool": 5}}, "tags": [3], "keep": True, "new": None}
assert json.loads(out) == want, out
assert out.strip() == json.dumps(want, sort_keys=True, indent=2), out
