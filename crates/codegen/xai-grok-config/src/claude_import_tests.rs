use super::is_claude_import_marked;

#[test]
fn only_an_explicit_true_marker_reads_as_imported() {
    let cases = [
        (None, false),
        (Some(r#"{"other": {"key": "value"}}"#), false),
        (Some(r#"{"claude_compat": {"imported": false}}"#), false),
        (Some(r#"{"claude_compat": {"imported": "#), false),
        (Some(r#"{"claude_compat": {"imported": true}}"#), true),
    ];

    for (config, expected) in cases {
        let home = tempfile::tempdir().expect("create temp grok home");
        if let Some(config) = config {
            std::fs::write(home.path().join(crate::USER_CONFIG_FILENAME), config)
                .expect("write config");
        }

        assert_eq!(expected, is_claude_import_marked(home.path()), "{config:?}");
    }
}

#[test]
fn a_legacy_toml_marker_still_reads_as_imported() {
    let home = tempfile::tempdir().expect("create temp grok home");
    std::fs::write(
        home.path().join("config.toml"),
        "[claude_compat]\nimported = true\n",
    )
    .expect("write config");
    assert!(is_claude_import_marked(home.path()));
}
