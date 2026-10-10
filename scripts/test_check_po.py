#!/usr/bin/env python3
"""Unit tests for check_po duplicate msgid and escape validation."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import check_po


HEADER = """msgid ""
msgstr ""
"Language: es\\n"
"""


class CheckPoTests(unittest.TestCase):
    def _parse(self, body: str) -> tuple[dict[str, str], list[str]]:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "es.po"
            path.write_text(HEADER + body, encoding="utf-8")
            return check_po.parse_po_checked(path)

    def test_duplicate_msgid(self) -> None:
        body = """
msgid "Enter PIN"
msgstr "Introducir PIN"

msgid "Enter PIN"
msgstr "PIN"
"""
        _entries, errors = self._parse(body)
        self.assertTrue(any("duplicate msgid" in e for e in errors), errors)

    def test_broken_escape_trailing_backslash(self) -> None:
        # Same corruption as gtk-cert-unusable po/es.po:633.
        body = r"""
msgid "Enter PIN for token \"{token}\""
msgstr "ok"

msgid "Enter PIN for token \"{token}\\"
msgstr "Introduzca el PIN del token \"{token}\\"
"""
        _entries, errors = self._parse(body)
        self.assertTrue(
            any("broken escape" in e or "dangling backslash" in e for e in errors),
            errors,
        )

    def test_invalid_escape_sequence(self) -> None:
        body = r"""
msgid "bad \q escape"
msgstr "x"
"""
        _entries, errors = self._parse(body)
        self.assertTrue(any("invalid escape" in e for e in errors), errors)

    def test_clean_catalog_has_no_errors(self) -> None:
        body = r"""
msgid "Enter PIN for token \"{token}\""
msgstr "Introduzca el PIN del token \"{token}\""
"""
        _entries, errors = self._parse(body)
        self.assertEqual(errors, [])


if __name__ == "__main__":
    unittest.main()
