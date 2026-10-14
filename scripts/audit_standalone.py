#!/usr/bin/env python3
# Copyright (C) 2026 whispr.dev
# SPDX-License-Identifier: GPL-3.0-only
"""Check that the shipped Rust source has no extracted-tool wiring or orphan modules, and that every code file
opens with its copyright and licence lines."""
from __future__ import annotations

import json
from pathlib import Path
import re
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
BANNED_DEPENDENCIES = {
    'maxwells-demon-detector', 'osintropy', 'osintropy-py', 'crowdwatch',
    'clap', 'crossterm', 'pcap', 'csv', 'sha2',
}
# SkeleTrace's own std-only algorithm crate, kept in this repository. It is part of the engine, not an
# extracted tool, so it is the one path dependency allowed.
ALLOWED_PATH_DEPENDENCIES = {'skeletrace-algo'}
BANNED_IDENTIFIERS = re.compile(
    r'\b(?:maxwells_demon_detector|osintropy|crowdwatch|behavioral_sink|behavioural_sink|'
    r'LandscapeRequest|LandscapeReport|LandscapeError|CaseCommand|CaseResponse|CaseStore|'
    r'CaseLoadReport|LoadCase|PacketVerificationRequest)\b', re.IGNORECASE
)
MODULE = re.compile(r'^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;', re.MULTILINE)
# Every code file opens with these two lines, after any #! line (see CONTRIBUTING.md).
HEADER_COPYRIGHT = re.compile(r'Copyright \(C\) 20\d\d(?:-20\d\d)? whispr\.dev')
HEADER_SPDX = 'SPDX-License-Identifier: GPL-3.0-only'
CODE_FOLDERS = ['src', 'tests', 'examples', 'scripts', 'packaging', 'skeletrace-algo/src',
                'skeletrace-algo/examples', 'skeletrace-algo/tests']
CODE_SUFFIXES = {'.rs', '.py', '.ps1', '.sh'}


def code_files() -> list[Path]:
    """The code files git tracks in CODE_FOLDERS; outside a git checkout, every one on disk (build output aside)."""
    try:
        listing = subprocess.run(['git', 'ls-files', '-z', '--', *CODE_FOLDERS], cwd=ROOT, capture_output=True,
                                 check=True)
        names = [name for name in listing.stdout.decode('utf-8').split('\0') if name]
    except (OSError, subprocess.CalledProcessError):
        names = []
    if not names:
        names = [path.relative_to(ROOT).as_posix() for folder in CODE_FOLDERS if (ROOT / folder).is_dir()
                 for path in (ROOT / folder).rglob('*') if 'target' not in path.relative_to(ROOT).parts]
    return [ROOT / name for name in sorted(names) if Path(name).suffix in CODE_SUFFIXES and (ROOT / name).is_file()]


def has_header(path: Path) -> bool:
    head = path.read_text(encoding='utf-8', errors='replace').splitlines()[:4]
    return any(HEADER_COPYRIGHT.search(line) for line in head) and any(HEADER_SPDX in line for line in head)


def main() -> int:
    errors: list[str] = []
    manifest = tomllib.loads((ROOT / 'Cargo.toml').read_text(encoding='utf-8'))
    lock = tomllib.loads((ROOT / 'Cargo.lock').read_text(encoding='utf-8'))
    packages = {entry['name'] for entry in lock['package']}
    forbidden = packages & BANNED_DEPENDENCIES
    if forbidden:
        errors.append(f'Extracted-tool packages remain: {sorted(forbidden)}')
    if manifest.get('workspace'):
        errors.append('A workspace remains in the standalone manifest')
    if manifest.get('features'):
        errors.append('Embedded-tool features remain in the standalone manifest')
    for section in ['dependencies', 'dev-dependencies', 'build-dependencies']:
        for name, spec in manifest.get(section, {}).items():
            if isinstance(spec, dict) and 'path' in spec and name not in ALLOWED_PATH_DEPENDENCIES:
                errors.append(f'Path dependency remains: {name}')
    if (ROOT / 'crates').exists():
        errors.append('An embedded crates directory remains')
    seen: set[Path] = set()
    pending = [ROOT / 'src/lib.rs', *sorted((ROOT / 'src/bin').glob('*.rs'))]
    while pending:
        path = pending.pop().resolve()
        if path in seen:
            continue
        seen.add(path)
        text = path.read_text(encoding='utf-8')
        if BANNED_IDENTIFIERS.search(text):
            errors.append(f'Extracted-tool identifier in {path.relative_to(ROOT)}')
        if '#[path' in text:
            errors.append(f'Explicit module path requires audit: {path.relative_to(ROOT)}')
        for name in MODULE.findall(text):
            parent = path.parent if path.name in ['lib.rs', 'main.rs', 'mod.rs'] else path.with_suffix('')
            candidates = [parent / f'{name}.rs', parent / name / 'mod.rs']
            existing = [candidate for candidate in candidates if candidate.is_file()]
            if len(existing) != 1:
                errors.append(f'Module {name} in {path.relative_to(ROOT)} resolves to {len(existing)} files')
            else:
                pending.append(existing[0])
    orphaned = {p.resolve() for p in (ROOT / 'src').rglob('*.rs')} - seen
    if orphaned:
        errors.append(f'Unreachable source files: {sorted(str(p.relative_to(ROOT)) for p in orphaned)}')
    checked = code_files()
    unmarked = [path.relative_to(ROOT).as_posix() for path in checked if not has_header(path)]
    if unmarked:
        errors.append(f'Code files without the copyright and licence lines: {unmarked}')
    result = {'ok': not errors, 'reachable_rust_files': len(seen),
              'locked_packages': len(lock['package']), 'code_files_with_header': len(checked) - len(unmarked),
              'errors': errors}
    print(json.dumps(result, indent=2))
    return 0 if not errors else 1


if __name__ == '__main__':
    sys.exit(main())
