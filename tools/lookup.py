#!/usr/bin/env python3
"""Grep the CI-generated API digest. usage: lookup.py PATTERN [file]"""
import re, sys, glob
pat = re.compile(sys.argv[1], re.I)
files = sys.argv[2:] or ["docs/ci/win-foundation.txt", "docs/ci/egui.txt"]
for path in files:
    for line in open(path, encoding="utf-8", errors="replace"):
        if pat.search(line):
            print(f"{path.split('/')[-1][:6]}: {line.rstrip()[:600]}")
