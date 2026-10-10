#!/usr/bin/env python3
"""Summarize `strace -f -tt -e trace=openat,connect` output (informational).

Prints: openat counts by category, connect calls with targets, and the top 10
gaps > 50 ms between consecutive syscalls with the syscall just before each.
"""
import re
import sys
from collections import Counter

LINE = re.compile(r"^(?:\[pid\s+)?(\d+)\]?\s+(\d\d):(\d\d):(\d\d\.\d+)\s+(.*)$")


def category(path: str) -> str:
    if "/gio/modules/" in path:
        return "GIO modules"
    if "gdk-pixbuf" in path and "loaders" in path:
        return "gdk-pixbuf loaders"
    if re.search(r"\.so(\.\d+)*$", path):
        return "shared libs (.so)"
    if "/icons/" in path or "/themes/" in path or path.endswith((".theme", "icon-theme.cache")):
        return "icon/theme files"
    if "font" in path.lower() or path.endswith((".ttf", ".otf", ".pfb", ".ttc")):
        return "fonts"
    return "other"


def main(path: str) -> int:
    events = []
    for raw in open(path, encoding="utf-8", errors="replace"):
        m = LINE.match(raw.rstrip("\n"))
        if not m:
            continue
        pid, h, mi, sec, rest = m.groups()
        t = int(h) * 3600 + int(mi) * 60 + float(sec)
        events.append((t, pid, rest))
    events.sort(key=lambda e: e[0])
    opens, connects = Counter(), []
    for _, pid, rest in events:
        if rest.startswith("openat(") or "openat resumed" in rest:
            m = re.search(r'openat\([^,]+,\s*"([^"]*)"', rest)
            if m:
                opens[category(m.group(1))] += 1
        elif rest.startswith("connect("):
            m = re.search(r'sun_path=(@?"[^"]*"|[^,}]+)|sin6?_addr=[^)]*\("([^"]+)"\)', rest)
            res = rest.rsplit("=", 1)[-1].strip()
            target = (m.group(1) or m.group(2)) if m else rest[:120]
            connects.append(f"pid {pid}: {target} -> {res}")
    print(f"## strace summary ({len(events)} syscalls)")
    print("\n### openat by category")
    for k in ["shared libs (.so)", "GIO modules", "gdk-pixbuf loaders", "icon/theme files", "fonts", "other"]:
        print(f"- {k}: {opens[k]}")
    print(f"- total: {sum(opens.values())}")
    print(f"\n### connect calls: {len(connects)}")
    for c in connects:
        print(f"- {c}")
    gaps = []
    for (t0, p0, r0), (t1, _, _) in zip(events, events[1:]):
        d = (t1 - t0) * 1000
        if d > 50:
            gaps.append((d, p0, r0))
    gaps.sort(reverse=True)
    print(f"\n### top 10 gaps > 50 ms ({len(gaps)} total)")
    for d, p0, r0 in gaps[:10]:
        print(f"- {d:.0f} ms after pid {p0}: {r0[:200]}")
    if events:
        print(f"\nspan first->last syscall: {(events[-1][0] - events[0][0]) * 1000:.0f} ms")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else "trace.txt"))
