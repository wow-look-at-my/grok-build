use super::*;

const UPDATE_ENV: &str = "GROK_UPDATE_PAGER_SCHEMA";

fn schema_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../schemas/pager.schema.json")
}

#[test]
fn pager_schema_file_matches_the_appearance_types() {
    let generated = pager_json_schema_text();
    let path = schema_path();
    if std::env::var(UPDATE_ENV).as_deref() == Ok("1") {
        std::fs::write(&path, &generated).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap();
    assert!(
        committed == generated,
        "{} is not the schema the appearance types make. Regenerate it with \
         `{UPDATE_ENV}=1 cargo test -p xai-grok-pager-render --lib appearance::schema`.",
        path.display()
    );
    assert!(
        xai_grok_config::format::PAGER_SCHEMA_JSON == generated,
        "xai_grok_config::format::PAGER_SCHEMA_JSON is stale; rebuild after you regenerate the file"
    );
}

fn validator() -> jsonschema::Validator {
    jsonschema::validator_for(&pager_json_schema()).unwrap()
}

fn errors(validator: &jsonschema::Validator, doc: &Value) -> Vec<String> {
    validator
        .iter_errors(doc)
        .map(|e| format!("{}: {e}", e.instance_path))
        .collect()
}

#[test]
fn the_root_has_the_sections_and_refuses_other_keys() {
    let schema = pager_json_schema();
    let props = schema["properties"].as_object().unwrap();
    for key in [
        "$schema",
        "terminal",
        "animation",
        "prompt",
        "scrollback",
        "disable_plugins",
        "show_plan_chip",
    ] {
        assert!(props.contains_key(key), "no `{key}` member");
    }
    assert_eq!(schema["additionalProperties"], json!(false));
}

#[test]
fn doc_comments_are_the_descriptions() {
    let text = pager_json_schema_text();
    for needle in [
        "Opt-in: keep manually folded blocks as-is",
        "Whether to apply 2-char indent",
        "Minimum content lines to show",
        "collapsed_edit_blocks",
        "Names are case-insensitive",
    ] {
        assert!(text.contains(needle), "the schema lacks {needle:?}");
    }
}

#[test]
fn runtime_only_keys_are_not_in_the_schema() {
    let text = pager_json_schema_text();
    for key in [
        "body_dim_italic",
        "collapsed_expand_hint",
        "rail_under_bullet",
    ] {
        assert!(!text.contains(key), "`{key}` is not a pager.json key");
    }
}

#[test]
fn a_full_file_is_valid() {
    let doc = json!({
        "$schema": "./pager.schema.json",
        "terminal": { "alt_screen": "never", "minimal": true, "minimal_live_rows": 12 },
        "animation": { "fps": 60, "wave_rows": 16, "show_fps": false },
        "prompt": { "collapse_unfocused": false, "mouse_hover": true, "show_prefix": true },
        "disable_plugins": true,
        "show_plan_chip": true,
        "scrollback": {
            "layout": { "outer_vpad": 0, "outer_hpad_left": 1 },
            "scrollbar": { "enabled": true, "scrollbar_fg": "#7aa2f7", "scrollbar_bg": "none" },
            "scroll": { "margin": 3, "follow_indicator": "none", "respect_manual_folds": true },
            "display": { "tab_width": 2, "rtl_bidi": null, "collapsed_accent_char": "|" },
            "blocks": {
                "edit": { "bg": "dark", "accent": [122, 162, 247], "expanded_by_default": null },
                "prompt": { "bg": "light", "min_lines": 3 },
                "thinking": { "accent": "MAGENTA", "bg_blend": 50 },
                "tool": { "bullet": "small-triangle" },
                "list_dir": { "terminal_bg": false },
                "execute": { "header_style": "shell", "running_accent": "magenta" }
            }
        }
    });
    let validator = validator();
    assert_eq!(errors(&validator, &doc), Vec::<String>::new());
}

#[test]
fn a_misspelt_key_or_a_wrong_type_is_invalid() {
    let validator = validator();
    for doc in [
        json!({ "bogus": 1 }),
        json!({ "scrollback": { "scrol": {} } }),
        json!({ "scrollback": { "blocks": { "edit": { "indnet": true } } } }),
        json!({ "animation": { "fps": "fast" } }),
        json!({ "terminal": { "alt_screen": "sometimes" } }),
        json!({ "scrollback": { "blocks": { "edit": { "accent": [1, 2] } } } }),
    ] {
        assert!(!validator.is_valid(&doc), "{doc} must be invalid");
    }
}
