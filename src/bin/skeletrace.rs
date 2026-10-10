// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! `skeletrace` command-line entry point.

use std::io::{self, Write};
use std::process::ExitCode;

use skeletrace::{run_cli_command, CliCommand};

/// The SkeleTrace mark, a skull seen through a magnifying glass, in plain ASCII so every terminal shows it. It opens
/// the help screen only: command output stays pure JSON.
const LOGO: &str = r"
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
";

/// Printed under the version line, in the form GNU programs use.
const NOTICE: &str = "\
Copyright (C) 2026 whispr.dev
License GPL-3.0-only: GNU General Public License, version 3 <https://www.gnu.org/licenses/gpl-3.0.html>
This is free software: you are free to change and redistribute it.
There is NO WARRANTY, to the extent permitted by law.";

const USAGE: &str = "\
skeletrace - sparse spatiotemporal OSINT flow-graph engine
Copyright (C) 2026 whispr.dev. Free software under GPL-3.0-only, with NO WARRANTY.

USAGE:
  skeletrace <command> [arguments]

ENGINE (profile and request files are JSON):
  profile-validate <profile.json>
  tick <profile.json>
  poll-source <profile.json> <source-uuid>
  materialize-topology <profile.json> <view.json>
  materialize-sparse-geo <profile.json> <view.json>
  analyze-topology <profile.json> <view.json>     chokepoints and bridges
  strongest-chain <profile.json> <view.json> <from> <to>
      the most believable chain of links between two nodes, each named by its
      ID or exact label; a chain's confidence is its links' confidences multiplied
  fence-check <profile.json> <view.json> <fences.geojson>
      which nodes are inside, on the edge of, or outside each GeoJSON polygon
  cover-area <profile.json> <view.json>
      the outline round every positioned node, its area in km2, and GeoJSON
  export-snapshot <profile.json> <job.json> <output-dir> [catalog.db]
  query-latest <profile.json> <filter.json>
  query-advanced <profile.json> <saved-query.json>
  evaluate-watchlist <profile.json> <watchlist.json>
  health <profile.json>

TEXT (no profile needed):
  sweep-text [--exact] [--fuzzy 1|2] [--max-hits N] <terms.txt> <file>...
      every watch-list term in the files, one pass per file. terms.txt holds one
      term per line; lines starting \"# \" are notes. Capital letters are ignored
      unless --exact. --fuzzy also lists words a typo or two away, in near_hits:
      one typo for terms of 5 to 7 characters, up to 2 from 8; shorter terms, and
      terms that are not one word, are only matched exactly. Lists the first N
      hits and the first N near-misses (default 10000); counts are complete.

  help | --help | -h
  version | --version | -V

Output is JSON on stdout. Exit codes: 0 success, 1 runtime error, 2 usage error.
Paths in profiles and requests resolve from the current working directory.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        None => {
            eprintln!("{LOGO}\n{USAGE}");
            return ExitCode::from(2);
        }
        Some("help" | "--help" | "-h") if args.len() == 2 => {
            return print_text(&format!("{LOGO}\n{USAGE}"));
        }
        Some("version" | "--version" | "-V") if args.len() == 2 => {
            return print_text(&format!(
                "skeletrace {}\n{NOTICE}",
                env!("CARGO_PKG_VERSION")
            ));
        }
        _ => {}
    }

    let command = match CliCommand::parse_from(args) {
        Ok(command) => command,
        Err(err) => {
            eprintln!("skeletrace: {err}\nRun `skeletrace --help` for usage.");
            return ExitCode::from(2);
        }
    };
    match run_cli_command(command).and_then(|response| {
        serde_json::to_string_pretty(&response)
            .map_err(|err| skeletrace::OperatorError::Serde(err.to_string()))
    }) {
        Ok(response) => print_text(&response),
        Err(err) => {
            eprintln!("skeletrace: {err}");
            ExitCode::FAILURE
        }
    }
}

/// A consumer closing its pipe is normal; other output failures must surface.
fn print_text(text: &str) -> ExitCode {
    let mut out = io::stdout().lock();
    match writeln!(out, "{text}").and_then(|()| out.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("skeletrace: cannot write output: {err}");
            ExitCode::FAILURE
        }
    }
}
