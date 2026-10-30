#!/usr/bin/env python3
"""Smoke-test the packaged host binary."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DIST = ROOT / "dist"


def main() -> None:
    bundle = max(
        (
            path
            for path in DIST.iterdir()
            if path.is_dir()
            and path.name.startswith("orleans-")
            and ((path / "orleans").is_file() or (path / "orleans.exe").is_file())
        ),
        key=lambda path: path.stat().st_mtime,
    )
    binary = bundle / ("orleans.exe" if os.name == "nt" else "orleans")
    subprocess.run([str(binary), "--version"], cwd=ROOT, check=True)
    subprocess.run(
        [
            str(binary),
            "run",
            "examples/coaster.svg",
            "--mock",
            "--driver",
            "marlin",
        ],
        cwd=ROOT,
        check=True,
    )


if __name__ == "__main__":
    main()
