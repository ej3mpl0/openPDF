#!/usr/bin/env python3
"""Env parity: the app's normal launch env vs the env --measure really ran with.

usage: env-parity.py BASELINE MEASURED [MEASURED...] --allow KEY=VALUE ...

Compares XDG_*, FONTCONFIG_*, GTK_*, GDK_*, GSK_RENDERER. A difference fails
(exit 1) unless KEY is a declared per-variant exception whose measured value is
exactly VALUE. All exceptions and differences are printed."""
import re
import sys

WATCH = re.compile(r"^(XDG_|FONTCONFIG_|GTK_|GDK_)|^GSK_RENDERER$")


def load(p):
    env = {}
    for line in open(p, encoding="utf-8", errors="replace"):
        k, sep, v = line.rstrip("\n").partition("=")
        if sep and WATCH.search(k):
            env[k] = v
    return env


def main(argv):
    allow, files = {}, []
    it = iter(argv)
    for a in it:
        if a == "--allow":
            k, _, v = next(it).partition("=")
            allow[k] = v
        else:
            files.append(a)
    base = load(files[0])
    print("declared exceptions: " + (", ".join(f"{k}={v}" for k, v in sorted(allow.items())) or "(none)"))
    print(f"baseline ({files[0]}): " + (" ".join(f"{k}={v}" for k, v in sorted(base.items())) or "(no watched vars)"))
    bad = 0
    for f in files[1:]:
        meas = load(f)
        before = bad
        for k in sorted(set(base) | set(meas) | set(allow)):
            b, m = base.get(k), meas.get(k)
            if k in allow:
                if m == allow[k]:
                    print(f"  ok (exception) {f}: {k}={m} (baseline {b!r})")
                else:
                    print(f"::error::env parity {f}: declared exception {k}={allow[k]!r} but measured {m!r}")
                    bad += 1
            elif b != m:
                print(f"::error::env parity {f}: {k} baseline={b!r} measured={m!r} (not a declared exception)")
                bad += 1
        print(f"{f}: {'OK' if bad == before else 'MISMATCH'}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
