#!/usr/bin/env python3
"""Build a compact, greppable API digest from downloaded crate sources.

usage: build_digest.py <crate-dir> <out-file> <win32 path relative to src/Windows/Win32, e.g. System/Services> ...
"""
import os
import re
import sys


def squeeze(t: str, n: int = 400) -> str:
    return re.sub(r"\s+", " ", t).strip()[:n]


def balanced(src: str, start: int, open_ch: str, close_ch: str) -> int:
    depth = 0
    j = start
    while j < len(src):
        if src[j] == open_ch:
            depth += 1
        elif src[j] == close_ch:
            depth -= 1
            if depth == 0:
                return j
        j += 1
    return len(src) - 1


def digest_file(path: str) -> list[str]:
    src = open(path, encoding="utf-8", errors="replace").read()
    out = []
    fn_re = re.compile(r"fn ([A-Za-z0-9_]+)\s*\(")
    for m in fn_re.finditer(src):
        name = m.group(1)
        i = m.end() - 1
        j = balanced(src, i, "(", ")")
        end = j + 1
        k = src.find(";", end)
        if k != -1 and k - end < 40:
            end = k + 1
        out.append("FN  " + squeeze(src[m.start():end], 460))
    for m in re.finditer(r"pub const [A-Z0-9_]+(?:\s*:\s*[^=;]+)?\s*=[^;]*;", src):
        out.append("CST " + squeeze(m.group(0), 220))
    for m in re.finditer(r"pub (?:struct|union) [A-Za-z0-9_]+", src):
        brace = src.find("{", m.end())
        semi = src.find(";", m.end())
        if semi != -1 and (brace == -1 or semi < brace):
            out.append("ST  " + squeeze(src[m.start():semi + 1], 260))
            continue
        if brace == -1:
            continue
        close = balanced(src, brace, "{", "}")
        body = src[brace:close + 1]
        body = re.sub(r"(?m)^\s*///?[^\n]*$", "", body)
        body = re.sub(r"#\[[^\]]*\]", "", body)
        body = re.sub(r"\n\s*\n", "\n", body)
        lines = [ln.strip() for ln in body.splitlines() if ln.strip()]
        out.append("ST  " + squeeze(" ".join(lines), 700))
    for m in re.finditer(r"pub type [A-Za-z0-9_]+[^;]*;", src):
        out.append("TY  " + squeeze(m.group(0), 220))
    return out


def main() -> None:
    crate_dir, out_path = sys.argv[1], sys.argv[2]
    parts = sys.argv[3:]
    base = os.path.join(crate_dir, "src", "Windows", "Win32")
    with open(out_path, "w", encoding="utf-8") as fh:
        for rel in parts:
            path = os.path.join(base, rel, "mod.rs")
            if not os.path.isfile(path):
                fh.write(f"##### !! missing {rel}\n")
                continue
            fh.write(f"\n##### {rel}\n")
            fh.write("\n".join(digest_file(path)) + "\n")
    print(f"{out_path}: {os.path.getsize(out_path)} bytes")


if __name__ == "__main__":
    main()
