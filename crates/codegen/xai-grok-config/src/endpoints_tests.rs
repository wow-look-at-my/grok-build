use super::*;

fn endpoints(
    proxy: &str,
    models_base_url: Option<&str>,
    models_list_url: Option<&str>,
) -> EndpointsConfig {
    EndpointsConfig {
        cli_chat_proxy_base_url: Some(proxy.to_owned()),
        models_base_url: models_base_url.map(|s| s.to_owned()),
        models_list_url: models_list_url.map(|s| s.to_owned()),
        ..Default::default()
    }
}

#[test]
fn inference_url_defaults_to_proxy() {
    let ep = endpoints("https://proxy.grok.invalid/v1", None, None);
    assert_eq!(
        ep.resolve_inference_base_url(),
        "https://proxy.grok.invalid/v1"
    );
}

#[test]
fn inference_url_uses_models_base_url() {
    let ep = endpoints(
        "https://proxy.grok.invalid/v1",
        Some("https://enterprise.acme.com/v1"),
        None,
    );
    assert_eq!(
        ep.resolve_inference_base_url(),
        "https://enterprise.acme.com/v1"
    );
}

#[test]
fn inference_url_ignores_models_list_url() {
    let ep = endpoints(
        "https://proxy.grok.invalid/v1",
        Some("https://inference.acme.com/v1"),
        Some("https://registry.acme.com/api/models"),
    );
    assert_eq!(
        ep.resolve_inference_base_url(),
        "https://inference.acme.com/v1"
    );
}

#[test]
fn list_url_defaults_to_proxy_models() {
    let ep = endpoints("https://proxy.grok.invalid/v1", None, None);
    assert_eq!(
        ep.resolve_models_list_url(),
        "https://proxy.grok.invalid/v1/models"
    );
}

#[test]
fn list_url_derived_from_base_url() {
    let ep = endpoints(
        "https://proxy.grok.invalid/v1",
        Some("https://api.example.invalid/v1"),
        None,
    );
    assert_eq!(
        ep.resolve_models_list_url(),
        "https://api.example.invalid/v1/models"
    );
}

#[test]
fn list_url_explicit_overrides_derivation() {
    let ep = endpoints(
        "https://proxy.grok.invalid/v1",
        Some("https://inference.acme.com/v1"),
        Some("https://registry.acme.com/api/list-models"),
    );
    assert_eq!(
        ep.resolve_models_list_url(),
        "https://registry.acme.com/api/list-models"
    );
}

#[test]
fn otlp_traces_endpoint_precedence() {
    let proxy = "https://inference.acme.com/v1".to_string();

    let derived = EndpointsConfig {
        cli_chat_proxy_base_url: Some(proxy.clone()),
        ..Default::default()
    };
    assert_eq!(
        derived.resolve_otlp_traces_endpoint(),
        "https://inference.acme.com/v1/traces"
    );

    let base = EndpointsConfig {
        cli_chat_proxy_base_url: Some(proxy.clone()),
        otel_exporter_otlp_endpoint: Some("https://otel.acme.com".to_string()),
        ..Default::default()
    };
    assert_eq!(
        base.resolve_otlp_traces_endpoint(),
        "https://otel.acme.com/v1/traces"
    );

    let full = EndpointsConfig {
        cli_chat_proxy_base_url: Some(proxy),
        otel_exporter_otlp_endpoint: Some("https://ignored.example".to_string()),
        otel_exporter_otlp_traces_endpoint: Some("https://otel.acme.com/v1/traces".to_string()),
        ..Default::default()
    };
    assert_eq!(
        full.resolve_otlp_traces_endpoint(),
        "https://otel.acme.com/v1/traces"
    );
}

#[test]
fn otlp_headers_trim_whitespace_and_skip_blank_keys() {
    let cfg = EndpointsConfig {
        otel_exporter_otlp_headers: Some("a=1, b = 2 ,=skip,c=".to_string()),
        ..Default::default()
    };

    assert_eq!(
        cfg.resolve_otlp_headers(),
        vec![
            ("a".to_string(), "1".to_string()),
            ("b".to_string(), "2".to_string()),
            ("c".to_string(), String::new()),
        ]
    );
}

