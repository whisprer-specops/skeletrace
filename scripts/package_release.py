#!/usr/bin/env python3
# Copyright (C) 2026 whispr.dev
# SPDX-License-Identifier: GPL-3.0-only
"""Build the release program and pack it for download, with a SHA-256 checksum.

Usage (from the repository root):
    Windows:  py scripts\\package_release.py
    Linux:    python3 scripts/package_release.py

It runs `cargo build --locked --release`, then writes into dist/:
    skeletrace-<version>-<platform>.zip         the program, README.md, LICENSE and CHANGELOG.md
    skeletrace-<version>-<platform>.zip.sha256  the zip's SHA-256, as `sha256sum --check` reads it

The zip is repeatable: the same program file always gives the same zip, byte for byte, because every entry has a
fixed date and the entries are in a fixed order. Run the checks (scripts/check.ps1) before packaging.
Needs Python 3.8 or newer and only the standard library.
"""
from __future__ import annotations

import argparse
import hashlib
import platform
import re
import subprocess
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
# The date stamped on every zip entry. Zip dates can't go before 1980.
FIXED_DATE = (1980, 1, 1, 0, 0, 0)
DOCS = ('README.md', 'LICENSE', 'CHANGELOG.md')


def package_version() -> str:
    """The `version` line of the [package] table in Cargo.toml."""
    text = (ROOT / 'Cargo.toml').read_text(encoding='utf-8')
    package = re.search(r'^\[package\]\s*$(.*?)(?=^\[|\Z)', text, re.MULTILINE | re.DOTALL)
    version = package and re.search(r'^version\s*=\s*"([^"]+)"', package.group(1), re.MULTILINE)
    if not version:
        raise SystemExit('Cannot find the package version in Cargo.toml.')
    return version.group(1)


def platform_tag() -> str:
    """windows-x86_64, linux-x86_64, macos-aarch64 and so on, for the machine this runs on."""
    system = {'Windows': 'windows', 'Linux': 'linux', 'Darwin': 'macos'}.get(platform.system(), platform.system().lower())
    machine = platform.machine().lower()
    machine = {'amd64': 'x86_64', 'x64': 'x86_64', 'arm64': 'aarch64'}.get(machine, machine)
    return f'{system}-{machine}'


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open('rb') as handle:
        for block in iter(lambda: handle.read(1 << 20), b''):
            digest.update(block)
    return digest.hexdigest()


def write_zip(zip_path: Path, folder: str, members: list[tuple[str, Path]]) -> None:
    """Writes the members into folder/ inside the zip, in the order given, all with the fixed date."""
    with zipfile.ZipFile(zip_path, 'w', compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name, source in members:
            info = zipfile.ZipInfo(f'{folder}/{name}', date_time=FIXED_DATE)
            info.compress_type = zipfile.ZIP_DEFLATED
            # Unix permissions: the program can run, the documents can be read.
            mode = 0o755 if name.startswith('skeletrace') and not name.endswith('.md') else 0o644
            info.external_attr = (0o100000 | mode) << 16
            archive.writestr(info, source.read_bytes(), compresslevel=9)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--skip-build', action='store_true',
                        help='pack the program already in target/release instead of building it first')
    args = parser.parse_args()

    version = package_version()
    if not args.skip_build:
        print('Building the release program (cargo build --locked --release)...', flush=True)
        built = subprocess.run(['cargo', 'build', '--locked', '--release'], cwd=ROOT, check=False)
        if built.returncode != 0:
            print('The build failed, so nothing was packed.', file=sys.stderr)
            return 1
    exe = 'skeletrace.exe' if platform.system() == 'Windows' else 'skeletrace'
    program = ROOT / 'target' / 'release' / exe
    if not program.is_file():
        print(f'No program at {program}: build it first.', file=sys.stderr)
        return 1
    missing = [name for name in DOCS if not (ROOT / name).is_file()]
    if missing:
        print(f'Missing from the repository root: {", ".join(missing)}', file=sys.stderr)
        return 1

    stem = f'skeletrace-{version}-{platform_tag()}'
    dist = ROOT / 'dist'
    dist.mkdir(exist_ok=True)
    zip_path = dist / f'{stem}.zip'
    write_zip(zip_path, stem, [(exe, program)] + [(name, ROOT / name) for name in DOCS])
    digest = sha256_of(zip_path)
    checksum_path = dist / f'{zip_path.name}.sha256'
    checksum_path.write_text(f'{digest}  {zip_path.name}\n', encoding='ascii', newline='\n')

    print(f'Packed:   {zip_path.relative_to(ROOT)} ({zip_path.stat().st_size:,} bytes)')
    print(f'Checksum: {checksum_path.relative_to(ROOT)}')
    print(f'SHA-256:  {digest}')
    print('To check a download on Windows:  (Get-FileHash .\\' + zip_path.name + ' -Algorithm SHA256).Hash')
    print('To check a download on Linux:    sha256sum --check ' + checksum_path.name)
    return 0


if __name__ == '__main__':
    sys.exit(main())
