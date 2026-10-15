# Changelog

## 1.0.2 (2026-10-15)

Packaging only; the engine is unchanged apart from its version number.

- Each release now also carries every download under a name without the version, such
  as `skeletrace-windows-x86_64-setup.exe` beside `skeletrace-1.0.2-windows-x86_64-setup.exe`.
  The copies are identical and `SHA256SUMS.txt` lists both. GitHub serves
  `releases/latest/download/<name>` from the newest release, so skeletrace.com/download/
  can link those names once and always hand out the newest version.
- `packaging/` is now shared word for word with the other whispr.dev programs' release kits. `product.json` may
  name a suggested install folder (`"folder"`), every value placed in the Linux and macOS installer script is
  quoted, so a name with an apostrophe works, and the installed README, LICENSE and CHANGELOG come from the top
  of the repository first.

## 1.0.1 (2026-10-14)

Packaging only; the engine is unchanged apart from its version number.

- Downloads for Linux and macOS, as well as Windows. A GitHub Actions workflow
  (`.github/workflows/release.yml`) builds every release on Windows (x86_64), Linux
  (x86_64 and ARM64) and macOS (one program for Apple silicon and Intel). It runs the
  full checks on each system first, and test-installs each installer before publishing.
  The Linux programs are static, so they run on any distribution.
- Installers with a licence step. Each shows the licence and asks you to accept it, then
  asks which folder to install into, and changes nothing else: no registry entries, no
  PATH changes, no uninstaller. To remove SkeleTrace, delete its folder. Windows gets an
  Inno Setup `-setup.exe`; Linux and macOS get a self-extracting `.run` file. Plain
  `.zip` and `.tar.gz` archives sit beside them, and `SHA256SUMS.txt` lists every file's
  checksum.
- New `packaging/package.py` does the building and packing, on GitHub or on a desktop;
  `packaging/product.json` names the program.
- `scripts/export_public.py` can now refresh an existing clone of the public repository,
  as well as fill an empty folder, and it exports the workflow and `packaging/` too.
  `scripts/audit_standalone.py` checks the copyright lines in `packaging/` as well.

## 1.0.0 (2026-10-10)

The first public release: the free Community edition, copyright whispr.dev, under
GPL-3.0-only.

- Copyright notices. Every code file opens with `Copyright (C) 2026 whispr.dev` and
  `SPDX-License-Identifier: GPL-3.0-only`, and `scripts/audit_standalone.py` fails any
  that doesn't. `skeletrace --version` keeps its first line and adds the copyright,
  licence and no-warranty lines under it; the help screen gives the copyright line. The
  README states the copyright, and that the GPL covers the program but not the SkeleTrace
  name or logo. Both manifests name whispr.dev as the author, and the engine's now says
  `license = "GPL-3.0-only"`. CONTRIBUTING asks for terms to be agreed with whispr.dev
  before code is sent, and the logo SVG in `assets/` carries the copyright line too.
- Line-1 columns now match what an editor shows when a file starts with a UTF-8
  byte-order mark, as files saved by Windows PowerShell 5.1 do. The mark used to count
  as one character. This applies to hits and near-misses alike; byte offsets still
  count every byte, the mark's three included.
- The help screen (`skeletrace` on its own, or `skeletrace --help`) opens with the
  SkeleTrace logo in plain ASCII. Command output is unchanged: pure JSON, no logo.
- New `scripts/package_release.py` builds the release program and packs it, with the
  README, licence and changelog, into `dist/skeletrace-<version>-<platform>.zip`, with a
  SHA-256 checksum file beside it. The same program file always gives the same zip,
  byte for byte.
- New `scripts/export_public.py` copies the files that belong in the public repository
  into a new folder. It works from an allowlist, so old archives, build history,
  internal notes and 0.12 leftovers stay out.
- README: the logo sits in a code block so GitHub draws it as intended, a "Get it"
  section points to skeletrace.com/download/, the dead link to `docs/CLEANUP_REPORT.md`
  is gone, and the licence section states GPL-3.0. CONTRIBUTING names the pinned Rust
  1.97.0, and SECURITY names this release and a private contact address. Stray first
  lines left over from pasting were removed from five Markdown files.

Validation on Linux x86_64 with Rust 1.97: 155 tests (4 new; the two brute-force sweep
checks now start one text in five with a byte-order mark), one documentation example,
clippy with warnings denied, format checks, the release build, the offline workflow,
the smoke test (16 commands) and the isolation audit pass. The public export built and
passed the same tests on its own. `skeletrace-algo` is unchanged at 0.4.0.

## 0.17.0 (2026-10-10)

