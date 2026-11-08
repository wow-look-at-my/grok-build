//! Rewrite the key tables of the config reference page into the JSON layout.

use std::collections::BTreeMap;

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: config_reference_to_json_layout <26-config-reference.md>");
        std::process::exit(2);
    };
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    std::fs::write(&path, rewrite(&text)).unwrap_or_else(|e| panic!("{path}: {e}"));
}

const TABLE_HEAD: &str =
    "| Key | Type / Values | Requirements | Managed | Details |\n| --- | --- | --- | --- | --- |\n";

fn rewrite(text: &str) -> String {
    let start = text.find("\n### ").expect("no key sections") + 1;
    let end = text[start..]
        .find("\n## ")
        .map(|i| start + i + 1)
        .expect("no section after the key tables");
    let (head, body, tail) = (&text[..start], &text[start..end], &text[end..]);

    let mut sections: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in body.lines() {
        let Some(rest) = line.strip_prefix("| `") else {
            continue;
        };
        let Some(close) = rest.find('`') else {
            continue;
        };
        let key = &rest[..close];
        if key == "Key" {
            continue;
        }
        // `auth` is the spelling of `login`; the JSON layout documents only
        // `login`
        if key == "auth" || key.starts_with("auth.") {
            continue;
        }
        let json = xai_grok_config::layout::internal_path_to_json(key);
        let section = json.split('.').next().unwrap_or(&json).to_string();
        let row = format!("| `{json}`{}", translate_cells(&rest[close + 1..]));
        sections.entry(section).or_default().push(row);
    }

    let mut out = String::from(head);
    for (section, mut rows) in sections {
        rows.sort();
        out.push_str(&format!("### `{section}`\n\n{TABLE_HEAD}"));
        for row in rows {
            out.push_str(&row);
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(tail);
    out
}

/// Replace TOML section references (`` `[a.b]` `` and `` `[[a.b]]` ``) and backticked internal key paths in the other cells with JSON paths.
fn translate_cells(cells: &str) -> String {
    let mut out = String::with_capacity(cells.len());
    let mut rest = cells;
    while let Some(open) = rest.find('`') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('`') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let inner = &after[..close];
        out.push('`');
        out.push_str(&translate_code(inner));
        out.push('`');
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

fn translate_code(inner: &str) -> String {
    let bare = inner.trim_start_matches('[').trim_end_matches(']');
    let is_path = !bare.is_empty()
        && bare
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '<' | '>' | '*'));
    if !is_path || !(inner.starts_with('[') || bare.contains('.')) {
        return inner.to_string();
    }
    let json = xai_grok_config::layout::internal_path_to_json(bare);
    if inner.starts_with('[') {
        return json;
    }
    json
}
