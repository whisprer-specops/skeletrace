# Contributing to SkeleTrace

## Copyright first

whispr.dev holds the copyright in SkeleTrace. Before starting work on a change you would
like merged, open an issue or write to info@skeletrace.com, so the terms for your
contribution can be agreed first. Bug reports need no agreement: see the end of this file.

Every code file opens with these two lines, as comments in its own language, after any
`#!` line. `scripts/audit_standalone.py` fails a file without them.

```text
Copyright (C) 2026 whispr.dev
SPDX-License-Identifier: GPL-3.0-only
```

## Building and checking

Use Rust 1.97.0, as pinned in `rust-toolchain.toml` (rustup installs it on the first
build), and a C build toolchain for bundled SQLite. The code keeps to Rust 1.87, the
declared minimum in `Cargo.toml`. Work on the standalone engine; keep separately distributed tools
out of Cargo dependencies, operator enums and CLI commands.

Build and validate from the project root:

```text
cargo fmt --all --check
cargo test --locked --all-targets
cargo test --locked --doc
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
cargo run --locked --release --example standalone_demo -- ./output/demo
```

On Windows, `powershell -ExecutionPolicy Bypass -File .\scripts\check.ps1` runs all of
these; `scripts/check.sh` does the same elsewhere.

Contract tests belong in `tests/`; offline examples belong in `examples/`. Use
recorded responses or local fixtures for adapters. Explain any dependency change
and update `Cargo.lock` intentionally. Preserve profile and serialization contracts
unless a documented breaking change is required.

Bug reports should include the version, operating system, exact command, expected
result and redacted diagnostic output. Never include credentials, private case
files or real personal data in public reports. Keep discussion constructive and
follow `CODE_OF_CONDUCT.md`.
