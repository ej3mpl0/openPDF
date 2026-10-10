#!/usr/bin/env python3
"""Compare cache snapshots (cache-snapshot.sh). Prints, per top-level cache dir
(~/.cache/<name>, /var/cache/fontconfig), files/bytes before and after and what
appeared or grew, for each consecutive pair of snapshots given."""
import os
import sys
from collections import defaultdict

HOME_CACHE = os.path.expanduser("~/.cache")


def load(p):
    files = {}
    for line in open(p, encoding="utf-8", errors="replace"):
        parts = line.rstrip("\n").split("\t", 2)
        if len(parts) == 3 and parts[1] == "f":
            files[parts[2]] = int(parts[0])
    return files


def top(path):
    if path.startswith(HOME_CACHE + "/"):
        rest = path[len(HOME_CACHE) + 1:]
        return "~/.cache/" + rest.split("/", 1)[0]
    if path.startswith("/var/cache/fontconfig"):
        return "/var/cache/fontconfig"
    return path


def agg(files):
    a = defaultdict(lambda: [0, 0])
    for p, s in files.items():
        a[top(p)][0] += 1
        a[top(p)][1] += s
    return a


def main(args):
    names = [os.path.basename(a).removesuffix(".txt") for a in args]
    snaps = [load(a) for a in args]
    for i in range(1, len(snaps)):
        before, after = snaps[i - 1], snaps[i]
        ab, aa = agg(before), agg(after)
        print(f"### {names[i-1]} -> {names[i]}")
        rows = []
        for d in sorted(set(ab) | set(aa)):
            fb, sb = ab.get(d, [0, 0])
            fa, sa = aa.get(d, [0, 0])
            if (fb, sb) != (fa, sa):
                tag = "NEW" if fb == 0 else "grew" if sa > sb else "changed"
                rows.append(f"- {tag} `{d}`: {fb} files/{sb} B -> {fa} files/{sa} B")
        new_files = sorted(p for p in after if p not in before)
        print("\n".join(rows) if rows else "- no change")
        if new_files:
            print(f"  new files ({len(new_files)}), first 15:")
            for p in new_files[:15]:
                print(f"  - {p} ({after[p]} B)")
        print()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
