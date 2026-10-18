Write `toc.py`. `python3 toc.py FILE` prints a table of contents for a
Markdown file: one line per `##` heading as `- [Title](#slug)`, and per
`###` heading the same but indented by two spaces. `#` headings and
headings inside ``` code blocks are skipped. The slug is the title in
lower case, with every character that is not a letter, digit, space or
hyphen removed, then spaces turned into hyphens.
