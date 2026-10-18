import os, subprocess, tempfile
path = os.path.join(tempfile.mkdtemp(), "data.csv")
with open(path, "w") as f:
    f.write('name,price,qty\n"Widget, large",10.5,3\ngadget,,7\nthing,2,1\nother,4.25,\n')
r = subprocess.run(["python3", "-B", "/work/csvstats.py", path, "price"], capture_output=True, text=True)
assert r.returncode == 0, r.stderr
assert r.stdout.split("\n")[:4] == ["count: 3", "min: 2.00", "max: 10.50", "mean: 5.58"], r.stdout
r = subprocess.run(["python3", "-B", "/work/csvstats.py", path, "missing"], capture_output=True, text=True)
assert r.returncode == 2, r.returncode
assert r.stderr.strip(), "no error message"
