#!/usr/bin/env python3
"""Create a probe crate that pulls the WinRust dependency set and vendor it."""
import subprocess, sys, pathlib

root = pathlib.Path("/tmp/probe")
subprocess.run(["cargo", "new", "--quiet", "--bin", str(root)], check=True)
(root / "Cargo.toml").write_text("""[package]
name = "probe"
version = "0.0.0"
edition = "2021"

[dependencies]
windows-sys = { version = "0.59", features = ["Win32_Foundation"] }
eframe = { version = "0.29", default-features = false, features = ["glow", "default_fonts"] }
egui = "0.29"
egui_extras = "0.29"
""")
p = subprocess.run(["cargo", "fetch", "--manifest-path", str(root / "Cargo.toml")], text=True)
print("fetch rc:", p.returncode)
p = subprocess.run(
    ["cargo", "vendor", str(root / "vendor"), "--manifest-path", str(root / "Cargo.toml")],
    text=True, capture_output=True)
print("vendor rc:", p.returncode)
print(p.stdout[-3000:], p.stderr[-6000:])
print("VERSIONS")
for d in sorted((root / "vendor").iterdir()):
    print(" ", d.name)