/// A config with a fixed proxy URL, every OTLP field unset, and the master switch off.
/// `EndpointsConfig::default()` would fill each of these from env vars and config files.
fn internal_otlp_test_config() -> EndpointsConfig {
    EndpointsConfig {
        cli_chat_proxy_base_url: Some("https://proxy.example/v1".to_string()),
        otel_exporter_otlp_endpoint: None,
        otel_exporter_otlp_traces_endpoint: None,
        otel_exporter_otlp_headers: None,
        grok_internal_otlp_traces_endpoint: None,
        grok_internal_otlp_headers: None,
        external_otel_master_switch: false,
        ..Default::default()
    }
}

#[test]
fn internal_otlp_endpoint_grok_internal_wins_regardless_of_switch() {
    for switch in [false, true] {
        let cfg = EndpointsConfig {
            grok_internal_otlp_traces_endpoint: Some(
                "https://internal.example/traces/".to_string(),
            ),
            otel_exporter_otlp_traces_endpoint: Some(
                "https://legacy.example/v1/traces".to_string(),
            ),
            otel_exporter_otlp_endpoint: Some("https://legacy-base.example".to_string()),
            external_otel_master_switch: switch,
            ..internal_otlp_test_config()
        };
        assert_eq!(
            cfg.resolve_otlp_traces_endpoint(),
            "https://internal.example/traces",
            "switch={switch}: GROK_INTERNAL_OTLP_TRACES_ENDPOINT must win verbatim (trailing / trimmed)"
        );
    }
}

#[test]
fn internal_otlp_endpoint_legacy_fallback_when_switch_unset() {
    let traces = EndpointsConfig {
        otel_exporter_otlp_traces_endpoint: Some("https://legacy.example/v1/traces".to_string()),
        ..internal_otlp_test_config()
    };
    assert_eq!(
        traces.resolve_otlp_traces_endpoint(),
        "https://legacy.example/v1/traces"
    );

    let base = EndpointsConfig {
        otel_exporter_otlp_endpoint: Some("https://legacy-base.example/".to_string()),
        ..internal_otlp_test_config()
    };
    assert_eq!(
        base.resolve_otlp_traces_endpoint(),
        "https://legacy-base.example/v1/traces"
    );
}

#[test]
fn internal_otlp_ignores_legacy_vars_when_switch_set() {
    let cfg = EndpointsConfig {
        otel_exporter_otlp_traces_endpoint: Some(
            "https://admin-collector.example/v1/traces".to_string(),
        ),
        otel_exporter_otlp_endpoint: Some("https://admin-collector.example".to_string()),
        otel_exporter_otlp_headers: Some("authorization=Bearer admin".to_string()),
        external_otel_master_switch: true,
        ..internal_otlp_test_config()
    };

    assert_eq!(
        cfg.resolve_otlp_traces_endpoint(),
        "https://proxy.example/v1/traces",
        "internal firehose must never follow OTEL_* to the external collector"
    );
    assert_eq!(cfg.resolve_otlp_headers(), Vec::<(String, String)>::new());
}

