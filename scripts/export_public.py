#!/usr/bin/env python3
# Copyright (C) 2026 whispr.dev
# SPDX-License-Identifier: GPL-3.0-only
"""Copy the files that belong in the public SkeleTrace repository into a new folder, or refresh a clone of it.

Usage (from the repository root):
    Windows:  py scripts\\export_public.py ..\\skeletrace-public
    Linux:    python3 scripts/export_public.py ../skeletrace-public

This private repository's history holds copies of separately sold tools, old archives and internal notes, so the
public repository starts from a clean copy instead of this one. Only files git tracks are copied, and only those
on the allowlist below: anything new stays private until it is added here.

The folder must be new or empty, or a clone of the public repository with nothing uncommitted. In a clone,
everything but its .git folder is replaced by the export, so files removed here disappear there too, and
`git status` in the clone shows exactly what the next public commit changes.
Needs Python 3.8 or newer, git, and only the standard library.
"""
from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parents[1]

# Files at the top level that go public.
ROOT_FILES = {
    '.gitattributes', '.gitignore', 'Cargo.lock', 'Cargo.toml', 'CHANGELOG.md', 'CODE_OF_CONDUCT.md',
    'CONTRIBUTING.md', 'LICENSE', 'README.md', 'rust-toolchain.toml', 'SECURITY.md',
}
# Folders that go public whole.
FOLDERS = {'.github', 'assets', 'packaging', 'scripts', 'skeletrace-algo', 'src', 'tests'}
# Single files that go public from folders that also hold private or out-of-date material.
FILES = {
    'examples/standalone_demo.rs',
    'examples/lookyloo_source_profile.json',
    'examples/lookyloo_topology_source_profile.json',
}


def tracked_files() -> list[str]:
    listing = subprocess.run(['git', 'ls-files', '-z'], cwd=ROOT, capture_output=True, check=True)
    return [name for name in listing.stdout.decode('utf-8').split('\0') if name]


def is_public(name: str) -> bool:
    path = PurePosixPath(name)
    if len(path.parts) == 1:
        return name in ROOT_FILES
    return path.parts[0] in FOLDERS or name in FILES


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('destination', type=Path,
                        help='a new or empty folder, or a clean clone of the public repository, outside this one')
    args = parser.parse_args()
    destination = args.destination.resolve()
    if destination == ROOT or ROOT in destination.parents:
        print('Choose a folder outside this repository.', file=sys.stderr)
        return 2
    clone = (destination / '.git').exists()
    if clone:
        try:
            status = subprocess.run(['git', 'status', '--porcelain'], cwd=destination, capture_output=True,
                                    check=True).stdout
        except (OSError, subprocess.CalledProcessError) as error:
            print(f'Cannot read the clone with git: {error}', file=sys.stderr)
            return 1
        if status.strip():
            print(f'{destination} has uncommitted changes. Commit or remove them first.', file=sys.stderr)
            return 2
    elif destination.exists() and any(destination.iterdir()):
        print(f'{destination} is not empty. Choose a new or empty folder, or a clone of the public repository.',
              file=sys.stderr)
        return 2
    try:
        names = tracked_files()
    except (OSError, subprocess.CalledProcessError) as error:
        print(f'Cannot list the tracked files with git: {error}', file=sys.stderr)
        return 1
    public = [name for name in names if is_public(name)]
    missing = sorted((ROOT_FILES | FILES) - set(public))
    if missing:
        print(f'Expected but not tracked: {", ".join(missing)}', file=sys.stderr)
        return 1
    if clone:
        for entry in destination.iterdir():
            if entry.name == '.git':
                continue
            if entry.is_dir() and not entry.is_symlink():
                shutil.rmtree(entry)
            else:
                entry.unlink()
    for name in public:
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / name, target)
    left_out = sorted({PurePosixPath(name).parts[0] for name in names if not is_public(name)})
    print(f'Copied {len(public)} files into {destination}')
    print(f'Left out {len(names) - len(public)} files, under: {", ".join(left_out)}')
    if clone:
        print('Next: run the checks in that folder, then look at `git status` there and commit.')
    else:
        print('Next: run the checks in that folder, then make it the first commit of the public repository.')
    return 0


if __name__ == '__main__':
    sys.exit(main())
