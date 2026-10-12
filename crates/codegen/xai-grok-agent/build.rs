//! Writes `prompt_encrypted.rs` into `OUT_DIR` from the prompt templates.

use std::fmt::Write as _;
use std::path::Path;

/// Each constant, the template it holds, and the seed of its key.
const TEMPLATES: &[(&str, &str, u8)] = &[
    ("BASE_PROMPT_ENC", "prompt.md", 0x5A),
    ("SUBAGENT_PROMPT_ENC", "subagent_prompt.md", 0x3D),
];

/// XORs each byte with the seed plus its position, which wraps at a set value.
fn xor_encrypt(data: &[u8], seed: u8) -> Vec<u8> {
    let mut key = seed;
    data.iter()
        .map(|&b| {
            let enc = b ^ key;
            key = key.wrapping_add(1);
            enc
        })
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR")?;
    let out_dir = std::env::var("OUT_DIR")?;
    let mut out = String::new();
    for (name, file, seed) in TEMPLATES {
        let path = Path::new(&manifest).join("templates").join(file);
        println!("cargo:rerun-if-changed={}", path.display());
        let data =
            std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let bytes: Vec<String> = xor_encrypt(&data, *seed)
            .iter()
            .map(u8::to_string)
            .collect();
        writeln!(
            out,
            "pub(crate) const {name}: &[u8] = &[{}];",
            bytes.join(", ")
        )?;
    }
    let seeds: Vec<String> = TEMPLATES
        .iter()
        .map(|(_, _, s)| format!("0x{s:02X}"))
        .collect();
    writeln!(
        out,
        "pub(crate) const PROMPT_SEEDS: [u8; {}] = [{}];",
        TEMPLATES.len(),
        seeds.join(", ")
    )?;
    let dest = Path::new(&out_dir).join("prompt_encrypted.rs");
    std::fs::write(&dest, out).map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
    Ok(())
}
