#!/usr/bin/env bash
# gdb-on-signal.sh RC LABEL: if RC > 128 (launch ended by a signal), rerun the same
# --measure launch under gdb (same display/backend/env, no strace) and save the
# backtrace to crash/gdb-LABEL.txt; print the crashing thread's top ~30 frames.
set -u
rc="$1"; label="$2"
[ "$rc" -gt 128 ] 2>/dev/null || exit 0
mkdir -p crash
echo "::group::gdb backtrace ($label, exit $rc = signal $((rc - 128)))"
gdbwrap="gdb -batch -nx -ex 'set debuginfod enabled off' -ex 'set pagination off' -ex run -ex 'thread apply all bt' --args"
OPENPDF_MEASURE_WRAP="${GDB_BASE_WRAP:-} $gdbwrap" OPENPDF_ENV_DUMP= \
  python3 .github/scripts/run-measure.py --kind gui --single-cold --frames 32 \
  --out "crash/gdb-$label.out" > "crash/gdb-$label.txt" 2>&1 || true
echo "::endgroup::"
python3 - "crash/gdb-$label.txt" <<'PY'
import re, sys
t = open(sys.argv[1], errors="replace").read().splitlines()
sig = next((l for l in t if "received signal" in l), None)
print(sig or "gdb: no 'received signal' line (crash not reproduced under gdb?)")
tid = re.search(r"Thread (\d+)", sig).group(1) if sig and re.search(r"Thread (\d+)", sig) else None
frames, on = [], tid is None
for l in t:
    if l.startswith("Thread "):
        on = tid is None or l.startswith(f"Thread {tid} ")
        if frames and tid is None: break
        continue
    if on and l.startswith("#"): frames.append(l)
for l in frames[:30]: print(l[:240])
PY
