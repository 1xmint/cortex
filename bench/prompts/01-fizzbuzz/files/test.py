import subprocess
out = subprocess.run(["python3", "-B", "/work/fizzbuzz.py", "15"], capture_output=True, text=True, check=True).stdout
want = "1 2 Fizz 4 Buzz Fizz 7 8 Fizz Buzz 11 Fizz 13 14 FizzBuzz".split()
assert out.split() == want, out
out = subprocess.run(["python3", "-B", "/work/fizzbuzz.py", "1"], capture_output=True, text=True, check=True).stdout
assert out.split() == ["1"], out
