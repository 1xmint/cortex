import subprocess, tempfile, os
def count(text):
    path = os.path.join(tempfile.mkdtemp(), "in.txt")
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)
    return subprocess.run(["python3", "-B", "/work/wordcount.py", path], capture_output=True, text=True, check=True).stdout.strip()
assert count("one two\nthree\n") == "2 3 14", count("one two\nthree\n")
assert count("") == "0 0 0", count("")
assert count("café  au\tlait\n\n") == "2 3 15", count("café  au\tlait\n\n")
