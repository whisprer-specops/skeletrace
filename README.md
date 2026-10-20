```text
            .:---::.
        -*%@@@@@@@@@@#+:
     :*@@@%+-:....:=*@@@%=
    *@@@=.  -+*##*=:  :*@@%-
  .%@@+  .*@@@@@@@@@@=  .#@@+
  %@@=  :@@%#%@@@@%%@@#   #@@=
 -@@*   %%:   :%*.   =@=   @@@
 *@@-   @=     -.     %+   #@@-
 *@@=   -@=   ....  .*%.   #@@-
 -@@#    -# +@-  +@::#.   .@@@
  *@@+      %#-===%+     .%@@-
   *@@*.    +  +- :=    -%@@-
    =%@@*:     .     .=#@@@%-
      =%@@@#+=---=+*%@@@%@@@@#-
        .=*%@@@@@@@@#+-. -#@@@@%:
              ....         -#@%+ :*-
                             :.=+:==
                              .%%-
```

# SkeleTrace

Sparse spatiotemporal flow-graph engine for OSINT telemetry. Version **1.0.2**, the Community edition.
Copyright © 2026 whispr.dev, free software under GPL-3.0-only.

SkeleTrace stores identity-bearing nodes and relations, metric definitions and
samples, and a bounded hot cache. It creates topology, GeoJSON, data cards and
snapshots when requested. It includes scheduling, source policy and provenance,
query/watchlist evaluation, replay workloads and Lookyloo source adapters. The
strongest chain finds the most believable route between two nodes. Geofences say
which nodes fall inside an area, and how much ground a set of nodes covers. Text
sweeps find every term on a watch list in any number of files, one pass per file.

This is the standalone SkeleTrace source release. Maxwell's Demon Detector,
OSINTropy and CrowdWatch are separate applications; they are not build dependencies
or commands in this crate. The independent chokepoint/bridge analysis remains.

**New here?** Start with the [quickstart guide](docs/quickstart/QUICKSTART.md): a plain-English tour with pictures, from installing to your first results. It's also a [PDF](docs/quickstart/skeletrace-quickstart.pdf) to keep or print.

## Get it