Near-misses in text sweeps: the last of the four tool families.

- `sweep-text` takes `--fuzzy 1` or `--fuzzy 2`. The same pass then also lists words a
  typo or two away from a term, such as `jdeo88` for `jdoe88`, in a new `near_hits` list.
  A typo is an insertion, a deletion, a substitution, or a swap of two neighbouring
  characters. Terms under 5 characters get none, 5 to 7 characters get one, and 8 or more
  get two (one with `--fuzzy 1`).
- Words are runs of letters and digits in any alphabet plus `@ . _ - + #`, with full stops
  trimmed from both ends. A word holding an `@` is also tried piece by piece, so a handle
  inside an email address is found. A word cut by a chunk boundary is kept whole. A run of
  word characters longer than 4,096 bytes, or than the longest term plus 64 if that is
  more, is skipped rather than held in memory.
- Terms that aren't one word, such as spaced phone numbers, are only matched exactly. The
  report lists them in `near_skipped` with the reason, along with terms too short for
  typos.
- New report fields: `fuzzy`, `near_hits_total`, `near_hits` (each with `edits`),
  `near_truncated`, `near_only` and `near_skipped`; per term, `near_hits` and
  `edits_allowed`; per file, `near_hits`. Existing fields keep their meaning, and a word
  holding a term exactly is left to the exact results. `--max-hits` caps each list
  separately. Requests and reports saved by earlier versions still load.
- Library: `SweepRequest::fuzzy`, `Sweeper::with_fuzzy`, `SweepNearHit` and
  `SweepNearSkip`, plus `edits_allowed`, `is_word_char` and the limits in the `sweep`
  module.
- Uses `skeletrace-algo` 0.4.0, which adds `fuzzy::NearSet`. It files every term under its
  deletion variants (symmetric deletion, with a prefix limit), so each word costs a few
  hash lookups. The crate's existing functions are unchanged, and all 2,742 answers behind
  the website's algorithm lab came out byte-identical, so the lab needs no rebuild.
- The smoke test checks `sweep-text --fuzzy` on the demo.

Speed on a 300 MB synthetic dump with a 1,000-term watch list, on one core of the 2-core
cloud sandbox: 58 MB/s exact, 24 MB/s with `--fuzzy 1` and 18 MB/s with `--fuzzy 2`, in
under 15 MB of memory.

Validation on Linux x86_64 with Rust 1.97: 151 tests (13 new; one checks near-misses
against an independent word splitter and distance table on 600 random texts, at chunk
sizes from 1 byte to 1 MiB), one documentation example, clippy with warnings denied,
format checks, the release build, the offline workflow, the smoke test (16 commands) and
the isolation audit pass. In `skeletrace-algo`: 92 tests, clippy and format checks pass.

## 0.16.0 (2026-10-10)

Geofences, and the area a set of nodes covers.

- New command `fence-check PROFILE VIEW FENCES`. Fences are GeoJSON (`Polygon`,
  `MultiPolygon`, `Feature` or `FeatureCollection`), one fence per feature, named by its
  `name` property. For each fence it lists the view's nodes inside and exactly on the
  edge, and gives the fence's area in km² with holes taken out. Nodes in no fence and
  nodes without a position are listed separately. Rings spanning more than 180° of
  longitude are refused.
- New command `cover-area PROFILE VIEW`: the convex hull round every positioned node,
  anticlockwise as GeoJSON expects, its area in km², and the outline as a GeoJSON
  `Feature`. It warns when the nodes span more than 180° of longitude.
- Library: the `geofence` module (`fence_check`, `cover_area`, `fences_from_geojson`,
  `GeoFence`, `FencePolygon`, `FenceCheck`, `FenceResult`, `CoverArea`, `GeoNode`,
  `FenceError`), `OperatorRequest::{FenceCheck, CoverArea}` and the matching responses.
  Placement and areas come from `skeletrace-algo`.
- The smoke test checks both commands on the demo.
- The toolchain pin in `rust-toolchain.toml` moves from Rust 1.87.0 to 1.97.0, the
  version these checks run on, so clippy gives the same verdict everywhere. Clippy 1.87
  had flagged one range check in `geofence.rs`, now written more simply. The declared
  minimum stays at 1.87 (`rust-version` in `Cargo.toml`).

Validation on Linux x86_64 with Rust 1.97: 138 tests (8 new contract tests, one checking
fences against an independent winding-number method on 200 random outlines), one
documentation example, clippy with warnings denied, format checks, the release build,
the offline workflow, the smoke test (15 commands) and the isolation audit pass. 0.15.0
passed its checks on the pinned Rust 1.87.0 on Windows.

