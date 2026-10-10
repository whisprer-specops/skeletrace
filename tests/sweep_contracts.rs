// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Contract tests for text sweeps: the library, the operator API and the `sweep-text` command.

use std::cmp::Reverse;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use skeletrace::{
    parse_terms, sweep_files, CliCommand, EngineConfig, OperatorApi, OperatorRequest,
    OperatorResponse, SkeletraceEngine, SnapshotExporter, SweepHit, SweepRequest, Sweeper,
};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_skeletrace"))
        .args(args)
        .output()
        .expect("the Cargo-built standalone binary must launch")
}

/// A fresh, empty folder for one test.
fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("skeletrace-sweep-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn utf16le(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn the_command_line_is_parsed_strictly() {
    let parse = |args: &[&str]| {
        CliCommand::parse_from(
            ["skeletrace", "sweep-text"]
                .iter()
                .chain(args)
                .map(|s| s.to_string()),
        )
    };
    assert_eq!(
        parse(&["watch.txt", "a.txt", "b.txt"]).unwrap(),
        CliCommand::SweepText {
            terms_path: "watch.txt".into(),
            input_paths: vec!["a.txt".into(), "b.txt".into()],
            exact: false,
            fuzzy: 0,
            max_hits: 10_000,
        }
    );
    assert_eq!(
        parse(&["watch.txt", "--exact", "a.txt", "--max-hits", "5"]).unwrap(),
        CliCommand::SweepText {
            terms_path: "watch.txt".into(),
            input_paths: vec!["a.txt".into()],
            exact: true,
            fuzzy: 0,
            max_hits: 5,
        }
    );
    assert_eq!(
        parse(&["--", "--odd.txt", "--exact"]).unwrap(),
        CliCommand::SweepText {
            terms_path: "--odd.txt".into(),
            input_paths: vec!["--exact".into()],
            exact: false,
            fuzzy: 0,
            max_hits: 10_000,
        }
    );
    for bad in [
        &[][..],
        &["watch.txt"],
        &["watch.txt", "a.txt", "--max-hits"],
        &["watch.txt", "a.txt", "--max-hits", "lots"],
        &["watch.txt", "a.txt", "--max-hits", "-1"],
        &["--loud", "watch.txt", "a.txt"],
    ] {
        assert!(parse(bad).is_err(), "accepted {bad:?}");
        let mut args = vec!["sweep-text"];
        args.extend_from_slice(bad);
        let output = cli(&args);
        assert_eq!(output.status.code(), Some(2), "args: {args:?}");
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
}

#[test]
fn the_binary_sweeps_files_and_reports_json() {
    let dir = scratch();
    // A watch list as Windows PowerShell writes it: UTF-16 with a byte-order mark, and CRLF line ends.
    let watch = dir.join("watch.txt");
    fs::write(
        &watch,
        utf16le("# the Lantern case\r\nlantern@example.org\r\n@moth_relay\r\n+44 7700 900123\r\n@quiet_fox\r\n"),
    )
    .unwrap();
    let first = dir.join("dump-1.txt");
    fs::write(
        &first,
        "user=LANTERN@EXAMPLE.ORG ip=198.51.100.23\nphone=+44 7700 900123\n",
    )
    .unwrap();
    let second = dir.join("dump-2.txt");
    fs::write(&second, "Réponse de @Moth_Relay : lantern@example.org\n").unwrap();
    let output = cli(&[
        "sweep-text",
        watch.to_str().unwrap(),
        first.to_str().unwrap(),
        second.to_str().unwrap(),
    ]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let report = &json["Sweep"];
    assert_eq!(report["terms"], 4);
    assert_eq!(report["hits_total"], 4);
    assert_eq!(report["truncated"], false);
    assert_eq!(report["missing"], serde_json::json!(["@quiet_fox"]));
    assert_eq!(report["counts"][0]["hits"], 2);
    assert_eq!(report["counts"][0]["files"], 2);
    let hits = report["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 4);
    assert_eq!(hits[0]["matched"], "LANTERN@EXAMPLE.ORG");
    assert_eq!(
        (hits[0]["line"].as_u64(), hits[0]["column"].as_u64()),
        (Some(1), Some(6))
    );
    assert_eq!(hits[1]["term"], "+44 7700 900123");
    assert_eq!(
        (hits[1]["line"].as_u64(), hits[1]["column"].as_u64()),
        (Some(2), Some(7))
    );
    // "Réponse" has an é, two bytes long. Offsets count bytes from 0, so the match starts at byte 12, but
    // columns count characters from 1, so it shows as column 12, not 13.
    assert_eq!(hits[2]["matched"], "@Moth_Relay");
    assert_eq!(
        (hits[2]["offset"].as_u64(), hits[2]["column"].as_u64()),
        (Some(12), Some(12))
    );
    assert_eq!(
        report["inputs"][1]["bytes"].as_u64(),
        Some(fs::metadata(&second).unwrap().len())
    );
    assert!(report["inputs"][0]["warning"].is_null());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn missing_files_and_empty_watch_lists_are_runtime_errors() {
    let dir = scratch();
    let watch = dir.join("watch.txt");
    fs::write(&watch, "lantern@example.org\n").unwrap();
    let absent = dir.join("absent.txt");
    let output = cli(&[
        "sweep-text",
        watch.to_str().unwrap(),
        absent.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("absent.txt"));
    let notes_only = dir.join("notes.txt");
    fs::write(&notes_only, "# nothing to look for yet\n\n").unwrap();
    let output = cli(&[
        "sweep-text",
        notes_only.to_str().unwrap(),
        watch.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no terms"));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_operator_api_runs_sweeps_without_engine_state() {
    let dir = scratch();
    let dump = dir.join("dump.txt");
    fs::write(&dump, "relay@example.net relay@example.net\n").unwrap();
    let engine = SkeletraceEngine::new(EngineConfig::default()).unwrap();
    let exporter = SnapshotExporter::new(dir.join("exports"), None::<PathBuf>).unwrap();
    let mut api = OperatorApi::new(engine, exporter);
    let request = SweepRequest::new(strings(&["relay@example.net"]), vec![dump.clone()]);
    match api.execute(OperatorRequest::SweepText { request }).unwrap() {
        OperatorResponse::Sweep(report) => {
            assert_eq!(report.hits_total, 2);
            assert_eq!(report.hits[1].column, 19);
        }
        other => panic!("expected a sweep report, got {other:?}"),
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn listing_stops_at_the_cap_but_counting_does_not() {
    let text = "moth ".repeat(10);
    let mut sweeper = Sweeper::new(&strings(&["moth"]), false, 3).unwrap();
    sweeper.sweep_reader("feed", Cursor::new(text)).unwrap();
    let report = sweeper.finish();
    assert_eq!(report.hits_total, 10);
    assert_eq!(report.counts[0].hits, 10);
    assert_eq!(report.hits.len(), 3);
    assert!(report.truncated);
    let offsets: Vec<u64> = report.hits.iter().map(|h| h.offset).collect();
    assert_eq!(offsets, vec![0, 5, 10]);
}

#[test]
fn a_leading_byte_order_mark_does_not_shift_columns() {
    let terms = strings(&["jdoe88"]);
    let text = "\u{feff}moth jdoe88\njdoe88 moth\n";
    for chunk in [1, 2, 3, 4, 64] {
        let mut sweeper = Sweeper::new(&terms, false, 10)
            .unwrap()
            .with_chunk_bytes(chunk)
            .unwrap();
        sweeper.sweep_reader("bom.txt", Cursor::new(text)).unwrap();
        let report = sweeper.finish();
        let places: Vec<(u64, u64, u64)> = report
            .hits
            .iter()
            .map(|h| (h.offset, h.line, h.column))
            .collect();
        // Offsets still count every byte, the mark's three included; columns match what an editor shows.
        assert_eq!(places, vec![(8, 1, 6), (15, 2, 1)], "chunk {chunk}");
    }
}

#[test]
fn utf16_inputs_are_flagged() {
    let mut sweeper = Sweeper::new(&strings(&["moth"]), false, 10)
        .unwrap()
        .with_chunk_bytes(1)
        .unwrap();
    sweeper
        .sweep_reader("utf16.txt", Cursor::new(utf16le("moth")))
        .unwrap();
    sweeper
        .sweep_reader("utf8.txt", Cursor::new("moth"))
        .unwrap();
    let report = sweeper.finish();
    assert!(report.inputs[0].warning.is_some());
    assert_eq!(report.inputs[0].hits, 0);
    assert!(report.inputs[1].warning.is_none());
    assert_eq!(report.inputs[1].hits, 1);
}

/// A small deterministic generator, so the random cases repeat exactly.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % n
    }
}

/// Every hit by checking every term at every position, with lines and columns counted from the start of the
/// text, in the order the sweep promises. A UTF-8 byte-order mark at the very start is not a column.
fn hits_by_checking_everything(terms: &[String], text: &[u8], exact: bool) -> Vec<SweepHit> {
    // The same de-duplication as the sweeper: first spelling wins.
    let mut kept: Vec<&String> = Vec::new();
    for term in terms {
        let key = |t: &String| {
            if exact {
                t.clone()
            } else {
                t.to_ascii_lowercase()
            }
        };
        if !term.is_empty() && !kept.iter().any(|k| key(k) == key(term)) {
            kept.push(term);
        }
    }
    let same = |a: u8, b: u8| {
        if exact {
            a == b
        } else {
            a.eq_ignore_ascii_case(&b)
        }
    };
    let mut found: Vec<(usize, usize, usize)> = Vec::new();
    for (index, term) in kept.iter().enumerate() {
        let t = term.as_bytes();
        for start in 0..text.len().saturating_sub(t.len() - 1) {
            if start + t.len() <= text.len()
                && t.iter().zip(&text[start..]).all(|(&x, &y)| same(x, y))
            {
                found.push((index, start, start + t.len()));
            }
        }
    }
    found.sort_by_key(|&(index, start, end)| (end, Reverse(end - start), index));
    found
        .into_iter()
        .map(|(index, start, end)| {
            let before = &text[..start];
            let line_start = before
                .iter()
                .rposition(|&b| b == b'\n')
                .map_or(0, |p| p + 1);
            let bom = u64::from(line_start == 0 && start >= 3 && text.starts_with(b"\xEF\xBB\xBF"));
            SweepHit {
                term: kept[index].clone(),
                input: "text".into(),
                offset: start as u64,
                line: 1 + before.iter().filter(|&&b| b == b'\n').count() as u64,
                column: 1 + text[line_start..start]
                    .iter()
                    .filter(|&&b| b & 0xC0 != 0x80)
                    .count() as u64
                    - bom,
                matched: String::from_utf8_lossy(&text[start..end]).into_owned(),
            }
        })
        .collect()
}

#[test]
fn sweeps_match_checking_everything_at_every_chunk_size() {
    let mut rng = Lcg(2026);
    let pieces = ["a", "b", "A", "B", "\n", "é", "€", "ab", "\r\n"];
    let letters = ["a", "b", "A", "B", "é"];
    for round in 0..300 {
        let exact = round % 2 == 0;
        // Every fifth text starts with a byte-order mark, as Windows PowerShell 5.1 writes it.
        let mark = if round % 5 == 0 { "\u{feff}" } else { "" };
        let text: String = std::iter::once(mark)
            .chain((0..rng.below(120)).map(|_| pieces[rng.below(pieces.len() as u64) as usize]))
            .collect();
        let terms: Vec<String> = (0..1 + rng.below(5))
            .map(|_| {
                (0..1 + rng.below(4))
                    .map(|_| letters[rng.below(letters.len() as u64) as usize])
                    .collect()
            })
            .collect();
        let expected = hits_by_checking_everything(&terms, text.as_bytes(), exact);
        for chunk in [1, 2, 3, 5, 8, 64, 1 << 20] {
            let mut sweeper = Sweeper::new(&terms, exact, usize::MAX)
                .unwrap()
                .with_chunk_bytes(chunk)
                .unwrap();
            sweeper
                .sweep_reader("text", Cursor::new(text.as_bytes()))
                .unwrap();
            let report = sweeper.finish();
            assert_eq!(
                report.hits, expected,
                "{terms:?} in {text:?}, chunk {chunk}"
            );
            assert_eq!(report.hits_total, expected.len() as u64);
            assert_eq!(report.inputs[0].bytes, text.len() as u64);
        }
    }
}

#[test]
fn whole_requests_run_files_in_order() {
    let dir = scratch();
    let paths: Vec<PathBuf> = ["one.txt", "two.txt"]
        .iter()
        .map(|name| dir.join(name))
        .collect();
    fs::write(&paths[0], "fox\nFOX\n").unwrap();
    fs::write(&paths[1], "no match here\n").unwrap();
    let mut request = SweepRequest::new(strings(&["fox", "badger"]), paths.clone());
    let report = sweep_files(&request).unwrap();
    assert_eq!(report.hits_total, 2);
    assert_eq!(report.missing, vec!["badger"]);
    assert_eq!(report.inputs.len(), 2);
    assert_eq!(report.inputs[1].hits, 0);
    request.exact = true;
    assert_eq!(sweep_files(&request).unwrap().hits_total, 1);
    request.inputs.clear();
    assert!(sweep_files(&request).is_err());
    assert_eq!(
        parse_terms(b"fox\n# note\n\nbadger\n").unwrap(),
        strings(&["fox", "badger"])
    );
    assert!(Path::new(&paths[0]).exists());
    fs::remove_dir_all(&dir).unwrap();
}
