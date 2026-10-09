#!/usr/bin/env python3
"""Run the perf/measure binary if present. Skip if it is not built yet.

Fails when parsed peak RSS exceeds OPENPDF_MEASURE_RAM_MIB (default 250)
or startup_ms exceeds OPENPDF_MEASURE_STARTUP_MS (default 8000).
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
from pathlib import Path


RAM_MIB = int(os.environ.get("OPENPDF_MEASURE_RAM_MIB", "250"))
STARTUP_MS = int(os.environ.get("OPENPDF_MEASURE_STARTUP_MS", "8000"))
RAM_BYTES = RAM_MIB * 1024 * 1024

ROOT = Path(os.environ.get("GITHUB_WORKSPACE", Path.cwd()))
RELEASE = ROOT / "target" / "release"
EXE_SUFFIX = ".exe" if os.name == "nt" else ""

CANDIDATES = [
    RELEASE / f"measure{EXE_SUFFIX}",
    RELEASE / f"perf{EXE_SUFFIX}",
    RELEASE / f"openpdf-measure{EXE_SUFFIX}",
]


def find_binary() -> Path | None:
    extra = os.environ.get("OPENPDF_MEASURE_BIN")
    if extra:
        p = Path(extra)
        if p.is_file():
            return p
    for p in CANDIDATES:
        if p.is_file() and os.access(p, os.X_OK):
            return p
    return None


def parse_metrics(text: str) -> tuple[int | None, int | None]:
    rss = None
    startup = None
    m = re.search(r"peak_rss_bytes\s*=\s*(\d+)", text)
    if m:
        rss = int(m.group(1))
    else:
        m = re.search(r"peak_rss_mb\s*=\s*([0-9.]+)", text)
        if m:
            rss = int(float(m.group(1)) * 1024 * 1024)
    m = re.search(r"startup_ms\s*=\s*(\d+)", text)
    if m:
        startup = int(m.group(1))
    return rss, startup


def main() -> int:
    binary = find_binary()
    if binary is None:
        print("measure: no perf/measure binary yet; skipping")
        return 0

    print(f"measure: running {binary}")
    print(f"measure: limits ram={RAM_MIB} MiB startup={STARTUP_MS} ms")
    proc = subprocess.run(
        [str(binary)],
        cwd=ROOT,
        text=True,
        capture_output=True,
        env=os.environ.copy(),
        check=False,
    )
    output = (proc.stdout or "") + (proc.stderr or "")
    if output:
        print(output)
    if proc.returncode != 0:
        print(f"measure: binary exited {proc.returncode}", file=sys.stderr)
        return proc.returncode

    rss, startup = parse_metrics(output)
    if rss is not None and rss > RAM_BYTES:
        print(f"measure: peak RSS {rss} bytes exceeds {RAM_BYTES} ({RAM_MIB} MiB)", file=sys.stderr)
        return 1
    if startup is not None and startup > STARTUP_MS:
        print(
            f"measure: startup {startup} ms exceeds {STARTUP_MS} ms (OPENPDF_MEASURE_STARTUP_MS)",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(errors="replace")
    raise SystemExit(main())
