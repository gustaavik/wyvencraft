#!/usr/bin/env python3
"""Collect the license texts of every compiled dependency into one notices file.

MIT and BSD require their copyright notice and license text to accompany a
binary distribution; Apache-2.0 section 4 requires the NOTICE file. The image
ships hundreds of such dependencies, so this generates the file that satisfies
them. Deliberately dependency-free: it reads `cargo metadata` and the license
files cargo has already vendored into the registry, so it runs anywhere the
build runs, including inside the Docker builder after `cargo fetch`.

Usage: scripts/third-party-notices.py [--check] > THIRD-PARTY.txt
"""
import json, subprocess, sys, os, glob

LICENSE_GLOBS = ("LICENSE*", "LICENCE*", "COPYING*", "NOTICE*", "UNLICENSE*")
# Crates in this workspace: our own code, covered by COPYRIGHT / the ticket crate.
LOCAL = {"wyvencraft", "wyven-core", "wyven-assets", "wyven-render", "wyven-model",
         "wyven-voxel", "wyven-net", "wyven-input", "wyven-auth", "wyven-app"}

def collect(features):
    # No --filter-platform: releases are built for macOS and Linux from one
    # lockfile, so the notices must cover the union rather than one host's slice.
    cmd = ["cargo", "metadata", "--format-version", "1", "--locked"]
    if features:
        cmd += ["--features", features]
    meta = json.loads(subprocess.run(cmd, capture_output=True, text=True, check=True).stdout)
    out = []
    for pkg in sorted(meta["packages"], key=lambda p: (p["name"].lower(), p["version"])):
        if pkg["name"] in LOCAL:
            continue
        root = os.path.dirname(pkg["manifest_path"])
        texts = []
        for pattern in LICENSE_GLOBS:
            for path in sorted(glob.glob(os.path.join(root, pattern))):
                if os.path.isfile(path) and os.path.getsize(path) < 200_000:
                    try:
                        texts.append((os.path.basename(path),
                                      open(path, encoding="utf-8", errors="replace").read().strip()))
                    except OSError:
                        pass
        out.append((pkg["name"], pkg["version"], pkg.get("license"),
                    pkg.get("repository"), texts))
    return out

def render(pkgs):
    w = []
    w.append("THIRD-PARTY SOFTWARE NOTICES")
    w.append("=" * 78)
    w.append("")
    w.append("Wyvencraft incorporates the open-source components listed below. Each remains")
    w.append("under its own license, reproduced here in full where the project ships a")
    w.append("license file. These are dependencies of Wyvencraft, not part of it:")
    w.append("Wyvencraft's own code is MIT OR Apache-2.0 (LICENSE-MIT, LICENSE-APACHE) and")
    w.append("its assets are proprietary (assets/LICENSE). Where a component's license")
    w.append("conflicts with those, the component's license governs that component.")
    w.append("")
    w.append("Listed for every platform this is built for, so one file covers every release")
    w.append("artifact. A crate with no text below vendors no license file of its own; its")
    w.append("declared license and source are named instead.")
    w.append("")
    w.append(f"{len(pkgs)} components.")
    w.append("")
    w.append("SUMMARY")
    w.append("-" * 78)
    for name, version, lic, _repo, _texts in pkgs:
        w.append(f"  {name} {version} — {lic or 'see text below'}")
    w.append("")
    w.append("FULL TEXTS")
    w.append("=" * 78)
    for name, version, lic, repo, texts in pkgs:
        w.append("")
        w.append("-" * 78)
        w.append(f"{name} {version}")
        if lic:  w.append(f"License: {lic}")
        if repo: w.append(f"Source:  {repo}")
        w.append("-" * 78)
        if texts:
            for fname, body in texts:
                w.append("")
                w.append(f"--- {fname} ---")
                w.append(body)
        else:
            w.append("")
            w.append(f"No license file is vendored with this crate. Its declared license is")
            w.append(f"{lic or 'unstated'}; see the source repository above for the full text.")
        w.append("")
    return "\n".join(w) + "\n"

if __name__ == "__main__":
    args = [a for a in sys.argv[1:]]
    check = "--check" in args
    if check: args.remove("--check")
    features = args[0] if args else ""
    text = render(collect(features))
    if check:
        target = "THIRD-PARTY.txt"
        current = open(target, encoding="utf-8").read() if os.path.exists(target) else ""
        if current != text:
            print(f"{target} is out of date; run `make licenses`", file=sys.stderr)
            sys.exit(1)
        print(f"{target} is current")
    else:
        sys.stdout.write(text)
