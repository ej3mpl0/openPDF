#!/usr/bin/env python3
"""Run the perf/measure binary if present. Skip if it is not built yet.

Fails when parsed peak RSS exceeds OPENPDF_MEASURE_RAM_MIB (default 250)
or startup_ms exceeds OPENPDF_MEASURE_STARTUP_MS.

Startup defaults are 300 ms for --kind core (core/CLI) and 1500 ms for
--kind gui. The matrix can override OPENPDF_MEASURE_STARTUP_MS per OS.
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from pathlib import Path


DEFAULT_STARTUP_MS = {
    "core": 300,
    "gui": 1500,
}


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--kind",
        choices=sorted(DEFAULT_STARTUP_MS),
        default=os.environ.get("OPENPDF_MEASURE_KIND", "core"),
        help="core/CLI (300 ms) or GUI (1500 ms) startup budget",
    )
    return parser.parse_args(argv)


ROOT = Path(os.environ.get("GITHUB_WORKSPACE", Path.cwd()))
RELEASE = ROOT / "target" / "release"
EXE_SUFFIX = ".exe" if os.name == "nt" else ""

CORE_CANDIDATES = [
    RELEASE / f"measure{EXE_SUFFIX}",
    RELEASE / f"perf{EXE_SUFFIX}",
    RELEASE / f"openpdf-measure{EXE_SUFFIX}",
]
GUI_CANDIDATES = [
    RELEASE / f"measure-gui{EXE_SUFFIX}",
    RELEASE / f"openpdf-gtk-measure{EXE_SUFFIX}",
    RELEASE / f"openpdf-measure-gui{EXE_SUFFIX}",
]


def candidates_for(kind: str) -> list[Path]:
    return GUI_CANDIDATES if kind == "gui" else CORE_CANDIDATES


def find_binary(kind: str) -> Path | None:
    extra = os.environ.get("OPENPDF_MEASURE_BIN")
    if extra:
        p = Path(extra)
        if p.is_file():
            return p
    for p in candidates_for(kind):
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
    args = parse_args()
    kind = args.kind
    ram_mib = int(os.environ.get("OPENPDF_MEASURE_RAM_MIB", "250"))
    startup_ms = int(
        os.environ.get("OPENPDF_MEASURE_STARTUP_MS", str(DEFAULT_STARTUP_MS[kind]))
    )
    ram_bytes = ram_mib * 1024 * 1024

    binary = find_binary(kind)
    if binary is None:
        print(f"measure: no {kind} perf/measure binary yet; skipping")
        return 0

    print(f"measure: kind={kind} running {binary}")
    print(f"measure: limits ram={ram_mib} MiB startup={startup_ms} ms")
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
    if rss is not None and rss > ram_bytes:
        print(f"measure: peak RSS {rss} bytes exceeds {ram_bytes} ({ram_mib} MiB)", file=sys.stderr)
        return 1
    if startup is not None and startup > startup_ms:
        print(
            f"measure: startup {startup} ms exceeds {startup_ms} ms (OPENPDF_MEASURE_STARTUP_MS)",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(errors="replace")
    raise SystemExit(main())
