import subprocess, tempfile
d = tempfile.mkdtemp()
def todo(*args):
    return subprocess.run(["python3", "-B", "/work/todo.py", *args], cwd=d, capture_output=True, text=True)
assert todo("list").stdout.strip() == ""
assert todo("add", "buy milk").returncode == 0
assert todo("add", "write report").returncode == 0
assert todo("done", "2").returncode == 0
assert todo("list").stdout.splitlines() == ["1. [ ] buy milk", "2. [x] write report"], todo("list").stdout
r = todo("done", "7")
assert r.returncode == 1 and r.stderr.strip(), r
assert todo("list").stdout.splitlines() == ["1. [ ] buy milk", "2. [x] write report"]
