// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

use std::time::Duration;

use chrono::{TimeZone, Timelike, Utc};

use skeletrace::{
    AdapterKind, AdapterProfile, HttpRequestProfile, LookylooMetricBindings, LookylooSourceConfig,
    MetricId, Quality,
};

#[test]
fn lookyloo_profile_builds_http_poller_adapter() {
    let config = LookylooSourceConfig {
        payload_root_pointer: Some("/captures".into()),
        include_raw_payload: true,
        default_quality: Quality::new(1.0).unwrap(),
        metrics: LookylooMetricBindings {
            title: Some(MetricId::new()),
            root_url: Some(MetricId::new()),
            final_url: None,
            redirect_count: Some(MetricId::new()),
            has_error: Some(MetricId::new()),
            error_text: None,
            no_index: Some(MetricId::new()),
            category_count: Some(MetricId::new()),
            categories_joined: None,
            has_parent: None,
            parent_capture: None,
            user_agent: None,
            referer: None,
            capture_dir: None,
        },
    };

    let profile = AdapterProfile::LookylooSummary {
        config,
        request_profile: HttpRequestProfile::direct(Duration::from_secs(5)),
    };

    assert_eq!(profile.expected_kind(), AdapterKind::HttpPoller);
    profile.validate().unwrap();
    let mut adapter = profile.build_adapter().unwrap();
    assert_eq!(adapter.kind(), AdapterKind::HttpPoller);
    assert!(adapter
        .as_any_mut()
        .downcast_mut::<skeletrace::LookylooSummaryAdapter>()
        .is_some());
}

#[test]
fn lookyloo_timestamp_parser_accepts_python_cache_format() {
    let body = serde_json::json!({
        "uuid": "abc",
        "timestamp": "2025-03-23T12:34:56.123456+0000",
        "url": "https://example.test"
    });

    let summary = skeletrace::LookylooCaptureSummary::from_value(&body).unwrap();
    assert_eq!(
        summary.timestamp.unwrap(),
        Utc.with_ymd_and_hms(2025, 3, 23, 12, 34, 56)
            .unwrap()
            .with_nanosecond(123_456_000)
            .unwrap()
    );
}
