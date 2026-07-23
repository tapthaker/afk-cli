#!/usr/bin/env python3
"""Verify that a Linux release artifact has no dynamic ELF dependencies."""

from __future__ import annotations

import argparse
from dataclasses import asdict, dataclass
import json
import os
from pathlib import Path
import re
import subprocess
import sys
from typing import Sequence


_INTERP_SEGMENT = re.compile(r"^\s*INTERP\s", re.MULTILINE)
_INTERP_PATH = re.compile(r"Requesting program interpreter:\s*([^\]]+)\]")
_NEEDED = re.compile(r"\(NEEDED\).*Shared library:\s*\[([^\]]+)\]")
_ELF_CLASS = re.compile(r"^\s*Class:\s*(.+?)\s*$", re.MULTILINE)
_MACHINE = re.compile(r"^\s*Machine:\s*(.+?)\s*$", re.MULTILINE)


@dataclass(frozen=True)
class ElfReport:
    path: str
    elf_class: str
    machine: str
    interpreter: str | None
    needed: tuple[str, ...]

    @property
    def is_static(self) -> bool:
        return self.interpreter is None and not self.needed


def parse_report(
    path: Path,
    elf_header: str,
    program_headers: str,
    dynamic_section: str,
) -> ElfReport:
    """Parse stable fields from GNU or LLVM readelf output."""

    elf_class_match = _ELF_CLASS.search(elf_header)
    machine_match = _MACHINE.search(elf_header)
    if elf_class_match is None or machine_match is None:
        raise ValueError("readelf output does not contain a valid ELF class and machine")

    interpreter_match = _INTERP_PATH.search(program_headers)
    has_interpreter_segment = _INTERP_SEGMENT.search(program_headers) is not None
    if has_interpreter_segment and interpreter_match is None:
        interpreter = "<present but path was not reported>"
    elif interpreter_match is not None:
        interpreter = interpreter_match.group(1).strip()
    else:
        interpreter = None

    return ElfReport(
        path=str(path),
        elf_class=elf_class_match.group(1).strip(),
        machine=machine_match.group(1).strip(),
        interpreter=interpreter,
        needed=tuple(_NEEDED.findall(dynamic_section)),
    )


def run_readelf(readelf: str, option: str, binary: Path) -> str:
    try:
        completed = subprocess.run(
            [readelf, option, str(binary)],
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=30,
        )
    except FileNotFoundError as error:
        raise RuntimeError(f"readelf executable not found: {readelf}") from error
    except subprocess.TimeoutExpired as error:
        raise RuntimeError(f"readelf timed out while inspecting {binary}") from error

    if completed.returncode != 0:
        detail = completed.stderr.strip() or "no diagnostic"
        raise RuntimeError(f"{readelf} {option} failed: {detail}")
    return completed.stdout


def inspect(binary: Path, readelf: str) -> ElfReport:
    if not binary.is_file():
        raise ValueError(f"artifact is not a file: {binary}")

    return parse_report(
        binary,
        run_readelf(readelf, "-hW", binary),
        run_readelf(readelf, "-lW", binary),
        run_readelf(readelf, "-dW", binary),
    )


def format_failure(report: ElfReport) -> str:
    reasons: list[str] = []
    if report.interpreter is not None:
        reasons.append(f"PT_INTERP={report.interpreter}")
    if report.needed:
        reasons.append(f"DT_NEEDED={','.join(report.needed)}")
    return "; ".join(reasons)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Fail if an ELF artifact has a PT_INTERP segment or DT_NEEDED entries."
    )
    parser.add_argument("artifact", type=Path)
    parser.add_argument(
        "--readelf",
        default=os.environ.get("READELF", "readelf"),
        help="readelf-compatible executable (default: READELF or readelf)",
    )
    parser.add_argument("--json", action="store_true", help="emit the inspection report as JSON")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        report = inspect(args.artifact, args.readelf)
    except (RuntimeError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2

    if args.json:
        payload = asdict(report)
        payload["needed"] = list(report.needed)
        payload["is_static"] = report.is_static
        print(json.dumps(payload, sort_keys=True))
    elif report.is_static:
        print(f"static ELF: {report.path} ({report.machine}, {report.elf_class})")
    else:
        print(f"dynamic ELF dependencies: {format_failure(report)}", file=sys.stderr)

    return 0 if report.is_static else 1


if __name__ == "__main__":
    raise SystemExit(main())
