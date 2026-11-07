//! Convert the fenced TOML config snippets in Markdown files to JSON in the config layout.

use std::path::Path;

fn main() {
    let mut failed = 0usize;
    let mut converted = 0usize;
    for arg in std::env::args().skip(1) {
        let path = Path::new(&arg);
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{arg}: {e}");
                std::process::exit(1);
            }
        };
        let (out, ok, bad) = convert(&arg, &text);
        converted += ok;
        failed += bad;
        if out != text
            && let Err(e) = std::fs::write(path, out)
        {
            eprintln!("{arg}: {e}");
            std::process::exit(1);
        }
    }
    eprintln!("converted {converted} block(s); {failed} block(s) left as TOML");
}

fn convert(name: &str, text: &str) -> (String, usize, usize) {
    let mut out = String::with_capacity(text.len());
    let mut lines = text.split_inclusive('\n').enumerate().peekable();
    let (mut ok, mut bad) = (0, 0);
    while let Some((n, line)) = lines.next() {
        let trimmed = line.trim_start();
        let indent = &line[..line.len() - trimmed.len()];
        if trimmed.trim_end() != "```toml" {
            out.push_str(line);
            continue;
        }
        let mut body = String::new();
        let mut closed = false;
        for (_, inner) in lines.by_ref() {
            if inner.trim() == "```" {
                closed = true;
                break;
            }
            body.push_str(inner.strip_prefix(indent).unwrap_or(inner));
        }
        match to_json(&body) {
            Some(json) if closed => {
                ok += 1;
                out.push_str(indent);
                out.push_str("```json\n");
                for l in json.lines() {
                    out.push_str(indent);
                    out.push_str(l);
                    out.push('\n');
                }
                out.push_str(indent);
                out.push_str("```\n");
            }
            _ => {
                bad += 1;
                eprintln!("{name}:{}: TOML block left as it is", n + 1);
                out.push_str(line);
                for l in body.split_inclusive('\n') {
                    out.push_str(indent);
                    out.push_str(l);
                }
                if closed {
                    out.push_str(indent);
                    out.push_str("```\n");
                }
            }
        }
    }
    (out, ok, bad)
}

fn to_json(body: &str) -> Option<String> {
    let tree = toml::from_str::<toml::Value>(body).ok()?;
    let doc = xai_grok_config::format::internal_to_json_document(&tree);
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(doc)).ok()?;
    Some(text)
}
