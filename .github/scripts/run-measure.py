#!/usr/bin/env python3
"""Measure peak RAM and startup of the release binaries. Never skips.

--kind gui   runs target/release/openpdf-gtk --measure N (under xvfb-run on
             Linux when there is no display) and parses its
             startup_ms / peak_rss_bytes (or peak_rss_mb) lines.
--kind core  runs target/release/openpdf info corpus/two_page.pdf and measures
             wall-clock startup and peak RSS of the child process itself
             (wait4 on Linux/macOS, PeakWorkingSetSize on Windows).

Exits non-zero if the binary is missing, exits non-zero, reports no metrics,
or exceeds OPENPDF_MEASURE_RAM_MIB (default 250) or OPENPDF_MEASURE_STARTUP_MS
(default 300 core, 1500 gui). OPENPDF_MEASURE_BIN overrides the binary path.
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

DEFAULT_STARTUP_MS = {"core": 300, "gui": 1500}
ROOT = Path(os.environ.get("GITHUB_WORKSPACE", Path.cwd()))
RELEASE = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / "release"
EXE = ".exe" if os.name == "nt" else ""
BINARIES = {"gui": f"openpdf-gtk{EXE}", "core": f"openpdf{EXE}"}


def fail(msg: str) -> int:
    print(f"measure: FAIL: {msg}", file=sys.stderr)
    return 1


def find_binary(kind: str) -> Path:
    extra = os.environ.get("OPENPDF_MEASURE_BIN")
    return Path(extra) if extra else RELEASE / BINARIES[kind]


def parse_metrics(text: str) -> tuple[int | None, int | None]:
    rss = startup = None
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


def run_gui(binary: Path, frames: str) -> tuple[int, str]:
    cmd = [str(binary), "--measure", frames]
    env = os.environ.copy()
    if sys.platform.startswith("linux"):
        env.setdefault("GDK_BACKEND", "x11")
        env.setdefault("GSK_RENDERER", "cairo")
        if not env.get("DISPLAY") and not env.get("WAYLAND_DISPLAY"):
            xvfb = shutil.which("xvfb-run")
            if not xvfb:
                raise FileNotFoundError("xvfb-run not found and no display")
            cmd = [xvfb, "-a", "--server-args=-screen 0 1280x800x24"] + cmd
    proc = subprocess.run(cmd, cwd=ROOT, env=env, text=True,
                          capture_output=True, check=False)
    return proc.returncode, (proc.stdout or "") + (proc.stderr or "")


def _peak_rss_windows(handle: int) -> int | None:
    import ctypes
    from ctypes import wintypes

    class PMC(ctypes.Structure):
        _fields_ = [("cb", wintypes.DWORD), ("PageFaultCount", wintypes.DWORD),
                    ("PeakWorkingSetSize", ctypes.c_size_t),
                    ("WorkingSetSize", ctypes.c_size_t),
                    ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
                    ("QuotaPagedPoolUsage", ctypes.c_size_t),
                    ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
                    ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
                    ("PagefileUsage", ctypes.c_size_t),
                    ("PeakPagefileUsage", ctypes.c_size_t)]
    pmc = PMC()
    pmc.cb = ctypes.sizeof(PMC)
    psapi = ctypes.WinDLL("psapi")
    if psapi.GetProcessMemoryInfo(wintypes.HANDLE(handle), ctypes.byref(pmc), pmc.cb):
        return int(pmc.PeakWorkingSetSize)
    return None


def run_core(binary: Path) -> tuple[int, str]:
    sample = ROOT / "corpus" / "two_page.pdf"
    cmd = [str(binary), "info", str(sample)]
    start = time.perf_counter()
    proc = subprocess.Popen(cmd, cwd=ROOT, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if os.name == "nt":
        out, _ = proc.communicate()
        ms = int((time.perf_counter() - start) * 1000)
        rss = _peak_rss_windows(int(proc._handle))  # handle stays open until Popen is collected
        code = proc.returncode
    else:
        out = proc.stdout.read() if proc.stdout else ""
        _, status, ru = os.wait4(proc.pid, 0)
        ms = int((time.perf_counter() - start) * 1000)
        proc.returncode = os.waitstatus_to_exitcode(status)
        code = proc.returncode
        # ru_maxrss is KiB on Linux, bytes on macOS.
        rss = ru.ru_maxrss if sys.platform == "darwin" else ru.ru_maxrss * 1024
    out = out or ""
    out += f"\nstartup_ms={ms}\n"
    if rss is not None:
        out += f"peak_rss_bytes={rss}\npeak_rss_mb={rss / 1048576:.1f}\n"
    return code, out


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kind", choices=sorted(DEFAULT_STARTUP_MS),
                        default=os.environ.get("OPENPDF_MEASURE_KIND", "core"))
    parser.add_argument("--frames", default=os.environ.get("OPENPDF_MEASURE_FRAMES", "32"))
    args = parser.parse_args(argv)
    kind = args.kind
    ram_mib = int(os.environ.get("OPENPDF_MEASURE_RAM_MIB", "250"))
    startup_limit = int(os.environ.get("OPENPDF_MEASURE_STARTUP_MS") or DEFAULT_STARTUP_MS[kind])

    binary = find_binary(kind)
    if not binary.is_file() or not os.access(binary, os.X_OK):
        return fail(f"{kind} binary not found at {binary}; build it first "
                    f"(cargo build --release -p {'openpdf-gtk' if kind == 'gui' else 'openpdf-cli'})")

    print(f"measure: kind={kind} running {binary}")
    print(f"measure: limits ram={ram_mib} MiB startup={startup_limit} ms")
    try:
        code, output = run_gui(binary, args.frames) if kind == "gui" else run_core(binary)
    except FileNotFoundError as e:
        return fail(str(e))
    print(output)
    if code != 0:
        return fail(f"binary exited {code}")

    rss, startup = parse_metrics(output)
    if rss is None or startup is None:
        return fail("binary printed no startup_ms / peak_rss metrics")
    if rss > ram_mib * 1048576:
        return fail(f"peak RSS {rss / 1048576:.1f} MiB exceeds {ram_mib} MiB")
    if startup > startup_limit:
        return fail(f"startup {startup} ms exceeds {startup_limit} ms")
    print(f"measure: OK kind={kind} startup_ms={startup} peak_rss_mb={rss / 1048576:.1f}")
    return 0


if __name__ == "__main__":
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(errors="replace")
    raise SystemExit(main())