#[test]
fn internal_otlp_consumed_standard_vars_cases() {
    struct Case {
        switch: bool,
        legacy_traces_ep: bool,
        legacy_base_ep: bool,
        legacy_headers: bool,
        internal_ep: bool,
        internal_headers: bool,
        expected: bool,
        why: &'static str,
    }

    let unset = Case {
        switch: false,
        legacy_traces_ep: false,
        legacy_base_ep: false,
        legacy_headers: false,
        internal_ep: false,
        internal_headers: false,
        expected: false,
        why: "nothing set",
    };
    let cases = [
        Case { ..unset },
        Case {
            legacy_traces_ep: true,
            expected: true,
            why: "legacy traces endpoint consumed",
            ..unset
        },
        Case {
            legacy_base_ep: true,
            expected: true,
            why: "legacy base endpoint consumed",
            ..unset
        },
        Case {
            legacy_headers: true,
            expected: true,
            why: "legacy headers consumed",
            ..unset
        },
        Case {
            legacy_traces_ep: true,
            internal_ep: true,
            expected: false,
            why: "internal endpoint shadows legacy",
            ..unset
        },
        Case {
            legacy_headers: true,
            internal_headers: true,
            expected: false,
            why: "internal headers shadow legacy",
            ..unset
        },
        Case {
            legacy_traces_ep: true,
            legacy_headers: true,
            internal_ep: true,
            expected: true,
            why: "endpoint shadowed but legacy headers still consumed (headers half)",
            ..unset
        },
        Case {
            switch: true,
            legacy_traces_ep: true,
            legacy_base_ep: true,
            legacy_headers: true,
            expected: false,
            why: "switch set: legacy vars ignored",
            ..unset
        },
    ];

    for case in cases {
        let cfg = EndpointsConfig {
            external_otel_master_switch: case.switch,
            otel_exporter_otlp_traces_endpoint: case
                .legacy_traces_ep
                .then(|| "https://legacy.example/v1/traces".to_string()),
            otel_exporter_otlp_endpoint: case
                .legacy_base_ep
                .then(|| "https://legacy-base.example".to_string()),
            otel_exporter_otlp_headers: case.legacy_headers.then(|| "k=v".to_string()),
            grok_internal_otlp_traces_endpoint: case
                .internal_ep
                .then(|| "https://internal.example/traces".to_string()),
            grok_internal_otlp_headers: case.internal_headers.then(|| "ik=iv".to_string()),
            ..internal_otlp_test_config()
        };
        assert_eq!(
            cfg.internal_otlp_consumed_standard_vars(),
            case.expected,
            "case: {}",
            case.why
        );
    }
}

#[test]
fn internal_otlp_headers_precedence() {
    for switch in [false, true] {
        let cfg = EndpointsConfig {
            grok_internal_otlp_headers: Some("x-debug=1".to_string()),
            otel_exporter_otlp_headers: Some("legacy=1".to_string()),
            external_otel_master_switch: switch,
            ..internal_otlp_test_config()
        };
        assert_eq!(
            cfg.resolve_otlp_headers(),
            vec![("x-debug".to_string(), "1".to_string())],
            "switch={switch}"
        );
    }

    let legacy = EndpointsConfig {
        otel_exporter_otlp_headers: Some("legacy=1".to_string()),
        ..internal_otlp_test_config()
    };
    assert_eq!(
        legacy.resolve_otlp_headers(),
        vec![("legacy".to_string(), "1".to_string())]
    );
}

#[test]
fn external_otel_master_switch_resolves_from_all_layers() {
    let enabled_table: toml::Value =
        toml::from_str("[telemetry]\notel_enabled = true").expect("enabled table parses");
    let disabled_table: toml::Value =
        toml::from_str("[telemetry]\notel_enabled = false").expect("disabled table parses");

    assert!(external_otel_master_switch_from(
        None,
        None,
        Some(&enabled_table)
    ));

    assert!(!external_otel_master_switch_from(None, None, None));

    assert!(!external_otel_master_switch_from(
        None,
        Some(false),
        Some(&enabled_table)
    ));
    assert!(external_otel_master_switch_from(
        None,
        Some(true),
        Some(&disabled_table)
    ));

    assert!(!external_otel_master_switch_from(
        Some(&disabled_table),
        Some(true),
        Some(&enabled_table)
    ));
    assert!(external_otel_master_switch_from(
        Some(&enabled_table),
        Some(false),
        None
    ));
}

fn parse(table: &str) -> Result<EndpointsConfig, serde_json::Error> {
    serde_json::from_str(&format!("{{{table}}}"))
}

/// An `[endpoints]` table can arrive from a managed or campaign layer.
/// A duplicate-field rejection would fail the whole config.
#[test]
fn a_table_naming_the_list_url_under_both_keys_under_one_value_parses_once() {
    let cfg = parse(
        r#""models_list_url":"https://m.test/models","models_endpoint":"https://m.test/models""#,
    )
    .expect("one value named under two keys is one value");
    assert_eq!(
        cfg.models_list_url.as_deref(),
        Some("https://m.test/models")
    );
}

