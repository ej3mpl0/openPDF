#!/usr/bin/env python3
"""Generate Localizable.strings from the shared po catalog (en + es)."""

from __future__ import annotations

import argparse
import os
import re
from pathlib import Path


ENTRY = re.compile(
    r'msgid\s+("(?:\\.|[^"])*")\s+msgstr\s+("(?:\\.|[^"])*")',
    re.MULTILINE,
)


def unescape(quoted: str) -> str:
    return bytes(quoted[1:-1], "utf-8").decode("unicode_escape")


def parse_po(path: Path) -> dict[str, str]:
    text = path.read_text(encoding="utf-8")
    out: dict[str, str] = {}
    for match in ENTRY.finditer(text):
        msgid = unescape(match.group(1))
        msgstr = unescape(match.group(2))
        if msgid:
            out[msgid] = msgstr
    return out


def write_strings(path: Path, entries: dict[str, str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    lines = []
    for msgid, msgstr in entries.items():
        lines.append(f'"{escape(msgid)}" = "{escape(msgstr)}";')
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def escape(value: str) -> str:
    return value.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--po", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()
    po_dir = Path(args.po)
    out_dir = Path(args.out)
    pot = parse_po(po_dir / "openpdf.pot")
    es = parse_po(po_dir / "es.po")
    write_strings(out_dir / "en.lproj" / "Localizable.strings", {k: k for k in pot})
    write_strings(
        out_dir / "es.lproj" / "Localizable.strings",
        {k: (es.get(k) or k) for k in pot},
    )


if __name__ == "__main__":
    main()
