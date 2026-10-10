// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Contract tests for the standalone binary and its file boundary.

use std::process::{Command, Output};

use skeletrace::{
    CliCommand, ExportFormat, SnapshotExportJob, SnapshotRequest, TimeRange, ViewJob, ViewJobId,
    ViewKind,
};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_skeletrace"))
        .args(args)
        .output()
        .expect("the Cargo-built standalone binary must launch")
}

#[test]
fn help_and_version_only_expose_the_standalone_engine() {
    let help = cli(&["--help"]);
    assert!(help.status.success());
    assert!(help.stderr.is_empty());
    let text = String::from_utf8(help.stdout).unwrap();
    for command in ["tick", "analyze-topology", "query-advanced", "health"] {
        assert!(text.contains(command));
    }
    for removed in [
        "landscape",
        "verify-packets",
        "case",
        "OSINTropy",
        "Maxwell",
        "CrowdWatch",
    ] {
        assert!(
            !text.contains(removed),
            "removed command or tool: {removed}"
        );
    }
    assert!(text.contains("Copyright (C) 2026 whispr.dev"));
    let version = cli(&["--version"]);
    assert!(version.status.success());
    let version = String::from_utf8(version.stdout).unwrap();
    // The first line stays `skeletrace X.Y.Z` for scripts that read it; the notice follows, as GNU programs give it.
    let mut lines = version.lines();
    assert_eq!(
        lines.next(),
        Some(concat!("skeletrace ", env!("CARGO_PKG_VERSION")))
    );
    assert_eq!(lines.next(), Some("Copyright (C) 2026 whispr.dev"));
    assert!(version.contains("GPL-3.0-only") && version.contains("NO WARRANTY"));
}

#[test]
fn the_help_screen_opens_with_the_logo_and_nothing_else_does() {
    let logo_top = ".:---::.";
    let help = cli(&["--help"]);
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout).unwrap();
    let first = text.lines().find(|line| !line.trim().is_empty()).unwrap();
    assert_eq!(first.trim(), logo_top);
    assert!(text.find(logo_top).unwrap() < text.find("USAGE:").unwrap());
    // With no command at all, the same screen goes to stderr, with the usage exit code.
    let bare = cli(&[]);
    assert_eq!(bare.status.code(), Some(2));
    assert!(bare.stdout.is_empty());
    assert!(String::from_utf8_lossy(&bare.stderr).contains(logo_top));
    // The version line carries no logo. Command output is JSON with no logo either: the sweep contract tests
    // parse it.
    let version = cli(&["--version"]);
    assert!(!String::from_utf8_lossy(&version.stdout).contains(logo_top));
}

#[test]
fn malformed_commands_return_usage_status_and_do_not_emit_json() {
    for args in [
        vec![],
        vec!["tick"],
        vec!["landscape"],
        vec!["landscape-report", "unused.json"],
        vec!["verify-packets", "unused.json"],
        vec!["case", "demo"],
        vec!["--help", "extra"],
        vec!["--version", "extra"],
        vec!["unknown-command"],
    ] {
        let output = cli(&args);
        assert_eq!(output.status.code(), Some(2), "args: {args:?}");
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
        assert!(CliCommand::parse_from(std::iter::once("skeletrace").chain(args)).is_err());
    }
}

#[test]
fn a_missing_profile_returns_runtime_status() {
    let path =
        std::env::temp_dir().join(format!("skeletrace-missing-{}.json", uuid::Uuid::new_v4()));
    let output = cli(&["profile-validate", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

fn snapshot_job(stem: Option<&str>) -> SnapshotExportJob {
    SnapshotExportJob {
        request: SnapshotRequest {
            entities: vec![],
            metrics: vec![],
            time_range: TimeRange::LatestOnly,
            format: ExportFormat::NativeJson,
            notes: None,
        },
        view: ViewJob {
            id: ViewJobId::new(),
            kind: ViewKind::Topology,
            entities: vec![],
            metrics: vec![],
            time_range: TimeRange::LatestOnly,
            detail_override: None,
            viewport: None,
        },
        output_stem: stem.map(str::to_string),
    }
}

#[test]
fn snapshot_stems_cannot_escape_output_directory_or_use_windows_reserved_names() {
    for bad in [
        "",
        " ",
        ".",
        "..",
        "../outside",
        "/outside",
        "sub/entry",
        "sub\\entry",
        "C:\\outside",
        "D:outside",
        "file\nname",
        "a:b",
        "a?b",
        "a*b",
        "a|b",
        "a\"b",
        "a<b",
        "a>b",
        "trailing.",
        "trailing ",
        "CON",
        "nul.txt",
        "COM1",
        "LPT9.json",
        "COM¹",
        "CONIN$",
        "CONOUT$",
    ] {
        assert!(
            snapshot_job(Some(bad)).validate().is_err(),
            "accepted: {bad:?}"
        );
    }
    for good in [
        None,
        Some("capture"),
        Some("case-42"),
        Some("topology.v2"),
        Some("Welsh map"),
    ] {
        assert!(snapshot_job(good).validate().is_ok(), "rejected: {good:?}");
    }
}
