import os, subprocess, tempfile
path = os.path.join(tempfile.mkdtemp(), "doc.md")
with open(path, "w") as f:
    f.write("# Project\n\n## Getting Started\ntext\n### Install (v2)!\n```\n## not a heading\n```\n## FAQ & Help\n#### deep\n")
out = subprocess.run(["python3", "-B", "/work/toc.py", path], capture_output=True, text=True, check=True).stdout
want = ["- [Getting Started](#getting-started)", "  - [Install (v2)!](#install-v2)", "- [FAQ & Help](#faq--help)"]
assert out.splitlines() == want, out
