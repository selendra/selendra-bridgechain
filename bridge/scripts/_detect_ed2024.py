#!/usr/bin/env python3
# List crates in Cargo.lock whose extracted registry manifest declares
# edition = "2024" (which the Solana 1.18 platform-tools cargo can't parse).
import os, re, glob, sys

lock = "Cargo.lock"
reg = glob.glob(os.path.expanduser("~/.cargo/registry/src/*/"))
name = ver = None
found = []
for line in open(lock):
    line = line.strip()
    if line.startswith("name = "):
        name = line.split('"')[1]
    elif line.startswith("version = ") and '"' in line:
        ver = line.split('"')[1]
        for r in reg:
            m = os.path.join(r, f"{name}-{ver}", "Cargo.toml")
            if os.path.isfile(m):
                txt = open(m, encoding="utf-8", errors="ignore").read()
                if re.search(r'edition\s*=\s*"2024"', txt):
                    found.append(f"{name}@{ver}")
                break
for f in sorted(set(found)):
    print(f)