Ready-built programs for Windows, Linux (x86_64 and ARM64) and macOS (Apple silicon
and Intel) are on this repository's Releases page and on
[skeletrace.com/download](https://skeletrace.com/download/), with SHA-256 checksums.
Each system has an installer that shows the licence, asks you to accept it, then asks
which folder to install into. It changes nothing else: no registry entries, no PATH
changes and no uninstaller, so removing SkeleTrace means deleting its folder. Plain
`.zip` and `.tar.gz` archives hold the same files.

- **Windows:** run the `-setup.exe`. It isn't code-signed yet, so Windows may say
  "Windows protected your PC": click **More info**, then **Run anyway**.
- **macOS and Linux:** open a terminal and run the `.run` file with `sh`, for example
  `sh ~/Downloads/skeletrace-macos-universal.run`.

Every download comes twice on each release: with the version in its name, and without
it (such as `skeletrace-linux-x86_64.run`). The two are identical. Links of the form
`https://github.com/whisprer-specops/skeletrace/releases/latest/download/skeletrace-linux-x86_64.run`
always give the newest release.

To build it yourself, follow the steps for your system below. `skeletrace --help`
lists every command.

## Build on Windows 11

Install [Rust through rustup](https://rust-lang.org/tools/install/) and the Visual
Studio 2022 Build Tools with **Desktop development with C++** and a Windows SDK.
Use the MSVC Rust host. This source tree pins Rust **1.97.0** in
`rust-toolchain.toml`, and exact direct dependency versions. The code still keeps to
Rust 1.87, the declared minimum (`rust-version` in `Cargo.toml`). `Cargo.lock` pins
the dependency graph. The first build downloads the pinned toolchain and crates.

Extract the archive into a new folder and run PowerShell there:

```powershell
cd .\skeletrace-standalone
cargo build --locked --release
.\target\release\skeletrace.exe --version
.\target\release\skeletrace.exe --help
cargo run --locked --release --example standalone_demo -- .\output\demo
Get-Content .\output\demo\report.json
```

The demonstration requires no credentials and makes no network requests. It reads
three synthetic NDJSON observations, queries their metrics, materializes topology
and geography, finds one chokepoint and two bridges, evaluates a watchlist, and
writes a GeoJSON snapshot with a local SQLite catalog. It also writes complete JSON
profiles and request files that match this release's actual types.

SQLite is compiled from the bundled dependency. No Python, packet-capture SDK,
terminal UI library or separate SQLite installation is required to build/run SkeleTrace.

## Build on Linux

Install Rust through rustup and a C compiler/linker (`build-essential` on Ubuntu).

```bash
cd skeletrace-standalone
cargo build --locked --release
./target/release/skeletrace --help
cargo run --locked --release --example standalone_demo -- ./output/demo
cat ./output/demo/report.json
```

## Commands

Profile and request files are JSON. Output is JSON on stdout; diagnostics go to
stderr. Exit statuses are **0** for success, **1** for runtime errors and **2** for
missing, unknown or malformed commands. A closed downstream pipe is successful.

| Command | Arguments | Result |
|---|---|---|
| `profile-validate` | `PROFILE` | Validate the profile and summarize its nodes |
| `tick` | `PROFILE` | Poll sources due on the engine's current tick |
| `poll-source` | `PROFILE SOURCE_UUID` | Poll one source explicitly |
| `materialize-topology` | `PROFILE VIEW` | Selected graph nodes, edges and boundaries |
| `materialize-sparse-geo` | `PROFILE VIEW` | GeoJSON for the requested viewport |
| `analyze-topology` | `PROFILE VIEW` | Components, chokepoints and bridges |
| `strongest-chain` | `PROFILE VIEW FROM TO` | The most believable chain of links between two nodes |
| `fence-check` | `PROFILE VIEW FENCES` | Which nodes are inside, on the edge of, or outside each GeoJSON polygon |
| `cover-area` | `PROFILE VIEW` | The outline round every positioned node, its area in km² and GeoJSON |
| `export-snapshot` | `PROFILE JOB OUTPUT_DIR [CATALOG_DB]` | Native JSON, CSV or GeoJSON snapshot |
| `query-latest` | `PROFILE FILTER` | Current metric values matching a filter |
| `query-advanced` | `PROFILE SAVED_QUERY` | Sorted/grouped/comparison query |
| `evaluate-watchlist` | `PROFILE WATCHLIST` | Evaluate rules against current values |
| `health` | `PROFILE` | Engine, source, cache and event health |
| `sweep-text` | `[--exact] [--fuzzy 1\|2] [--max-hits N] TERMS FILE...` | Every watch-list term in the files, with line and column; with `--fuzzy`, near-misses too |
| `help`, `--help`, `-h` | none | Usage |
| `version`, `--version`, `-V` | none | Version |

After running the demonstration:

```powershell
$st = ".\target\release\skeletrace.exe"
& $st profile-validate .\output\demo\profile.json
& $st analyze-topology .\output\demo\profile.json .\output\demo\topology-view.json
& $st materialize-sparse-geo .\output\demo\profile.json .\output\demo\geo-view.json
```

## The strongest chain between two nodes

Every link carries a confidence from 0 to 1. A chain of links is only as believable
as all of them together, so its confidence is their confidences multiplied.
`strongest-chain` finds the chain between two nodes with the highest product: two
solid links at 0.9 each (0.81 together) beat one shaky link at 0.5.

```powershell
& $st strongest-chain .\output\demo\profile.json .\output\demo\topology-view.json "Synthetic station A" "Synthetic station C"
```

Name each node by its ID or its exact label. If several nodes share a label, the
command lists their IDs so you can pick one. Links count both ways, as in
`analyze-topology`. A link at confidence 0 is never used, and the report lists it
under `unbelieved_links`. Links that leave the view are listed under
`dangling_links`. When nothing joins the two nodes, `found` is false; that is an
answer, not an error. Equally strong chains are settled by link ID, so the same view
always gives the same answer.

## Geofences and covered area

`fence-check` takes fences as GeoJSON: a `Polygon` or `MultiPolygon`, a `Feature`
holding one, or a `FeatureCollection` of them, as geojson.io, QGIS and most mapping
tools save. Each feature is one fence, named by its `name` property. For every fence
it lists the view's nodes inside and exactly on the edge, and gives the fence's area
in km², with holes taken out. A node in a hole is outside. Nodes in no fence, and
nodes with no position, are listed separately.

```powershell
& $st fence-check .\output\demo\profile.json .\output\demo\topology-view.json .\fences.geojson
& $st cover-area .\output\demo\profile.json .\output\demo\topology-view.json
```

`cover-area` draws the tightest outline round every node that has a position (the
convex hull), gives its area in km², and returns it as a GeoJSON `Feature` you can
load straight onto a map. Nodes all in one line enclose no area.

Edges count as straight lines on a latitude/longitude map, which suits areas up to
a few hundred kilometres across. Fences that cross the ±180° meridian aren't
supported, so a ring spanning more than 180° of longitude is refused; split it in
two. Areas are on a sphere of the Earth's mean radius.

## Sweeping text for a watch list

`sweep-text` needs no profile. Give it a watch list, one term per line, and the files
to search: dumps, pastes, saved pages, logs. Each file is read once, a megabyte at a
time, so file size doesn't matter and the number of terms barely does. Capital
letters (A to Z) are ignored unless you add `--exact`.

```powershell
$st = ".\target\release\skeletrace.exe"
"lantern@example.org", "@moth_relay", "+44 7700 900123" | Set-Content -Encoding utf8 .\watch.txt
$report = (& $st sweep-text .\watch.txt .\dump-1.txt .\dump-2.txt | Out-String | ConvertFrom-Json).Sweep
$report.counts | Format-Table term, hits, files
$report.hits | Select-Object -First 20 | Format-Table term, input, line, column, matched
$report.missing
```

The watch list can be UTF-8, with or without a byte-order mark, or the UTF-16 that
PowerShell's `>` writes. Spaces around a term are trimmed. Blank lines are skipped, and
so are notes: lines that are just `#`, or start with `#` and a space. A hashtag such
as `#lantern` is a term. Terms can't span lines.

The report lists each term's hit count and how many files held it, the terms found
nowhere, and the first 10,000 hits (`--max-hits` changes that). Each hit gives the file,
the byte offset, the line, the column (in characters, as editors count them) and the
text as written. Counts are always complete, and `truncated` says whether some hits
went unlisted. Files are searched as bytes, so UTF-8 and ASCII text work as they are. A
file that starts with a UTF-16 byte-order mark gets a `warning`, because UTF-8 terms
can't match it; save it as UTF-8 first.

### Near-misses with `--fuzzy`

Add `--fuzzy 1` or `--fuzzy 2` and the same pass also lists words a typo or two away
from a term: `jdeo88` for `jdoe88`, or `lantern@exmaple.org` for `lantern@example.org`. A
typo is a character added, dropped or changed, or two neighbouring characters swapped.
Each near-miss gives the file, byte offset, line, column, the word as written, and its
`edits`: how many typos away it is.

```powershell
$report = (& $st sweep-text --fuzzy 2 .\watch.txt .\dump-1.txt | Out-String | ConvertFrom-Json).Sweep
$report.near_hits | Select-Object -First 20 | Format-Table term, matched, edits, line, column
$report.counts | Format-Table term, hits, near_hits, edits_allowed
$report.near_only
```

How many typos a term may have depends on its length. Short terms sit one typo away from
too many ordinary words: "moth" is one from "math", "both" and "mote".

| Term length | Typos allowed |
|---|---|
| Under 5 characters | None: exact matches only |
| 5 to 7 characters | 1 |
| 8 characters or more | 2, or 1 with `--fuzzy 1` |

For this, the text is split into words: runs of letters and digits in any alphabet, plus
`@ . _ - + #`. Full stops are trimmed from both ends, so a word that ends a sentence still
counts. A word with an `@` in it is also tried piece by piece, so a handle inside an email
address is found. Terms that aren't one word, such as `+44 7700 900123`, are only matched
exactly. `near_skipped` lists them with the reason, along with the terms too short for
typos.

Near-misses come back in their own list, `near_hits`, so the exact results read as they
always did. A word that holds a term exactly is left to the exact results, so nothing is
listed twice. `near_only` names the terms found only as near-misses. They stay in
`missing` too, which means "not found as written". `--max-hits` caps each list
separately, and every count stays complete.

An `@` or `#` at the start of a term counts as a character. So `@jdoe88` is one typo
from a bare `jdoe88`, as in `user=jdoe88`. List a handle without its `@` if you want bare
mentions to be exact hits.

Speed, measured on a 300 MB synthetic dump with a 1,000-term watch list, on one core of a
2-core cloud machine: 58 MB/s exact, 24 MB/s with `--fuzzy 1` and 18 MB/s with
`--fuzzy 2`. Memory stayed under 15 MB. Watch lists of long identifiers run faster,
because words too short or too long to be near any term are dropped at once.

Paths inside JSON resolve against your current working directory. The demonstration
writes absolute paths, so its files can be invoked from another directory on that
same machine. Regenerate them if the output directory moves. The two Lookyloo
source-profile examples are adapter configuration fragments, not full engine
profiles; supply your own Lookyloo endpoint and registered metric definitions.

## Runtime boundary

Each CLI invocation creates a fresh engine. In-memory latest values, cache state,
adapter read offsets and source scheduling are not a running daemon shared by
successive CLI commands. SQLite stores retained samples; opening it does not
reconstruct all current engine state. A new `query-latest` process therefore does
not automatically see a previous `poll-source` process's latest-value map.

For an ingest/query/view/export session, keep one `SkeletraceEngine` or `OperatorApi`
alive, as the complete `examples/standalone_demo.rs` demonstrates. This separation
release preserves that original runtime model rather than inventing an automatic
collector or changing acquisition behavior. Topology analysis does not require
samples and can be used directly from the CLI.

## Library and external applications

Import `skeletrace` and use `SourceAdapter`/`SourcePull`, `FileSampleRecord` with the
NDJSON adapter, or explicit `OperatorRequest` values. These are application-neutral
interfaces. The strongest chain is `skeletrace::strongest_chain`, and
`OperatorRequest::StrongestChain` runs it on a live engine's links. Geofences are
`skeletrace::fence_check`, `cover_area` and `fences_from_geojson`, with
`OperatorRequest::FenceCheck` and `OperatorRequest::CoverArea`. Text sweeps are
`skeletrace::Sweeper`, which also reads any `std::io::Read` source, `sweep_files`, and
`OperatorRequest::SweepText`; near-misses are `Sweeper::with_fuzzy` or
`SweepRequest::fuzzy`. The search machine and the near-miss index
(`skeletrace_algo::fuzzy::NearSet`) come from `skeletrace-algo`, SkeleTrace's own
standard-library-only algorithm crate in this repository. A future integration should
translate an external application's observations at that boundary and remain
separately distributed. No replacement plugin framework or hidden dependency is
included here.

The public `landscape`, `osintropy`, `Case*`, `LoadCase` and packet-verification
interfaces were removed, along with the `landscape`/`live` Cargo features. This is a
breaking separation change from 0.12.0. Ordinary engine profile and request formats
remain compatible; embedded-tool requests should go to the separate applications.

## Validation

```powershell
cargo fmt --all --check
cargo test --locked --all-targets
cargo test --locked --doc
cargo clippy --locked --all-targets -- -D warnings
```

`scripts/check.ps1` and `scripts/check.sh` run those checks, build the release binary,
and execute the offline example. `scripts/audit_standalone.py` checks source/module
and dependency isolation, and that every code file opens with its copyright lines.
`scripts/smoke_test.py` exercises every engine command against the generated demo
files. Both Python scripts require Python 3.11+ and only the standard library. The
core Cargo checks need no Python.

Releases are built by `.github/workflows/release.yml`, which runs the checks, builds and
packs on Windows, Linux and macOS, and publishes when a version tag such as `v1.0.2` is
pushed. `packaging/package.py` does its building and packing and also runs on a desktop.
`scripts/package_release.py` packs a plain zip for the machine it runs on. Each
release's changes, and what was tested, are in `CHANGELOG.md`.

## License

Copyright © 2026 whispr.dev.

SkeleTrace Community is free software: you can redistribute it and change it under the
terms of the GNU General Public License, version 3 only (GPL-3.0-only). It comes with
NO WARRANTY, to the extent permitted by law. The full terms are in `LICENSE`. Every code
file opens with its copyright line and `SPDX-License-Identifier: GPL-3.0-only`.

The GPL covers the program. It gives no right to use the SkeleTrace name or logo, which
stay with whispr.dev, as section 7 of the licence allows.
