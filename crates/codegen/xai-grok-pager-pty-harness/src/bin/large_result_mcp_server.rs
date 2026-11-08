//! A stdio MCP server whose tool `dump` returns a result larger than the inline MCP limit.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

const TAIL_MARKER: &str = "LARGE_MCP_RESULT_TAIL";

fn large_result() -> String {
    let mut items: Vec<String> = (0..800)
        .map(|i| format!("item number {i} of the large result"))
        .collect();
    items.push(TAIL_MARKER.to_owned());
    serde_json::to_string_pretty(&json!({ "items": items })).expect("serialize the large result")
}

fn answer(method: &str, params: &Value) -> Value {
    match method {
        "initialize" => json!({
            "protocolVersion": params.get("protocolVersion").cloned().unwrap_or(json!("2025-06-18")),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "large-result", "version": "1.0.0" }
        }),
        "tools/list" => json!({
            "tools": [{
                "name": "dump",
                "description": "Return a large JSON document.",
                "inputSchema": { "type": "object", "properties": {} }
            }]
        }),
        "tools/call" => json!({
            "content": [{ "type": "text", "text": large_result() }],
            "isError": false
        }),
        _ => json!({}),
    }
}

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { return };
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        // A notification has no id and gets no answer.
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        let response = json!({ "jsonrpc": "2.0", "id": id, "result": answer(method, &params) });
        if writeln!(stdout, "{response}").is_err() || stdout.flush().is_err() {
            return;
        }
    }
}