#[test]
fn a_table_reading_either_list_url_spelling_alone_still_works() {
    let canonical = parse(r#""models_list_url":"https://a.test/models""#).unwrap();
    assert_eq!(
        canonical.models_list_url.as_deref(),
        Some("https://a.test/models")
    );

    let legacy = parse(r#""models_endpoint":"https://b.test/models""#).unwrap();
    assert_eq!(
        legacy.models_list_url.as_deref(),
        Some("https://b.test/models")
    );
}

#[test]
fn a_table_whose_list_url_spellings_disagree_is_an_error_naming_the_field() {
    let err = parse(r#""models_list_url":"https://a.test","models_endpoint":"https://b.test""#)
        .expect_err("two list URLs must not resolve silently");
    let text = err.to_string();
    assert!(text.contains("models_list_url"), "{err}");
    assert!(text.contains("models_endpoint"), "{err}");
}

#[test]
fn the_list_url_serializes_under_the_canonical_key_only() {
    let mut cfg = EndpointsConfig::default();
    cfg.models_list_url = Some("https://a.test/models".to_owned());
    let json = serde_json::to_value(&cfg).unwrap();
    assert_eq!(json["models_list_url"], "https://a.test/models");
    assert!(
        json.get("models_endpoint").is_none(),
        "the alias key must not appear on the wire: {json}"
    );
}

/// The wire struct repeats the field list. A field it does not read comes back as its default.
#[test]
fn the_wire_struct_reads_every_field_the_config_writes() {
    let mut cfg = EndpointsConfig::default();
    cfg.cli_chat_proxy_base_url = Some("https://proxy.test".to_owned());
    cfg.xai_api_base_url = "https://api.test".to_owned();
    cfg.allowed_endpoints = vec!["https://allow.test".to_owned()];
    cfg.alpha_test_key = Some("alpha".to_owned());
    cfg.models_base_url = Some("https://models.test".to_owned());
    cfg.models_list_url = Some("https://list.test".to_owned());
    cfg.feedback_base_url = Some("https://feedback.test".to_owned());
    cfg.trace_upload_url = Some("https://upload.test".to_owned());
    cfg.managed_config_url = Some("https://managed.test".to_owned());
    cfg.otel_exporter_otlp_endpoint = Some("https://otlp.test".to_owned());
    cfg.otel_exporter_otlp_traces_endpoint = Some("https://traces.test".to_owned());
    cfg.otel_exporter_otlp_headers = Some("a=b".to_owned());
    cfg.grok_internal_otlp_traces_endpoint = Some("https://internal.test".to_owned());
    cfg.grok_internal_otlp_headers = Some("c=d".to_owned());
    cfg.otel_traces_exporter = Some("otlp".to_owned());
    cfg.otel_traces_export_interval = Some(1_234);
    cfg.otel_exporter_otlp_timeout = Some(5_678);
    cfg.management_api_key = Some("mgmt".to_owned());

    let written = serde_json::to_value(&cfg).unwrap();
    let keys = written.as_object().expect("a table").len();
    assert!(
        keys >= 18,
        "the fixture must exercise every field the wire struct reads, got {keys}: {written}"
    );
    let read_back: EndpointsConfig =
        serde_json::from_value(written.clone()).expect("a config this type wrote must parse");
    assert_eq!(
        serde_json::to_value(&read_back).unwrap(),
        written,
        "a key the wire struct does not read comes back as its default"
    );
}

#[test]
fn a_removed_direct_upload_key_fails_the_load_and_says_it_was_removed() {
    for key in REMOVED_DIRECT_UPLOAD_KEYS {
        let err = parse(&format!("\"{key}\": \"gs://bucket\""))
            .expect_err("a removed upload key must not load");
        let message = err.to_string();
        assert!(
            message.contains(&format!("endpoints.{key}")) && message.contains("removed"),
            "{key}: {message}"
        );
    }
}

#[test]
fn an_unset_proxy_derives_blank_urls() {
    let ep = EndpointsConfig {
        cli_chat_proxy_base_url: None,
        models_base_url: None,
        models_list_url: None,
        managed_config_url: None,
        grok_internal_otlp_traces_endpoint: None,
        otel_exporter_otlp_endpoint: None,
        otel_exporter_otlp_traces_endpoint: None,
        ..Default::default()
    };
    assert_eq!(ep.proxy_url(), "");
    assert_eq!(ep.resolve_managed_config_url(), "");
    assert_eq!(ep.resolve_otlp_traces_endpoint(), "");
    assert_eq!(ep.resolve_models_list_url(), "");
}