## 0.15.0 (2026-10-10)

The strongest chain: the most believable route between two nodes.

- New command `strongest-chain PROFILE VIEW FROM TO`. On a topology view, it finds the
  chain of links between two nodes whose confidences, multiplied together, are highest.
  It reports the nodes and links in order, each link's confidence, and the chain's
  overall confidence. Nodes are named by ID or exact label; a shared label is refused
  with the matching IDs. Links count both ways, as in `analyze-topology`. Links at
  confidence 0 are never used and are listed, as are links that leave the view. No
  route is a normal answer (`found: false`), and ties are settled by link ID, so the
  output is stable.
- Library: the `chain` module (`strongest_chain`, `find_node`, `StrongestChain`,
  `ChainNode`, `ChainLink`, `ChainError`), `OperatorRequest::StrongestChain` and
  `OperatorResponse::StrongestChain`. It runs Dijkstra's method from `skeletrace-algo`.
- The smoke test now checks `strongest-chain` on the demo's three-node line.

Validation on Linux x86_64 with Rust 1.97: 130 tests (6 new contract tests, one of them
checking the chain against every possible route on 400 random graphs), one
documentation example, clippy with warnings denied, format checks, the release build,
the offline workflow, the smoke test (13 commands) and the isolation audit pass. The
pinned Rust 1.87.0 couldn't be downloaded here; clippy's minimum-version check found
nothing newer than 1.87.

## 0.14.0 (2026-10-10)

Text sweeps, and the engine's first use of `skeletrace-algo`, SkeleTrace's own
standard-library-only algorithm crate.

- New command `sweep-text [--exact] [--max-hits N] TERMS FILE...`. It finds every term on a
  watch list (one per line) in any number of files, one pass per file, reading 1 MiB at a
  time. The JSON report gives per-term counts, the terms never found, and each hit's file,
  byte offset, line, column (in characters) and text as written. ASCII capitals are ignored
  unless `--exact`. Watch lists may be UTF-8 or UTF-16, as Windows PowerShell writes them.
  Inputs that start with a UTF-16 byte-order mark are flagged.
- Library: the `sweep` module (`Sweeper`, `SweepRequest`, `SweepReport`, `sweep_files`,
  `parse_terms`), `OperatorRequest::SweepText` and `OperatorResponse::Sweep`.
- `skeletrace-algo` 0.3.0 is a path dependency in this repository. The isolation audit
  allows it by name and still rejects any other path dependency. The smoke test now
  checks `sweep-text` too.
- Fixes: `Cargo.lock` listed `skeletrace-algo` 0.1.0, so `--locked` builds failed.
  `examples/case_bottlenecks.rs`, a 0.12 example for the separated notebook tool, no longer
  compiled and is removed. A redundant `.into_iter()` in `src/engine.rs` failed newer
  clippy. Build output under `skeletrace-algo/target` is no longer tracked, and `target/`
  is ignored at any depth.

Validation on Linux x86_64 with Rust 1.97: 124 tests (8 new contract tests, 2 new unit
tests), one documentation example, clippy with warnings denied, format checks, the release
build, the offline workflow, the smoke test (12 commands) and the isolation audit pass.
The pinned Rust 1.87.0 couldn't be downloaded there; clippy's minimum-version check found
nothing newer than 1.87.

## 0.13.0 (2026-10-05)

Standalone SkeleTrace separation release. The embedded detector/case APIs and
commands, application workspace crates, optional features, orphan packet/source
modules, integration fixtures and generated outputs have been removed. No
CrowdWatch integration remained in the supplied code.

The generic engine and independent topology analysis remain. Core hot paths avoid
unnecessary ring-buffer/latest-value/edge-store allocations and repeated selection
scans. Snapshot stems are confined to portable filenames. CLI error statuses and
stdout failures now match their documented contract.

Rust 1.87.0 and surviving dependency versions are pinned. Added a full synthetic
offline workflow, standalone CLI/file-boundary tests, isolation/smoke scripts and
Windows/Linux validation commands. The supplied GPLv3 license text is preserved;
metadata references it rather than the contradictory previous MIT claim.

Validation: 114 tests, one documentation example, clippy with warnings denied,
format checks, an optimized release build, the offline workflow and all 11 engine
commands pass on Linux x86_64. Native Windows execution was not available.

See `docs/CLEANUP_REPORT.md` for the removal ledger, compatibility notes, unchanged
runtime limits and license discrepancy. The entries below describe earlier versions
and do not imply that their embedded tools or old source archives are shipped here.

## 0.12.0 (2026-09-30)

One workspace, three instruments: the engine, Maxwell's Demon Detector and the OSINTropy case
notebook.

