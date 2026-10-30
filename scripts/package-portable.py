#!/usr/bin/env python3
"""Build and package a host release for Linux, macOS or Windows."""

from __future__ import annotations

import hashlib
import os
import re
import shutil
import subprocess
import sys
import tarfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DIST = ROOT / "dist"


def run(command: list[str]) -> str:
    result = subprocess.run(
        command,
        cwd=ROOT,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    return result.stdout


def version() -> str:
    text = (ROOT / "Cargo.toml").read_text()
    match = re.search(r"^\[workspace\.package\][\s\S]*?^version\s*=\s*\"([^\"]+)\"", text, re.M)
    if not match:
        raise SystemExit("could not determine workspace version")
    return match.group(1)


def host_target() -> str:
    text = run(["rustc", "-vV"])
    match = re.search(r"^host:\s*(\S+)$", text, re.M)
    if not match:
        raise SystemExit("could not determine host target")
    return match.group(1)


def main() -> None:
    ver = version()
    target = host_target()
    windows = os.name == "nt" or "windows" in target
    bundle = f"orleans-{ver}-{target}"
    bundle_dir = DIST / bundle
    archive = DIST / (bundle + (".zip" if windows else ".tar.gz"))

    run(["cargo", "build", "--release", "--locked"])

    shutil.rmtree(bundle_dir, ignore_errors=True)
    (bundle_dir / "docs").mkdir(parents=True)
    (bundle_dir / "examples").mkdir(parents=True)

    binary = ROOT / "target" / "release" / ("orleans.exe" if windows else "orleans")
    if not binary.is_file():
        raise SystemExit(f"missing built binary: {binary}")
    shutil.copy2(binary, bundle_dir / binary.name)

    for name in ("README.md", "CHANGELOG.md", "LICENSE-MIT", "LICENSE-APACHE"):
        shutil.copy2(ROOT / name, bundle_dir / name)
    for path in sorted((ROOT / "docs").glob("*.md")):
        shutil.copy2(path, bundle_dir / "docs" / path.name)
    shutil.copy2(ROOT / "examples" / "coaster.svg", bundle_dir / "examples" / "coaster.svg")

    archive.unlink(missing_ok=True)
    if windows:
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as zf:
            for path in sorted(bundle_dir.rglob("*")):
                if path.is_file():
                    zf.write(path, path.relative_to(DIST).as_posix())
    else:
        with tarfile.open(archive, "w:gz") as tf:
            tf.add(bundle_dir, arcname=bundle)

    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    checksum = archive.with_name(archive.name + ".sha256")
    checksum.write_text(f"{digest}  {archive.name}\n")

    if sys.stdout.isatty():
        print(f"created dist/{archive.name}")
        print(f"created dist/{checksum.name}")


if __name__ == "__main__":
    main()
