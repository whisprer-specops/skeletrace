// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Contract tests for near-misses in text sweeps: the library, the operator API and `sweep-text --fuzzy`.

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::process::{Command, Output};

use skeletrace::{
    sweep_files, CliCommand, EngineConfig, OperatorApi, OperatorRequest, OperatorResponse,
    SkeletraceEngine, SnapshotExporter, SweepNearHit, SweepNearSkip, SweepReport, SweepRequest,
    Sweeper,
};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_skeletrace"))
        .args(args)
        .output()
        .expect("the Cargo-built standalone binary must launch")
}

/// A fresh, empty folder for one test.
fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("skeletrace-fuzzy-{}", uuid::Uuid::new_v4()));
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

/// The line and column (both from 1) of byte `offset`, counting columns as the sweep does: every byte except a
/// UTF-8 continuation byte, and not a byte-order mark at the very start.
fn line_and_column(text: &[u8], offset: usize) -> (u64, u64) {
    let before = &text[..offset];
    let line_start = before
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |p| p + 1);
    let bom = u64::from(line_start == 0 && offset >= 3 && text.starts_with(b"\xEF\xBB\xBF"));
    (
        1 + before.iter().filter(|&&b| b == b'\n').count() as u64,
        1 + text[line_start..offset]
            .iter()
            .filter(|&&b| b & 0xC0 != 0x80)
            .count() as u64
            - bom,
    )
}

fn near_hit(
    input: &str,
    text: &str,
    term: &str,
    matched_at: usize,
    matched: &str,
    edits: usize,
) -> SweepNearHit {
    let (line, column) = line_and_column(text.as_bytes(), matched_at);
    SweepNearHit {
        term: term.into(),
        input: input.into(),
        offset: matched_at as u64,
        line,
        column,
        matched: matched.into(),
        edits,
    }
}