### Added

- **Landscape** (`src/landscape.rs`, feature `landscape`, on by default): Maxwell's Demon Detector
  v0.3.0 as a skeletrace view. `skeletrace landscape` is the full interactive detector (every `mdd`
  flag works). `landscape-report` runs any offline input (demo, replay, bytes, pcap-file) headless
  on a virtual clock and returns per-lane statistics, alerts and the final ASCII frame.
  `analyze_packet_events` paces in-memory events exactly like mdd's replay source.
- **Case notebook** (`src/osintropy/`, `skeletrace case ...`): a Rust port of Python OSINTropy
  4.1.1. It has all 17 connectors and the SQLite case store (same schema, default path and stored
  text as the Python tool). It also brings the analysis, JSON/CSV/HTML/GraphML exports, JSON/CSV
  and legacy imports, and the full command set. `scripts/crosscheck_osintropy.py` proves the two
  tools read each other's case files and produce byte-identical exports (19 checks).
- **Bottlenecks** (`src/topology_analysis.rs`): chokepoints (articulation points) and bridges for
  any topology view, iterative and linear-time. Commands: `analyze-topology PROFILE VIEW`,
  `case graph RUN`, `case bottlenecks RUN`.
- A case loads into the engine as Observer, Identity and Endpoint nodes joined by Reference and
  Association edges (`osintropy::graph::load_case_into_engine`, `OperatorRequest::LoadCase`). IDs
  are UUIDv5, so loading a case twice updates it rather than duplicating it.
- Operator API additions:
  - `OperatorRequest::{Landscape, AnalyzeTopology, LoadCase}`
  - `OperatorResponse::{Landscape, TopologyAnalysis, Case, CaseLoaded}`
  - `OperatorError::{Landscape, Case}`
- Binary: `help`, `--version`, exit codes (2 usage, 3 case search saved with a failed source), and
  no error on a closed pipe.
- Tests: `landscape_contracts` (7), `osintropy_contracts` (25), `topology_analysis_contracts` (7),
  plus unit tests for the Python-compatible JSON writer. 189 tests pass across the workspace.
- Examples: landscape and verify-packets request files, case import samples (JSON and CSV),
  `examples/case_bottlenecks.rs`.
- Docs: a new README, `docs/ENGINES_ASSESSMENT.md`, this changelog.

### Changed

- The repository is a Cargo workspace: the root `skeletrace` crate plus
  `crates/maxwells-demon-detector` (v0.3.0, unchanged). Its release profile moved to the workspace
  root, the only place Cargo reads profiles.
- `verify-packets` is rebuilt on mdd. Its request is now `{"input_path": ..., "options": ...}` and
  it returns a `Landscape` report; the old `PacketVerification` report is gone. It accepts JSON
  arrays, JSON objects, JSONL and CSV event files and counts malformed rows.
- New dependencies: `csv` 1.4, `sha2` 0.10, `url` 2, `idna` 1, and `clap` 4.6 (optional, with
  `landscape`).

### Removed

- The pre-0.3 Maxwell code that had been woven through the engine:
  - modules `src/packet/` and `src/packet_workflow.rs`;
  - the unwired `src/{source/,render.rs,stats.rs,event.rs,model.rs,verify.rs,cli.rs}`;
  - their tests `integration_embedding.rs` and `packet_workflow_contracts.rs`.

  All of it is kept in `previous-versions/stage14-st12fix/` and `previous-versions/maxwell-pre-0.3/`.
- The duplicate trees `skeletrace3/`, `skeletrace4/` and `skeletrace-sourcecode/`; the same code is
  in the archived stage-14 tree.
- The four "monitoring empire" engines are archived, not wired in. See
  `docs/ENGINES_ASSESSMENT.md`.

### Differences from Python OSINTropy 4.1.1

- openalex, orcid, openlibrary, gleif, companies_house_companies, opencorporates and rdap store
  `kind = "record"`. 4.1.1 left them at the model default, `import`, which contradicts its own
  source table. The Python package in `osintropy-py/` is left exactly as it was.
- ORCID answering `null` for "no matches" is an empty result, not a source error.

### Known

- The stage-14 engine code keeps 21 pre-existing clippy style warnings: 6 missing `Default` impls,
  4 functions with eight arguments, 4 `map_or` simplifications, 2 complex types, 2 field
  assignments after `Default::default()`, and one each of a manual char comparison, a `?` rewrite
  and a collapsible `if`. The new modules are clippy-clean.
- The release profile (fat LTO, one codegen unit) was not built in the sandbox that produced this
  release. The debug build, every test and the cross-check were.
