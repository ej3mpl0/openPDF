#!/usr/bin/env python3
"""Fail if English msgids in source are missing from po/openpdf.pot or po/es.po."""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
POT = ROOT / "po" / "openpdf.pot"
ES = ROOT / "po" / "es.po"
POTFILES = ROOT / "po" / "POTFILES.in"

# Brand / keys that stay English in every language.
IDENTITY_OK = {"openPDF", "openpdf: {error}"}

CALL_START_RE = re.compile(r"\b(?:tr|tr_args|t)\s*\(")

DESKTOP_RE = re.compile(
    r"^(Name|Comment|GenericName|Keywords)=(.+)$"
)

META_RE = re.compile(
    r"<(name|summary|caption)>([^<]+)</(?:name|summary|caption)>",
    re.I,
)
META_P_RE = re.compile(r"<p>\s*([^<]+?)\s*</p>", re.I | re.S)


VALID_ESCAPES = {"n": "\n", "t": "\t", "r": "\r", '"': '"', "\\": "\\"}


def unescape(s: str) -> str:
    out = []
    it = iter(s)
    for c in it:
        if c != "\\":
            out.append(c)
            continue
        n = next(it, "")
        out.append(VALID_ESCAPES.get(n, "\\" + n))
    return "".join(out)


def escape_errors(quoted_inner: str, *, path: Path, lineno: int) -> list[str]:
    """Flag dangling backslashes and unknown escape sequences in a po string."""
    errors: list[str] = []
    i = 0
    while i < len(quoted_inner):
        if quoted_inner[i] != "\\":
            i += 1
            continue
        if i + 1 >= len(quoted_inner):
            errors.append(f"{path}:{lineno}: dangling backslash in quoted string")
            break
        nxt = quoted_inner[i + 1]
        if nxt not in VALID_ESCAPES:
            errors.append(
                f"{path}:{lineno}: invalid escape \\{nxt} in quoted string"
            )
        i += 2
    return errors


def parse_po(path: Path) -> dict[str, str]:
    entries, errors = parse_po_checked(path)
    if errors:
        raise ValueError("\n".join(errors))
    return entries


def parse_po_checked(path: Path) -> tuple[dict[str, str], list[str]]:
    entries: dict[str, str] = {}
    first_line: dict[str, int] = {}
    errors: list[str] = []
    msgid = ""
    msgstr = ""
    field = None
    msgid_line = 0

    def flush() -> None:
        nonlocal msgid, msgstr, field, msgid_line
        if msgid:
            if msgid.endswith("\\"):
                errors.append(
                    f"{path}:{msgid_line}: msgid ends with a backslash "
                    f"(broken escape) {msgid!r}"
                )
            if msgid in entries:
                errors.append(
                    f"{path}:{msgid_line}: duplicate msgid {msgid!r} "
                    f"(first at line {first_line[msgid]})"
                )
            else:
                first_line[msgid] = msgid_line
            entries[msgid] = msgstr
        msgid = ""
        msgstr = ""
        field = None
        msgid_line = 0

    def take_quoted(line: str, lineno: int) -> str:
        raw = line.strip()
        inner = raw[1:-1] if raw.startswith('"') and raw.endswith('"') and len(raw) >= 2 else raw.strip('"')
        errors.extend(escape_errors(inner, path=path, lineno=lineno))
        return unescape(inner)

    for lineno, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            flush()
            continue
        if line.startswith("msgid "):
            flush()
            field = "id"
            msgid_line = lineno
            msgid = take_quoted(line[6:], lineno)
        elif line.startswith("msgstr "):
            field = "str"
            msgstr = take_quoted(line[7:], lineno)
        elif line.startswith("msgctxt "):
            flush()
        elif line.startswith('"'):
            chunk = take_quoted(line, lineno)
            if field == "id":
                msgid += chunk
            elif field == "str":
                msgstr += chunk
    flush()
    entries.pop("", None)
    return entries, errors


