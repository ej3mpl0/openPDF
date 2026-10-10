#!/usr/bin/env python3
"""Measure peak RAM and startup of the release binaries. Never skips.

--kind gui   runs target/release/openpdf-gtk --measure N (under xvfb-run on
             Linux when there is no display) and parses its
             startup_ms / peak_rss_bytes (or peak_rss_mb) lines.
--kind core  runs target/release/openpdf info corpus/two_page.pdf and measures
             wall-clock startup and peak RSS of the child process itself
             (wait4 on Linux/macOS, PeakWorkingSetSize on Windows).
             If target/release/openpdf-measure also exists (openpdf-core, #10),
             it is run as well, as a separate core measurement: it prints one
             `openpdf_measure k=v ...` line with open-relative durations
             (open_ms, pdfium_load_ms, doc_open_ms, first_tile_ms, ...) and no
             startup_ms. Its required keys are CORE_MEASURE_KEYS; medians are
             reported under core_measure.*, never mixed with CLI startup_ms or
             GTK ui_* keys. Its gate is the median of core_ready_ms =
             open_ms + pdfium_load_ms + doc_open_ms + first_tile_ms against
             OPENPDF_MEASURE_CORE_READY_MS (default 1000).

Exits non-zero if the binary is missing, exits non-zero, reports no metrics,
or exceeds OPENPDF_MEASURE_RAM_MIB (default 250) or OPENPDF_MEASURE_STARTUP_MS
(default 300 core, 1500 gui). OPENPDF_MEASURE_BIN overrides the binary path.
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import statistics
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
    if extra:
        return Path(extra)
    return RELEASE / BINARIES[kind]


def is_core_measure(binary: Path) -> bool:
    return binary.stem == "openpdf-measure"


CORE_MEASURE_KEYS = ("open_ms", "pdfium_load_ms", "doc_open_ms", "first_tile_ms",
                     "peak_rss_bytes")
KV = re.compile(r"([a-z][a-z0-9_]*)=(\S+)")


def run_core_measure(binary: Path) -> tuple[int, str]:
    proc = subprocess.run(PIN + [str(binary)], cwd=ROOT, text=True,
                          capture_output=True, check=False)
    return proc.returncode, (proc.stdout or "") + (proc.stderr or "")


def parse_core_measure(text: str) -> dict[str, float] | str:
    """Numeric fields of the `openpdf_measure ...` line, or an error string."""
    line = next((l for l in text.splitlines() if l.startswith("openpdf_measure ")), None)
    if line is None:
        return "no `openpdf_measure` line"
    out: dict[str, float] = {}
    for k, v in KV.findall(line):
        try:
            out[k] = float(v)
        except ValueError:
            pass
    missing = [k for k in CORE_MEASURE_KEYS if k not in out]
    if missing:
        return f"missing/non-numeric keys {missing}"
    negative = [k for k in CORE_MEASURE_KEYS if out[k] < 0]
    if negative:
        return f"keys not measured (negative, PDFium missing?) {negative}"
    if any(k.startswith("ui_") for k in out):
        return "core measure emitted ui_* keys"
    out["core_ready_ms"] = (out["open_ms"] + out["pdfium_load_ms"]
                            + out["doc_open_ms"] + out["first_tile_ms"])
    return out


def fmt(key: str, v: float) -> str:
    return f"{v:.0f}" if key == "peak_rss_bytes" else f"{v:g}"


def measure_core_bin(binary: Path, runs: int, warmup: int) -> int:
    ready_limit = float(os.environ.get("OPENPDF_MEASURE_CORE_READY_MS") or 1000)
    ram_mib = int(os.environ.get("OPENPDF_MEASURE_RAM_MIB", "250"))
    print(f"measure: core_measure running {binary} runs={runs} warmup={warmup}")
    print(f"measure: core_measure limits median core_ready_ms<={ready_limit:g}, "
          f"max peak RSS<={ram_mib} MiB")
    samples: dict[str, list[float]] = {}
    for n in range(-warmup + 1, runs + 1):
        code, output = run_core_measure(binary)
        if code != 0:
            print(output)
            return fail(f"core_measure run {n}: binary exited {code}")
        got = parse_core_measure(output)
        if isinstance(got, str):
            print(output)
            return fail(f"core_measure run {n}: {got}")
        if n <= 0:
            print("measure: core_measure warm-up discarded")
            continue
        keys = [k for k in got if k.endswith("_ms") or k == "peak_rss_bytes"]
        for k in keys:
            samples.setdefault(k, []).append(got[k])
        print(f"measure: core_measure run {n}/{runs} "
              + " ".join(f"{k}={fmt(k, got[k])}" for k in keys))
    meds = {k: statistics.median(v) for k, v in samples.items()}
    peak = max(samples["peak_rss_bytes"])
    print("measure: core_measure medians "
          + " ".join(f"core_measure.{k}={fmt(k, v)}" for k, v in meds.items()))
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as f:
            f.write("### [core] openpdf-measure (open-relative, not process startup)\n\n"
                    "| metric | median |\n| --- | ---: |\n")
            for k, v in meds.items():
                f.write(f"| core_measure.{k} | {fmt(k, v)} |\n")
            f.write(f"| max peak_rss_bytes | {peak:.0f} ({peak / 1048576:.1f} MiB) |\n")
            f.write(f"| runs / warm-up | {runs} / {warmup} |\n\n")
    bad = False
    if meds["core_ready_ms"] > ready_limit:
        fail(f"core_measure median core_ready_ms {meds['core_ready_ms']:g} exceeds {ready_limit:g}")
        bad = True
    if peak > ram_mib * 1048576:
        fail(f"core_measure max peak RSS {peak / 1048576:.1f} MiB exceeds {ram_mib} MiB")
        bad = True
    if bad:
        return 1
    print("measure: OK core_measure")
    return 0


EXTRA_MS = re.compile(r"^([a-z][a-z0-9_]*_ms)\s*=\s*([0-9.]+)\s*$", re.M)


def parse_marks(text: str) -> dict[str, float]:
    """Every `<name>_ms=<n>` line except startup_ms and limit_* (e.g.
    activate_ms, window_present_ms, first_frame_ms, first_tile_ms)."""
    return {k: float(v) for k, v in EXTRA_MS.findall(text)
            if k != "startup_ms" and not k.startswith("limit_")}


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


PIN: list[str] = []


def pin_prefix(cpus: str) -> list[str]:
    """Opt-in `taskset -c CPUS` on Linux. Off by default: openpdf-gtk is
    multi-threaded and pinning to one core adds ~100 ms of startup."""
    if cpus and sys.platform.startswith("linux"):
        ts = shutil.which("taskset")
        if not ts:
            raise FileNotFoundError("--pin requested but taskset not found")
        return [ts, "-c", cpus]
    return []


def pixbuf_loaders_cache() -> Path | None:
    """The gdk-pixbuf loaders.cache GTK will use (GDK_PIXBUF_MODULE_FILE, else
    pkg-config's gdk_pixbuf_cache_file, else the first system one)."""
    env = os.environ.get("GDK_PIXBUF_MODULE_FILE")
    if env:
        return Path(env)
    pc = shutil.which("pkg-config")
    if pc:
        out = subprocess.run([pc, "--variable=gdk_pixbuf_cache_file", "gdk-pixbuf-2.0"],
                             text=True, capture_output=True, check=False).stdout.strip()
        if out:
            return Path(out)
    for c in sorted(Path("/usr/lib").glob("*/gdk-pixbuf-2.0/*/loaders.cache")):
        return c
    return None


def check_svg_loader() -> str | None:
    """GUI numbers are gated with the SVG loader ON (as shipped). Error or None."""
    cache = pixbuf_loaders_cache()
    if cache is None or not cache.is_file():
        return f"gdk-pixbuf loaders.cache not found ({cache}); install librsvg2-common"
    text = cache.read_text(encoding="utf-8", errors="replace")
    if not re.search(r'^"[^"]*pixbufloader[_-]?svg[^"]*\.so"', text, re.M):
        return f"gdk-pixbuf loaders.cache {cache} has no svg loader; install librsvg2-common"
    print(f"measure: svg pixbuf loader present in {cache}")
    return None


def run_gui(binary: Path, frames: str) -> tuple[int, str]:
    cmd = PIN + [str(binary), "--measure", frames]
    env = os.environ.copy()
    if sys.platform.startswith("linux"):
        env.setdefault("GDK_BACKEND", "x11")
        if not env.get("DISPLAY") and not env.get("WAYLAND_DISPLAY"):
            xvfb = shutil.which("xvfb-run")
            if not xvfb:
                raise FileNotFoundError("xvfb-run not found and no display")
            cmd = [xvfb, "-a", "--server-args=-screen 0 1280x800x24"] + cmd  # taskset pins the app; xvfb-run execs it
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
    cmd = PIN + [str(binary), "info", str(sample)]
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
    out = re.sub(r"^.*_ms\s*=.*$", "", out, flags=re.M)  # CLI output is not marks
    out += f"\nstartup_ms={ms}\n"
    if rss is not None:
        out += f"peak_rss_bytes={rss}\npeak_rss_mb={rss / 1048576:.1f}\n"
    return code, out


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kind", choices=sorted(DEFAULT_STARTUP_MS),
                        default=os.environ.get("OPENPDF_MEASURE_KIND", "core"))
    parser.add_argument("--frames", default=os.environ.get("OPENPDF_MEASURE_FRAMES", "32"))
    parser.add_argument("--runs", type=int, default=int(os.environ.get("OPENPDF_MEASURE_RUNS", "5")),
                        help="repeat N times; enforce median startup and max peak RSS")
    parser.add_argument("--warmup", type=int, default=int(os.environ.get("OPENPDF_MEASURE_WARMUP", "1")),
                        help="discarded warm-up runs before measuring")
    parser.add_argument("--pin", metavar="CPUS", default=os.environ.get("OPENPDF_MEASURE_PIN", ""),
                        help="opt-in: wrap runs in `taskset -c CPUS` on Linux (e.g. 0-1); default off")
    args = parser.parse_args(argv)
    try:
        PIN[:] = pin_prefix(args.pin)
    except FileNotFoundError as e:
        return fail(str(e))
    if args.runs < 1 or args.warmup < 0:
        return fail("--runs must be >= 1")
    kind = args.kind
    ram_mib = int(os.environ.get("OPENPDF_MEASURE_RAM_MIB", "250"))
    startup_limit = int(os.environ.get("OPENPDF_MEASURE_STARTUP_MS") or DEFAULT_STARTUP_MS[kind])

    binary = find_binary(kind)
    if kind == "core" and is_core_measure(binary):
        # Explicit OPENPDF_MEASURE_BIN=.../openpdf-measure: only that measurement.
        if not binary.is_file():
            return fail(f"core measure binary not found at {binary}")
        return measure_core_bin(binary, args.runs, args.warmup)
    if not binary.is_file() or not os.access(binary, os.X_OK):
        return fail(f"{kind} binary not found at {binary}; build it first "
                    f"(cargo build --release {'--workspace --locked' if kind == 'gui' else '-p openpdf-cli'})")

    if kind == "gui" and sys.platform.startswith("linux"):
        err = check_svg_loader()
        if err:
            return fail(err)
    print(f"measure: kind={kind} running {binary} runs={args.runs}")
    print(f"measure: limits median startup<={startup_limit} ms, max peak RSS<={ram_mib} MiB")
    print(f"measure: warmup={args.warmup} pin={' '.join(PIN) or 'off'}")
    for n in range(1, args.warmup + 1):
        try:
            code, output = run_gui(binary, args.frames) if kind == "gui" else run_core(binary)
        except FileNotFoundError as e:
            return fail(str(e))
        if code != 0:
            print(output)
            return fail(f"warm-up {n}: binary exited {code}")
        print(f"measure: warm-up {n}/{args.warmup} discarded")
    marks: dict[str, list[float]] = {}
    startups: list[int] = []
    rsses: list[int] = []
    for n in range(1, args.runs + 1):
        try:
            code, output = run_gui(binary, args.frames) if kind == "gui" else run_core(binary)
        except FileNotFoundError as e:
            return fail(str(e))
        if code != 0:
            print(output)
            return fail(f"run {n}: binary exited {code}")
        rss, startup = parse_metrics(output)
        if rss is None or startup is None:
            print(output)
            return fail(f"run {n}: binary printed no startup_ms / peak_rss metrics")
        startups.append(startup)
        rsses.append(rss)
        extra = parse_marks(output)
        for k, v in extra.items():
            marks.setdefault(k, []).append(v)
        extra_s = "".join(f" {k}={v:g}" for k, v in extra.items())
        print(f"measure: run {n}/{args.runs} startup_ms={startup} peak_rss_bytes={rss} "
              f"peak_rss_mb={rss / 1048576:.1f}{extra_s}")

    med = statistics.median(startups)
    peak = max(rsses)
    print(f"measure: median startup_ms={med:g} (min {min(startups)}, max {max(startups)}); "
          f"max peak_rss_bytes={peak} peak_rss_mb={peak / 1048576:.1f} "
          f"(median {statistics.median(rsses) / 1048576:.1f})")
    mark_meds = {k: statistics.median(v) for k, v in marks.items()}
    if mark_meds:
        print("measure: median marks " + " ".join(f"{k}={v:g}" for k, v in mark_meds.items()))
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        # Always tagged with --kind; each invocation measures one kind, so
        # core and gui keys (e.g. first_tile_ms vs ui_first_tile_ms) never mix.
        label = f"[{kind}] " + os.environ.get("OPENPDF_MEASURE_LABEL", "measure")
        with open(summary, "a", encoding="utf-8") as f:
            f.write(f"### {label}\n\n| metric | median |\n| --- | ---: |\n")
            f.write(f"| startup_ms | {med:g} |\n")
            for k, v in mark_meds.items():
                f.write(f"| {k} | {v:g} |\n")
            f.write(f"| max peak_rss_bytes | {peak} ({peak / 1048576:.1f} MiB) |\n")
            f.write(f"| runs / warm-up | {args.runs} / {args.warmup} |\n\n")
    bad = False
    if med > startup_limit:
        fail(f"median startup {med:g} ms exceeds {startup_limit} ms")
        bad = True
    if peak > ram_mib * 1048576:
        fail(f"max peak RSS {peak / 1048576:.1f} MiB exceeds {ram_mib} MiB")
        bad = True
    if bad:
        return 1
    print(f"measure: OK kind={kind}")
    measure_bin = RELEASE / f"openpdf-measure{EXE}"
    if kind == "core" and not os.environ.get("OPENPDF_MEASURE_BIN") and measure_bin.is_file():
        return measure_core_bin(measure_bin, args.runs, args.warmup)
    return 0


if __name__ == "__main__":
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(errors="replace")
    raise SystemExit(main())
