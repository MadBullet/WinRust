#!/usr/bin/env python3
"""Extract the exact API surface we depend on into compact text digests.

The agent workspace has no Rust toolchain and no crates.io access, so this
script runs on a CI runner and the produced files are committed back to the
repository.  Everything is best-effort: failures are recorded, never fatal.
"""
from __future__ import annotations

import io
import os
import re
import subprocess
import sys
import tarfile
import traceback
import urllib.request

CRATES = {
    "windows-sys": "0.59.0",
    "eframe": "0.29.1",
    "egui": "0.29.1",
    "egui_extras": "0.29.1",
}

OUT_DIR = os.path.join("docs", "ci")


def log(msg: str) -> None:
    print(msg, flush=True)


def fetch(name: str, version: str) -> str:
    dest = f"/tmp/crates/{name}-{version}"
    if os.path.isdir(dest):
        return dest
    os.makedirs("/tmp/crates", exist_ok=True)
    url = f"https://static.crates.io/crates/{name}/{name}-{version}.crate"
    log(f"fetching {url}")
    data = urllib.request.urlopen(url, timeout=120).read()
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tf:
        tf.extractall("/tmp/crates")
    return dest


def read(path: str) -> str:
    with open(path, encoding="utf-8", errors="replace") as fh:
        return fh.read()


def squeeze(text: str, limit: int = 700) -> str:
    text = re.sub(r"\s+", " ", text).strip()
    return text[:limit]


def extract_signature(src: str, start: int) -> str:
    i = src.find("(", start)
    if i < 0:
        return ""
    depth = 0
    j = i
    while j < len(src):
        c = src[j]
        if c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                break
        j += 1
    end = j + 1
    for term in (";", "{"):
        k = src.find(term, end)
        if k != -1:
            end = min(end + 1, k)
            break
    return squeeze(src[start:end])


def dump_items(src: str, out: list[str], label: str) -> None:
    out.append(f"\n##### {label}")
    for m in re.finditer(r"pub (?:unsafe )?fn [A-Za-z0-9_]+", src):
        sig = extract_signature(src, m.start())
        if sig:
            out.append("FN  " + sig)
    for m in re.finditer(r"pub const [A-Z0-9_]+(?:\s*:\s*[^=;]+)?\s*=", src):
        seg = src[m.start():m.start() + 200]
        out.append("CST " + squeeze(seg, 160))
    for m in re.finditer(r"pub (?:struct|union) [A-Za-z0-9_]+", src):
        name_end = m.end()
        brace = src.find("{", name_end)
        semi = src.find(";", name_end)
        if semi != -1 and (brace == -1 or semi < brace):
            out.append("ST  " + squeeze(src[m.start():semi + 1], 220))
            continue
        depth = 0
        j = brace
        while j < len(src):
            if src[j] == "{":
                depth += 1
            elif src[j] == "}":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        body = src[brace:j + 1]
        body = re.sub(r"(?m)^\s*//[^\n]*$", "", body)
        body = re.sub(r"#\[[^\]]*\]", "", body)
        body = re.sub(r"\n\s*\n", "\n", body)
        lines = [ln.strip() for ln in body.splitlines() if ln.strip()]
        out.append("ST  " + squeeze(" ".join(lines), 900))
    for m in re.finditer(r"pub type [A-Za-z0-9_]+[^;]*;", src):
        out.append("TY  " + squeeze(m.group(0), 200))