#[test]
fn the_fuzzy_flag_is_parsed_strictly() {
    let parse = |args: &[&str]| {
        CliCommand::parse_from(
            ["skeletrace", "sweep-text"]
                .iter()
                .chain(args)
                .map(|s| s.to_string()),
        )
    };
    assert_eq!(
        parse(&["--fuzzy", "2", "watch.txt", "a.txt"]).unwrap(),
        CliCommand::SweepText {
            terms_path: "watch.txt".into(),
            input_paths: vec!["a.txt".into()],
            exact: false,
            fuzzy: 2,
            max_hits: 10_000,
        }
    );
    assert_eq!(
        parse(&["watch.txt", "a.txt", "--fuzzy", "1", "--exact", "b.txt"]).unwrap(),
        CliCommand::SweepText {
            terms_path: "watch.txt".into(),
            input_paths: vec!["a.txt".into(), "b.txt".into()],
            exact: true,
            fuzzy: 1,
            max_hits: 10_000,
        }
    );
    // After `--`, everything is a path, so a file can be called --fuzzy.
    assert_eq!(
        parse(&["--", "--fuzzy", "2"]).unwrap(),
        CliCommand::SweepText {
            terms_path: "--fuzzy".into(),
            input_paths: vec!["2".into()],
            exact: false,
            fuzzy: 0,
            max_hits: 10_000,
        }
    );
    for bad in [
        &["watch.txt", "a.txt", "--fuzzy"][..],
        &["--fuzzy", "0", "watch.txt", "a.txt"],
        &["--fuzzy", "3", "watch.txt", "a.txt"],
        &["--fuzzy", "two", "watch.txt", "a.txt"],
        &["--fuzzy", "-1", "watch.txt", "a.txt"],
        &["--fuzzy", "1.5", "watch.txt", "a.txt"],
        &["--fuzzy", "watch.txt", "a.txt"],
        &["--fuzzy", "1", "watch.txt"],
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
fn the_binary_reports_near_misses_as_json() {
    let dir = scratch();
    // A watch list as Windows PowerShell writes it: UTF-16 with a byte-order mark, and CRLF line ends.
    let watch = dir.join("watch.txt");
    fs::write(
        &watch,
        utf16le("# the Lantern file\r\njdoe88\r\nlantern@example.org\r\n@moth_relay\r\n+44 7700 900123\r\nmoth\r\n#nightjar\r\n"),
    )
    .unwrap();
    let one = "Ask jdeo88. Mail jdeo88@example.net today.\nfrom LANTERN@EXMAPLE.ORG and lantern@example.org, cc lnatern@exmaple.org\n";
    let two = "ping @moth_rleay or math, #nigthjar, +44 7700 900132\n";
    let first = dir.join("dump-1.txt");
    let second = dir.join("dump-2.txt");
    fs::write(&first, one).unwrap();
    fs::write(&second, two).unwrap();
    let (first_name, second_name) = (first.to_str().unwrap(), second.to_str().unwrap());

    let run = |fuzzy: &str| -> SweepReport {
        let output = cli(&[
            "sweep-text",
            "--fuzzy",
            fuzzy,
            watch.to_str().unwrap(),
            first_name,
            second_name,
        ]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        serde_json::from_value(json["Sweep"].clone()).unwrap()
    };
    let at = |text: &str, needle: &str| text.find(needle).unwrap();

    let report = run("2");
    assert_eq!(report.fuzzy, 2);
    let first_jdeo = at(one, "jdeo88");
    let second_jdeo = at(one, "jdeo88@");
    assert!(second_jdeo > first_jdeo);
    let want = vec![
        near_hit(first_name, one, "jdoe88", first_jdeo, "jdeo88", 1),
        // The handle inside an email address, found as the piece before the @.
        near_hit(first_name, one, "jdoe88", second_jdeo, "jdeo88", 1),
        near_hit(
            first_name,
            one,
            "lantern@example.org",
            at(one, "LANTERN@"),
            "LANTERN@EXMAPLE.ORG",
            1,
        ),
        near_hit(
            first_name,
            one,
            "lantern@example.org",
            at(one, "lnatern@"),
            "lnatern@exmaple.org",
            2,
        ),
        near_hit(
            second_name,
            two,
            "@moth_relay",
            at(two, "@moth_rleay"),
            "@moth_rleay",
            1,
        ),
        near_hit(
            second_name,
            two,
            "#nightjar",
            at(two, "#nigthjar"),
            "#nigthjar",
            1,
        ),
    ];
    assert_eq!(report.near_hits, want);
    assert_eq!(report.near_hits_total, 6);
    assert!(!report.near_truncated);
    assert_eq!(
        (report.inputs[0].near_hits, report.inputs[1].near_hits),
        (4, 2)
    );
    // Exact results read as they always did: the exact address, and "moth" inside "@moth_rleay".
    assert_eq!(report.hits_total, 2);
    assert_eq!(report.hits[0].matched, "lantern@example.org");
    assert_eq!(report.hits[1].term, "moth");
    assert_eq!(
        report.missing,
        strings(&["jdoe88", "@moth_relay", "+44 7700 900123", "#nightjar"])
    );
    assert_eq!(
        report.near_only,
        strings(&["jdoe88", "@moth_relay", "#nightjar"])
    );
    let allowed: Vec<(String, u64, u64, usize)> = report
        .counts
        .iter()
        .map(|c| (c.term.clone(), c.hits, c.near_hits, c.edits_allowed))
        .collect();
    assert_eq!(
        allowed,
        vec![
            ("jdoe88".to_string(), 0, 2, 1),
            ("lantern@example.org".to_string(), 1, 2, 2),
            ("@moth_relay".to_string(), 0, 1, 2),
            ("+44 7700 900123".to_string(), 0, 0, 0),
            ("moth".to_string(), 1, 0, 0),
            ("#nightjar".to_string(), 0, 1, 2),
        ]
    );
    assert_eq!(report.near_skipped.len(), 2);
    assert_eq!(report.near_skipped[0].term, "+44 7700 900123");
    assert!(report.near_skipped[0].reason.starts_with("not one word"));
    assert_eq!(
        report.near_skipped[1],
        SweepNearSkip {
            term: "moth".into(),
            reason: "shorter than 5 characters".into()
        }
    );

    // At one typo, the double swap in "lnatern@exmaple.org" is too far; the exact results don't move.
    let one_typo = run("1");
    assert_eq!(one_typo.near_hits_total, 5);
    assert!(one_typo.near_hits.iter().all(|h| h.edits == 1));
    assert_eq!(one_typo.counts[1].edits_allowed, 1);
    assert_eq!(one_typo.hits, report.hits);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn exact_results_are_the_same_with_fuzzy_matching_on() {
    let dir = scratch();
    let dump = dir.join("dump.txt");
    fs::write(
        &dump,
        "relay@example.net relay@exmaple.net\nRELAY@EXAMPLE.NET quiet_fox quiet_fx\n",
    )
    .unwrap();
    let terms = strings(&["relay@example.net", "quiet_fox", "fox"]);
    let mut request = SweepRequest::new(terms, vec![dump.clone()]);
    let plain = sweep_files(&request).unwrap();
    request.fuzzy = 2;
    let fuzzy = sweep_files(&request).unwrap();
    assert_eq!(fuzzy.hits, plain.hits);
    assert_eq!(fuzzy.hits_total, plain.hits_total);
    assert_eq!(fuzzy.missing, plain.missing);
    assert_eq!(fuzzy.truncated, plain.truncated);
    for (with, without) in fuzzy.counts.iter().zip(&plain.counts) {
        assert_eq!((with.hits, with.files), (without.hits, without.files));
    }
    // Off, the new fields are present and empty.
    assert_eq!(plain.fuzzy, 0);
    assert_eq!(plain.near_hits_total, 0);
    assert!(
        plain.near_hits.is_empty() && plain.near_skipped.is_empty() && plain.near_only.is_empty()
    );
    assert!(plain
        .counts
        .iter()
        .all(|c| c.edits_allowed == 0 && c.near_hits == 0));
    // On: one near-miss per term, and "fox" is too short to get any.
    let matched: Vec<&str> = fuzzy.near_hits.iter().map(|h| h.matched.as_str()).collect();
    assert_eq!(matched, vec!["relay@exmaple.net", "quiet_fx"]);
    assert_eq!(fuzzy.near_skipped[0].term, "fox");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_operator_api_runs_fuzzy_sweeps_without_engine_state() {
    let dir = scratch();
    let dump = dir.join("dump.txt");
    fs::write(&dump, "seen: relay@exmaple.net, relay@example.net\n").unwrap();
    let engine = SkeletraceEngine::new(EngineConfig::default()).unwrap();
    let exporter = SnapshotExporter::new(dir.join("exports"), None::<PathBuf>).unwrap();
    let mut api = OperatorApi::new(engine, exporter);
    let mut request = SweepRequest::new(strings(&["relay@example.net"]), vec![dump.clone()]);
    request.fuzzy = 1;
    match api.execute(OperatorRequest::SweepText { request }).unwrap() {
        OperatorResponse::Sweep(report) => {
            assert_eq!(report.hits_total, 1);
            assert_eq!(report.near_hits_total, 1);
            assert_eq!(report.near_hits[0].matched, "relay@exmaple.net");
            assert_eq!(report.near_hits[0].column, 7);
            assert_eq!(report.near_hits[0].edits, 1);
        }
        other => panic!("expected a sweep report, got {other:?}"),
    }
    // A setting out of range is refused, not clamped.
    let mut request = SweepRequest::new(strings(&["relay@example.net"]), vec![dump]);
    request.fuzzy = 3;
    assert!(api.execute(OperatorRequest::SweepText { request }).is_err());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn near_misses_after_a_byte_order_mark_start_at_column_one() {
    let text = "\u{feff}jdeo88 here\n";
    for chunk in [1, 2, 3, 4, 64] {
        let mut sweeper = Sweeper::new(&strings(&["jdoe88"]), false, 10)
            .unwrap()
            .with_chunk_bytes(chunk)
            .unwrap()
            .with_fuzzy(1)
            .unwrap();
        sweeper.sweep_reader("bom.txt", Cursor::new(text)).unwrap();
        let report = sweeper.finish();
        let first = &report.near_hits[0];
        assert_eq!(
            (first.offset, first.line, first.column),
            (3, 1, 1),
            "chunk {chunk}"
        );
    }
}

#[test]
fn near_listing_stops_at_the_cap_but_counting_does_not() {
    let text = "jdeo88 ".repeat(10);
    let mut sweeper = Sweeper::new(&strings(&["jdoe88"]), false, 3)
        .unwrap()
        .with_fuzzy(1)
        .unwrap();
    sweeper.sweep_reader("feed", Cursor::new(text)).unwrap();
    let report = sweeper.finish();
    assert_eq!(report.near_hits_total, 10);
    assert_eq!(report.counts[0].near_hits, 10);
    assert_eq!(report.near_hits.len(), 3);
    assert!(report.near_truncated);
    let offsets: Vec<u64> = report.near_hits.iter().map(|h| h.offset).collect();
    assert_eq!(offsets, vec![0, 7, 14]);
}

#[test]
fn requests_and_reports_from_before_fuzzy_matching_still_load() {
    let request: SweepRequest =
        serde_json::from_str(r#"{"terms":["jdoe88"],"inputs":["dump.txt"]}"#).unwrap();
    assert_eq!(
        (request.fuzzy, request.exact, request.max_hits),
        (0, false, 10_000)
    );
    let old_report = r#"{"exact":false,"terms":1,
        "inputs":[{"path":"dump.txt","bytes":7,"hits":1,"warning":null}],
        "counts":[{"term":"jdoe88","hits":1,"files":1}],
        "missing":[],"hits_total":1,
        "hits":[{"term":"jdoe88","input":"dump.txt","offset":0,"line":1,"column":1,"matched":"jdoe88"}],
        "truncated":false}"#;
    let report: SweepReport = serde_json::from_str(old_report).unwrap();
    assert_eq!(report.fuzzy, 0);
    assert_eq!(report.near_hits_total, 0);
    assert_eq!(report.counts[0].edits_allowed, 0);
}

// ---------------------------------------------------------------------------------------------------------------
// The independent check: near-misses worked out the plain way, from the definitions, with nothing shared with the
// sweeper but the standard library.
// ---------------------------------------------------------------------------------------------------------------

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

fn word_char(c: char) -> bool {
    c.is_alphanumeric() || "@._-+#".contains(c)
}

/// Every word in `text` with its byte offset: maximal runs of word characters, with full stops trimmed from both
/// ends. Bytes that aren't UTF-8 split words.
fn words_by_hand(text: &[u8]) -> Vec<(usize, String)> {
    // (offset, character) for every character; None for each byte that isn't part of one.
    let mut chars: Vec<(usize, Option<char>)> = Vec::new();
    let mut offset = 0;
    for chunk in text.utf8_chunks() {
        for (i, c) in chunk.valid().char_indices() {
            chars.push((offset + i, Some(c)));
        }
        offset += chunk.valid().len();
        for _ in chunk.invalid() {
            chars.push((offset, None));
            offset += 1;
        }
    }
    let mut words = Vec::new();
    let mut run: Vec<(usize, char)> = Vec::new();
    // A word character extends the run. Anything else, and the end of the text, finishes it.
    for item in chars.into_iter().map(Some).chain([None]) {
        match item.and_then(|(at, c)| c.filter(|&c| word_char(c)).map(|c| (at, c))) {
            Some((at, c)) => run.push((at, c)),
            None => {
                let start = run.iter().position(|&(_, c)| c != '.');
                let end = run.iter().rposition(|&(_, c)| c != '.');
                if let (Some(start), Some(end)) = (start, end) {
                    words.push((
                        run[start].0,
                        run[start..=end].iter().map(|&(_, c)| c).collect(),
                    ));
                }
                run.clear();
            }
        }
    }
    words
}

/// The optimal-string-alignment distance, from the full table.
fn osa(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = best;
        }
    }
    d[a.len()][b.len()]
}

/// Every near-miss the sweep should list, in its order, worked out word by word and term by term.
fn near_by_hand(terms: &[String], text: &[u8], exact: bool, fuzzy: usize) -> Vec<SweepNearHit> {
    let fold = |s: &str| {
        if exact {
            s.to_string()
        } else {
            s.to_ascii_lowercase()
        }
    };
    // The sweeper's de-duplication: first spelling wins.
    let mut kept: Vec<&String> = Vec::new();
    for term in terms {
        if !term.is_empty() && !kept.iter().any(|k| fold(k) == fold(term)) {
            kept.push(term);
        }
    }
    // Each term's allowance: by length, capped, and only for single words.
    let allowance: Vec<usize> = kept
        .iter()
        .map(|term| {
            let chars = term.chars().count();
            let by_length = if chars >= 8 {
                2
            } else if chars >= 5 {
                1
            } else {
                0
            };
            let one_word =
                term.chars().all(word_char) && !term.starts_with('.') && !term.ends_with('.');
            if one_word {
                by_length.min(fuzzy)
            } else {
                0
            }
        })
        .collect();
    let mut hits = Vec::new();
    for (offset, word) in words_by_hand(text) {
        let compared = fold(&word);
        // The whole word, then the pieces either side of each @.
        let mut pieces = vec![(0, word.len())];
        if word.contains('@') {
            let mut from = 0;
            for (at, _) in word.match_indices('@') {
                pieces.push((from, at));
                from = at + 1;
            }
            pieces.push((from, word.len()));
        }
        let mut in_word: Vec<(usize, usize, usize, usize)> = Vec::new(); // (start, edits, term, end)
        for (index, term) in kept.iter().enumerate() {
            if allowance[index] == 0 || compared.contains(&fold(term)) {
                continue;
            }
            let mut best: Option<(usize, usize, usize)> = None; // (edits, start, end)
            for &(start, end) in &pieces {
                if start == end {
                    continue;
                }
                let edits = osa(&compared[start..end], &fold(term));
                if edits <= allowance[index] && best.is_none_or(|(e, _, _)| edits < e) {
                    best = Some((edits, start, end));
                }
            }
            if let Some((edits, start, end)) = best {
                in_word.push((start, edits, index, end));
            }
        }
        in_word.sort();
        for (start, edits, index, end) in in_word {
            let (line, column) = line_and_column(text, offset + start);
            hits.push(SweepNearHit {
                term: kept[index].clone(),
                input: "text".into(),
                offset: (offset + start) as u64,
                line,
                column,
                matched: word[start..end].to_string(),
                edits,
            });
        }
    }
    hits
}

#[test]
fn near_misses_match_working_them_out_by_hand_at_every_chunk_size() {
    let mut rng = Lcg(2027);
    // Pieces of text: bits of words, the six marks, separators of several kinds, two-byte letters, a four-byte
    // emoji, and bytes that aren't UTF-8 (a stray 0xFF, and a two-byte lead with nothing after it).
    let pieces: [&[u8]; 24] = [
        b"jd",
        b"oe",
        b"eo",
        b"88",
        b"a",
        b"b",
        b"ab",
        b"ba",
        b"A",
        b"B",
        b"@",
        b".",
        b"_",
        b"-",
        b"#",
        b" ",
        b"\n",
        b",",
        "\u{e9}".as_bytes(),
        "\u{201c}".as_bytes(),
        "\u{1f600}".as_bytes(),
        "\u{a0}".as_bytes(),
        b"\xFF",
        b"\xC3",
    ];
    let term_pieces = [
        "jd", "oe", "eo", "88", "a", "b", "ab", "@", ".", "\u{e9}", "A", "_", " ",
    ];
    // Characters for typing mistakes, including a capital, a space, a full stop and an @, so typos also split,
    // trim and cut words into pieces.
    let typo_chars = [
        'j', 'd', 'o', 'e', '8', 'a', 'b', 'A', '@', '.', '_', '\u{e9}', ' ',
    ];
    // What usually sits either side of a planted copy, so most copies stand as words of their own.
    let gaps: [&[u8]; 5] = [
        b" ",
        b"\n",
        b", ",
        "\u{201c}".as_bytes(),
        "\u{a0}".as_bytes(),
    ];
    let mut near_found = 0;
    for round in 0..600 {
        let exact = round % 3 == 0;
        let fuzzy = 1 + (round % 2);
        let terms: Vec<String> = (0..1 + rng.below(4))
            .map(|_| {
                (0..2 + rng.below(5))
                    .map(|_| term_pieces[rng.below(term_pieces.len() as u64) as usize])
                    .collect()
            })
            .collect();
        // Random text, with copies of the terms planted in it, each with 0 to 3 typos: exact copies, near-misses,
        // and misses one typo too far. Most copies are set apart by a gap; the rest are glued to their
        // neighbours.
        let mut text: Vec<u8> = Vec::new();
        // Every fifth text starts with a byte-order mark, as Windows PowerShell 5.1 writes it.
        if round % 5 == 0 {
            text.extend_from_slice("\u{feff}".as_bytes());
        }
        for _ in 0..rng.below(60) {
            if rng.below(3) == 0 {
                let gap = gaps[rng.below(gaps.len() as u64) as usize];
                let apart = rng.below(4) != 0;
                if apart {
                    text.extend_from_slice(gap);
                }
                let term = &terms[rng.below(terms.len() as u64) as usize];
                let mut cs: Vec<char> = term.chars().collect();
                for _ in 0..rng.below(4) {
                    let typo = typo_chars[rng.below(typo_chars.len() as u64) as usize];
                    let at = rng.below(cs.len() as u64 + 1) as usize;
                    match rng.below(4) {
                        0 => cs.insert(at, typo),
                        1 if at < cs.len() => {
                            cs.remove(at);
                        }
                        2 if at < cs.len() => cs[at] = typo,
                        3 if at + 1 < cs.len() => cs.swap(at, at + 1),
                        _ => {}
                    }
                }
                text.extend(cs.into_iter().collect::<String>().bytes());
                if apart {
                    text.extend_from_slice(gap);
                }
            } else {
                text.extend_from_slice(pieces[rng.below(pieces.len() as u64) as usize]);
            }
        }
        let expected = near_by_hand(&terms, &text, exact, fuzzy);
        near_found += expected.len();
        for chunk in [1, 2, 3, 5, 8, 64, 1 << 20] {
            let mut sweeper = Sweeper::new(&terms, exact, usize::MAX)
                .unwrap()
                .with_chunk_bytes(chunk)
                .unwrap()
                .with_fuzzy(fuzzy)
                .unwrap();
            sweeper.sweep_reader("text", Cursor::new(&text)).unwrap();
            let report = sweeper.finish();
            assert_eq!(
                report.near_hits,
                expected,
                "terms {terms:?}, exact {exact}, fuzzy {fuzzy}, chunk {chunk}, text {:?}",
                String::from_utf8_lossy(&text)
            );
            assert_eq!(report.near_hits_total, expected.len() as u64);
            assert_eq!(report.inputs[0].near_hits, expected.len() as u64);
            let per_term: u64 = report.counts.iter().map(|c| c.near_hits).sum();
            assert_eq!(per_term, expected.len() as u64);
        }
    }
    // The cases must actually exercise near-misses, not just agree on finding none. This generator plants 658.
    assert!(
        near_found > 400,
        "only {near_found} near-misses in the random cases"
    );
}