def rust_msgids(text: str) -> set[str]:
    found: set[str] = set()
    for m in CALL_START_RE.finditer(text):
        i = m.end()
        depth = 1
        while i < len(text) and depth:
            c = text[i]
            if c == '"':
                i += 1
                start = i
                while i < len(text):
                    if text[i] == "\\":
                        i += 2
                        continue
                    if text[i] == '"':
                        found.add(unescape(text[start:i]))
                        depth = 0
                        break
                    i += 1
                break
            if c == "(":
                depth += 1
            elif c == ")":
                depth -= 1
            i += 1
    return found


def desktop_msgids(text: str) -> set[str]:
    found: set[str] = set()
    for line in text.splitlines():
        if re.match(r"^(Name|Comment|GenericName|Keywords)\[", line):
            continue
        m = DESKTOP_RE.match(line)
        if m:
            found.add(m.group(2))
    return found


def metainfo_msgids(text: str) -> set[str]:
    found: set[str] = set()
    for m in META_RE.finditer(text):
        found.add(re.sub(r"\s+", " ", m.group(2)).strip())
    for m in META_P_RE.finditer(text):
        found.add(re.sub(r"\s+", " ", m.group(1)).strip())
    return found


def listed_files() -> list[Path]:
    files: list[Path] = []
    for raw in POTFILES.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        files.append(ROOT / line)
    extra_roots = [
        ROOT / "crates" / "openpdf-core" / "src",
        ROOT / "crates" / "openpdf-cli" / "src",
        ROOT / "crates" / "openpdf-gtk" / "src",
        ROOT / "crates" / "openpdf-app" / "src",
        ROOT / "data",
    ]
    seen = {p.resolve() for p in files}
    for root in extra_roots:
        if not root.exists():
            continue
        for path in root.rglob("*"):
            if not path.is_file():
                continue
            if path.suffix not in {".rs", ".desktop", ".xml"}:
                continue
            if path.resolve() not in seen:
                files.append(path)
                seen.add(path.resolve())
    return files


def extract_from(path: Path) -> set[str]:
    text = path.read_text(encoding="utf-8")
    if path.suffix == ".rs":
        cut = text.find("#[cfg(test)]")
        if cut != -1:
            text = text[:cut]
        return rust_msgids(text)
    if path.suffix == ".desktop":
        return desktop_msgids(text)
    if path.name.endswith(".metainfo.xml") or path.name.endswith(".appdata.xml"):
        return metainfo_msgids(text)
    return set()


def main() -> int:
    pot, pot_errs = parse_po_checked(POT)
    es, es_errs = parse_po_checked(ES)
    errors: list[str] = []
    errors.extend(pot_errs)
    errors.extend(es_errs)
    source: dict[str, list[str]] = {}
    missing_files = []
    for path in listed_files():
        if not path.exists():
            missing_files.append(str(path.relative_to(ROOT)))
            continue
        for msgid in extract_from(path):
            source.setdefault(msgid, []).append(str(path.relative_to(ROOT)))

    if missing_files:
        errors.append("POTFILES.in lists missing files:\n  " + "\n  ".join(missing_files))

    missing_pot = sorted(m for m in source if m not in pot)
    if missing_pot:
        lines = [f"  {m!r}  ({', '.join(source[m])})" for m in missing_pot]
        errors.append("msgids used in source but missing from po/openpdf.pot:\n" + "\n".join(lines))

    missing_es = sorted(m for m in source if m not in es and m not in IDENTITY_OK)
    if missing_es:
        lines = [f"  {m!r}  ({', '.join(source[m])})" for m in missing_es]
        errors.append("msgids used in source but missing from po/es.po:\n" + "\n".join(lines))

    empty = sorted(m for m, s in es.items() if not s)
    if empty:
        errors.append("empty msgstr in po/es.po:\n  " + "\n  ".join(empty))

    if errors:
        print("po catalogs are out of date:\n", file=sys.stderr)
        print("\n\n".join(errors), file=sys.stderr)
        return 1
    print(
        f"ok: {len(source)} source msgids, {len(pot)} pot, {len(es)} es.po"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