def main() -> None:
    os.makedirs(OUT_DIR, exist_ok=True)
    dirs = {}
    for name, ver in CRATES.items():
        try:
            dirs[name] = fetch(name, ver)
        except Exception:
            dirs[name] = ""
            log(f"!! failed to fetch {name}: {traceback.format_exc()}")

    # report the actual layout so paths can be corrected
    listing = []
    for name, d in dirs.items():
        if not d:
            continue
        for root, subdirs, files in os.walk(d):
            depth = root[len(d):].count(os.sep)
            if depth > 2:
                subdirs[:] = []
                continue
            rel = root[len(d):].lstrip(os.sep) or "."
            listing.append(f"{name}: {rel}/ -> dirs={sorted(subdirs)[:8]} files={sorted(files)[:8]}")
    with open("docs/ci/layout.txt", "w", encoding="utf-8") as fh:
        fh.write("\n".join(listing))

    # ---------------- windows-sys ----------------
    out: list[str] = []
    try:
        root = dirs["windows-sys"]
        cargo = read(os.path.join(root, "Cargo.toml"))
        feats = re.findall(r"^(Win32_[A-Za-z0-9_]+)\s*=\s*\[", cargo, re.M)
        out.append("### windows-sys 0.59.0 features of interest")
        groups = [
            "Services", "Registry", "ProcessStatus", "SystemInformation",
            "Threading", "Environment", "Diagnostics", "Storage_FileSystem",
            "Security", "IpHelper", "Globalization", "Graphics_Gdi", "UI_Shell",
            "WindowsAndMessaging", "System_IO", "LibraryLoader", "Com",
            "RemoteDesktop", "Power", "Console", "SystemInformation",
        ]
        for g in groups:
            hits = sorted({f for f in feats if g.lower() in f.lower()})
            out.append(f"{g}: {', '.join(hits)}")
        out.append("")

        want = {
            "System/Services": "services",
            "System/Threading": "threading",
            "System/ProcessStatus": "processstatus",
            "System/SystemInformation": "sysinfo",
            "System/Registry": "registry",
            "System/Diagnostics/Debug": "debug",
            "System/Environment": "environment",
            "UI/Shell": "shell",
            "UI/WindowsAndMessaging": "wam",
            "Graphics/Gdi": "gdi",
            "Storage/FileSystem": "filesystem",
            "NetworkManagement/IpHelper": "iphelper",
            "Security": "security",
            "Security/Authorization": "authorization",
            "Foundation": "foundation",
            "Globalization": "globalization",
            "System/LibraryLoader": "libraryloader",
            "System/RemoteDesktop": "wtssession",
            "System/Power": "power",
            "System/Console": "console",
            "System/Diagnostics/ToolHelp": "toolhelp",
        }
        base = os.path.join(root, "src", "Windows", "Win32")
        for rel, label in want.items():
            path = os.path.join(base, rel + ".rs")
            if not os.path.isfile(path):
                out.append(f"\n##### !! missing {rel}")
                continue
            dump_items(read(path), out, f"Win32::{rel.replace('/', '::')} ({label})")
    except Exception:
        out.append("!! windows-sys dump failed:\n" + traceback.format_exc())
    with open(os.path.join(OUT_DIR, "windows-sys.txt"), "w", encoding="utf-8") as fh:
        fh.write("\n".join(out))
    log(f"windows-sys.txt: {len(out)} items")
    del out

    # ---------------- eframe / egui / egui_extras ----------------
    out = []
    try:
        ef = dirs["eframe"]
        epi = os.path.join(ef, "src", "epi.rs")
        if os.path.isfile(epi):
            src = read(epi)
            dump_items(src, out, "eframe::epi")
        lib = os.path.join(ef, "src", "lib.rs")
        if os.path.isfile(lib):
            src = read(lib)
            m = re.search(r"pub fn run_native[^}]+?\n\}", src, re.S)
            if m:
                out.append("\n##### eframe::run_native\n" + squeeze(m.group(0), 900))
    except Exception:
        out.append("!! eframe dump failed:\n" + traceback.format_exc())

    try:
        eg = dirs["egui"]
        for rel, label in [
            ("src/fonts.rs", "egui::fonts"),
            ("src/data/icon_data.rs", "egui::icon_data"),
            ("src/data/output.rs", "egui::output"),
            ("src/response.rs", "egui::response"),
            ("src/menu.rs", "egui::menu"),
            ("src/style.rs", "egui::style"),
        ]:
            path = os.path.join(eg, rel)
            if not os.path.isfile(path):
                out.append(f"\n##### !! missing {rel}")
                continue
            dump_items(read(path), out, f"{label} ({rel})")

        ctx = os.path.join(eg, "src", "context.rs")
        if os.path.isfile(ctx):
            src = read(ctx)
            out.append("\n##### egui::Context selected methods")
            for name in [
                "set_fonts", "set_visuals", "set_theme", "style_mut", "input",
                "input_mut", "request_repaint", "request_repaint_after",
                "memory_mut", "add_font", "set_pixels_per_point", "run",
                "style", "all_styles_mut", "set_style", "fonts",
            ]:
                for m in re.finditer(r"pub (?:unsafe )?fn " + name + r"\b", src):
                    sig = extract_signature(src, m.start())
                    if sig:
                        out.append("FN  " + sig)
                        break

        vp = os.path.join(eg, "src", "viewport.rs")
        if os.path.isfile(vp):
            src = read(vp)
            out.append("\n##### egui::ViewportBuilder selected methods")
            for name in [
                "with_inner_size", "with_min_inner_size", "with_title",
                "with_icon", "with_resizable", "with_maximized",
                "with_transparent", "with_app_id", "with_decorations",
            ]:
                for m in re.finditer(r"pub fn " + name + r"\b", src):
                    sig = extract_signature(src, m.start())
                    if sig:
                        out.append("FN  " + sig)
                        break

        ins = os.path.join(eg, "src", "input_state.rs")
        if os.path.isfile(ins):
            src = read(ins)
            out.append("\n##### egui::InputState fields")
            m = re.search(r"pub struct InputState \{", src)
            if m:
                depth = 0
                j = m.end() - 1
                while j < len(src):
                    if src[j] == "{":
                        depth += 1
                    elif src[j] == "}":
                        depth -= 1
                        if depth == 0:
                            break
                    j += 1
                body = re.sub(r"(?m)^\s*//[^\n]*$", "", src[m.start():j])
                lines = [ln.strip() for ln in body.splitlines() if ln.strip()]
                out.append(squeeze(" ".join(lines), 3000))

        for rel in ["src/widgets/mod.rs", "src/ui.rs", "src/layout.rs"]:
            path = os.path.join(eg, rel)
            if os.path.isfile(path):
                src = read(path)
                out.append(f"\n##### egui::{rel} (selected)")
                for name in [
                    "label", "button", "small_button", "selectable_label",
                    "text_edit_singleline", "horizontal", "vertical", "add",
                    "add_sized", "separator", "checkbox", "heading", "strong",
                    "monospace", "hyperlink", "group", "collapsing", "columns",
                    "available_width", "allocate_ui_with_layout", "menu_button",
                    "scroll_to_me", "ctx", "painter", "allocate_exact_size",
                    "set_min_width", "scope", "with_layout", "frame",
                ]:
                    for m in re.finditer(r"pub fn " + name + r"\b", src):
                        sig = extract_signature(src, m.start())
                        if sig:
                            out.append("FN  " + sig)
                            break
    except Exception:
        out.append("!! egui dump failed:\n" + traceback.format_exc())

    try:
        ex = dirs["egui_extras"]
        table = os.path.join(ex, "src", "table.rs")
        if os.path.isfile(table):
            src = read(table)
            dump_items(src, out, "egui_extras::table")
    except Exception:
        out.append("!! egui_extras dump failed:\n" + traceback.format_exc())

    with open(os.path.join(OUT_DIR, "egui.txt"), "w", encoding="utf-8") as fh:
        fh.write("\n".join(out))
    log(f"egui.txt: {len(out)} items")

    for name in ("windows-sys.txt", "egui.txt"):
        p = os.path.join(OUT_DIR, name)
        log(f"{name}: {os.path.getsize(p)} bytes")


if __name__ == "__main__":
    try:
        main()
    except Exception:
        traceback.print_exc()
        os.makedirs(OUT_DIR, exist_ok=True)
        with open(os.path.join(OUT_DIR, "errors.txt"), "a", encoding="utf-8") as fh:
            fh.write(traceback.format_exc())
        sys.exit(0)
