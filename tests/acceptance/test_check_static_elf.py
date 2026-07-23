from __future__ import annotations

from contextlib import redirect_stderr, redirect_stdout
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).parent))

import check_static_elf


ELF_HEADER = """\
ELF Header:
  Class:                             ELF64
  Machine:                           Advanced Micro Devices X86-64
"""

STATIC_PROGRAM_HEADERS = """\
Elf file type is DYN (Position-Independent Executable file)
Program Headers:
  Type           Offset             VirtAddr
  LOAD           0x0000000000000000 0x0000000000000000
"""

DYNAMIC_PROGRAM_HEADERS = """\
Elf file type is DYN (Position-Independent Executable file)
Program Headers:
  Type           Offset             VirtAddr
  INTERP         0x0000000000000350 0x0000000000000350
      [Requesting program interpreter: /lib64/ld-linux-x86-64.so.2]
"""

DYNAMIC_SECTION = """\
Dynamic section at offset 0x2db0 contains 2 entries:
  Tag        Type                         Name/Value
 0x00000001 (NEEDED)                     Shared library: [libgcc_s.so.1]
 0x00000001 (NEEDED)                     Shared library: [libc.so.6]
"""


class ParseReportTests(unittest.TestCase):
    def test_static_pie_has_no_dynamic_dependencies(self) -> None:
        report = check_static_elf.parse_report(
            Path("afk"),
            ELF_HEADER,
            STATIC_PROGRAM_HEADERS,
            "There is no dynamic section in this file.\n",
        )

        self.assertTrue(report.is_static)
        self.assertIsNone(report.interpreter)
        self.assertEqual(report.needed, ())
        self.assertEqual(report.machine, "Advanced Micro Devices X86-64")

    def test_interpreter_and_needed_libraries_are_reported(self) -> None:
        report = check_static_elf.parse_report(
            Path("afk"),
            ELF_HEADER,
            DYNAMIC_PROGRAM_HEADERS,
            DYNAMIC_SECTION,
        )

        self.assertFalse(report.is_static)
        self.assertEqual(report.interpreter, "/lib64/ld-linux-x86-64.so.2")
        self.assertEqual(report.needed, ("libgcc_s.so.1", "libc.so.6"))
        self.assertEqual(
            check_static_elf.format_failure(report),
            "PT_INTERP=/lib64/ld-linux-x86-64.so.2; "
            "DT_NEEDED=libgcc_s.so.1,libc.so.6",
        )

    def test_unparsed_interp_segment_still_fails(self) -> None:
        report = check_static_elf.parse_report(
            Path("afk"),
            ELF_HEADER,
            "Program Headers:\n  INTERP         0x0000000000000350\n",
            "There is no dynamic section in this file.\n",
        )

        self.assertFalse(report.is_static)
        self.assertEqual(report.interpreter, "<present but path was not reported>")

    def test_invalid_header_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "valid ELF class and machine"):
            check_static_elf.parse_report(
                Path("afk"),
                "not an ELF header",
                STATIC_PROGRAM_HEADERS,
                "There is no dynamic section in this file.\n",
            )


class CommandTests(unittest.TestCase):
    def test_json_output_includes_empty_dependency_inventory(self) -> None:
        report = check_static_elf.ElfReport(
            path="afk",
            elf_class="ELF64",
            machine="AArch64",
            interpreter=None,
            needed=(),
        )

        stdout = io.StringIO()
        with mock.patch.object(check_static_elf, "inspect", return_value=report), redirect_stdout(
            stdout
        ):
            result = check_static_elf.main(["afk", "--json"])

        self.assertEqual(result, 0)
        payload = json.loads(stdout.getvalue())
        self.assertTrue(payload["is_static"])
        self.assertEqual(payload["needed"], [])

    def test_dynamic_artifact_fails(self) -> None:
        report = check_static_elf.ElfReport(
            path="afk",
            elf_class="ELF64",
            machine="Advanced Micro Devices X86-64",
            interpreter="/lib64/ld-linux-x86-64.so.2",
            needed=("libc.so.6",),
        )

        with mock.patch.object(check_static_elf, "inspect", return_value=report), redirect_stderr(
            io.StringIO()
        ):
            self.assertEqual(check_static_elf.main(["afk"]), 1)

    def test_missing_artifact_is_an_inspection_error(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory, redirect_stderr(io.StringIO()):
            missing = Path(temporary_directory) / "afk"
            self.assertEqual(check_static_elf.main([str(missing)]), 2)


if __name__ == "__main__":
    unittest.main()
