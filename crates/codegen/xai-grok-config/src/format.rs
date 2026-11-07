//! The on-disk config format.

use std::path::{Path, PathBuf};

/// The key that names the schema of a JSON config file. The loader drops it.
pub const SCHEMA_KEY: &str = "$schema";

/// The file name of the schema that grok writes next to the user config.
pub const SCHEMA_FILENAME: &str = "config.schema.json";

/// The schema text for this build. `schemas/config.schema.json` in the repository holds the same bytes.
pub const CONFIG_SCHEMA_JSON: &str = include_str!("../../../../schemas/config.schema.json");

/// The `$schema` value that grok writes into the user config. It is relative to the config file.
pub const USER_SCHEMA_REF: &str = "./config.schema.json";

/// The format of one config file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
    Json,
    Toml,
}

impl ConfigFormat {
    /// The format of `path`, from its extension.
    pub fn of(path: &Path) -> Self {
        match path.extension() {
            Some(ext) if ext.eq_ignore_ascii_case("json") => Self::Json,
            _ => Self::Toml,
        }
    }
}

/// Parse config text that was read from `path`. The path selects the format and labels the error.
/// Empty text is an empty table.
pub fn parse_config_source(path: &Path, source: &str) -> std::io::Result<toml::Value> {
    if source.trim().is_empty() {
        return Ok(empty_table());
    }
    match ConfigFormat::of(path) {
        ConfigFormat::Json => parse_json_source(path, source),
        ConfigFormat::Toml => parse_toml_source(path, source),
    }
}

/// Parse a JSON config document. The root must be an object.
/// A `null` member is the same as an absent member. A `null` array element is an error.
pub fn parse_json_source(path: &Path, source: &str) -> std::io::Result<toml::Value> {
    let json: serde_json::Value = serde_json::from_str(source).map_err(|e| {
        // serde_json errors give the line and the column and never quote the source, so they are safe to log
        let detail = format!(
            "JSON parse error at line {}, column {}: {}",
            e.line(),
            e.column(),
            json_error_kind(&e)
        );
        tracing::error!(file = %path.display(), "config json has syntax errors: {detail}");
        std::io::Error::new(std::io::ErrorKind::InvalidData, detail)
    })?;
    let serde_json::Value::Object(mut root) = json else {
        let detail = "the root of a JSON config must be an object".to_string();
        tracing::error!(file = %path.display(), "{detail}");
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, detail));
    };
    root.remove(SCHEMA_KEY);
    let mut tree = json_to_tree(&serde_json::Value::Object(root)).map_err(|detail| {
        tracing::error!(file = %path.display(), "config json is not valid: {detail}");
        std::io::Error::new(std::io::ErrorKind::InvalidData, detail)
    })?;
    crate::layout::json_to_internal(&mut tree);
    Ok(tree)
}

/// Convert a JSON config document (JSON layout) to the internal tree.
pub fn json_document_to_internal(
    doc: &serde_json::Map<String, serde_json::Value>,
) -> Result<toml::Value, String> {
    let mut doc = doc.clone();
    doc.remove(SCHEMA_KEY);
    let mut tree = json_to_tree(&serde_json::Value::Object(doc))?;
    crate::layout::json_to_internal(&mut tree);
    Ok(tree)
}

/// Convert an internal tree to a JSON config document (JSON layout).
pub fn internal_to_json_document(tree: &toml::Value) -> serde_json::Map<String, serde_json::Value> {
    let mut tree = tree.clone();
    crate::layout::internal_to_json(&mut tree);
    match tree_to_json(&tree) {
        serde_json::Value::Object(map) => map,
        _ => serde_json::Map::new(),
    }
}

/// Read, edit and atomically write a JSON config file through the internal tree.
/// `edit` sees the internal keys (`mcp_servers`, `toolset`, ...); the file keeps the JSON layout.
/// A file that does not parse is an error, so a writer never replaces a file it cannot read.
pub fn edit_config_tree<T>(
    path: &Path,
    schema_ref: Option<&str>,
    edit: impl FnOnce(&mut toml::Value) -> T,
) -> std::io::Result<T> {
    edit_json_document(path, schema_ref, |doc| {
        let mut tree = json_document_to_internal(doc).map_err(|detail| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("refusing to overwrite {}: {detail}", path.display()),
            )
        })?;
        let out = edit(&mut tree);
        *doc = internal_to_json_document(&tree);
        Ok(out)
    })
}

/// Read a JSON config file as the internal tree, without `$VAR` expansion or version overrides.
/// Absent or blank is an empty table.
pub fn read_config_tree(path: &Path) -> std::io::Result<toml::Value> {
    let doc = read_json_document(path)?;
    json_document_to_internal(&doc).map_err(|detail| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{}: {detail}", path.display()),
        )
    })
}

fn json_error_kind(e: &serde_json::Error) -> String {
    // Display appends " at line L column C"; the caller already states the location
    let text = e.to_string();
    match text.rfind(" at line ") {
        Some(i) => text.get(..i).unwrap_or(&text).to_string(),
        None => text,
    }
}

/// Parse a legacy TOML config document.
pub fn parse_toml_source(path: &Path, source: &str) -> std::io::Result<toml::Value> {
    toml::from_str::<toml::Value>(source).map_err(|e| {
        // The detail is built from the span, never from Display: Display echoes the source line, which can hold a secret
        let detail = crate::loader::toml_error_detail(source, &e);
        tracing::error!(file = %path.display(), "config toml has syntax errors: {detail}");
        std::io::Error::new(std::io::ErrorKind::InvalidData, detail)
    })
}

fn empty_table() -> toml::Value {
    toml::Value::Table(toml::map::Map::new())
}

/// Convert a JSON value to the config tree.
pub fn json_to_tree(value: &serde_json::Value) -> Result<toml::Value, String> {
    json_to_tree_at(value, &mut Vec::new())
}

fn json_to_tree_at(
    value: &serde_json::Value,
    path: &mut Vec<String>,
) -> Result<toml::Value, String> {
    Ok(match value {
        serde_json::Value::Null => {
            return Err(format!(
                "`{}` is null; delete the member to unset it",
                dotted(path)
            ));
        }
        serde_json::Value::Bool(b) => toml::Value::Boolean(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml::Value::Integer(i)
            } else if n.is_u64() {
                return Err(format!(
                    "`{}` is larger than the largest integer ({})",
                    dotted(path),
                    i64::MAX
                ));
            } else {
                toml::Value::Float(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        serde_json::Value::String(s) => toml::Value::String(s.clone()),
        serde_json::Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                path.push(i.to_string());
                out.push(json_to_tree_at(item, path)?);
                path.pop();
            }
            toml::Value::Array(out)
        }
        serde_json::Value::Object(map) => {
            let mut out = toml::map::Map::new();
            for (key, item) in map {
                if item.is_null() {
                    continue;
                }
                path.push(key.clone());
                out.insert(key.clone(), json_to_tree_at(item, path)?);
                path.pop();
            }
            toml::Value::Table(out)
        }
    })
}

fn dotted(path: &[String]) -> String {
    if path.is_empty() {
        return "<root>".to_string();
    }
    path.join(".")
}

/// Convert the config tree to JSON. A TOML datetime becomes its RFC 3339 string.
pub fn tree_to_json(value: &toml::Value) -> serde_json::Value {
    match value {
        toml::Value::String(s) => serde_json::Value::String(s.clone()),
        toml::Value::Integer(i) => serde_json::Value::from(*i),
        toml::Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            // JSON has no NaN or infinity; a string keeps the value visible instead of a silent drop
            .unwrap_or_else(|| serde_json::Value::String(f.to_string())),
        toml::Value::Boolean(b) => serde_json::Value::Bool(*b),
        toml::Value::Datetime(d) => serde_json::Value::String(d.to_string()),
        toml::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(tree_to_json).collect())
        }
        toml::Value::Table(table) => serde_json::Value::Object(
            table
                .iter()
                .map(|(k, v)| (k.clone(), tree_to_json(v)))
                .collect(),
        ),
    }
}

/// Remove every `null` member, at every depth. Use it on a value that `serde_json::to_value` made from a struct.
pub fn strip_nulls(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.retain(|_, v| !v.is_null());
            for v in map.values_mut() {
                strip_nulls(v);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                strip_nulls(item);
            }
        }
        _ => {}
    }
}

/// Serialize a config document as JSON with tab indents and a trailing newline.
/// `schema_ref`, when given, becomes the first member as `$schema`.
pub fn to_config_json_string(
    root: &serde_json::Map<String, serde_json::Value>,
    schema_ref: Option<&str>,
) -> String {
    let mut out = serde_json::Map::new();
    if let Some(schema_ref) = schema_ref {
        out.insert(
            SCHEMA_KEY.to_string(),
            serde_json::Value::String(schema_ref.to_string()),
        );
    }
    for (k, v) in root {
        if k == SCHEMA_KEY {
            continue;
        }
        out.insert(k.clone(), v.clone());
    }
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"\t");
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
    serde::Serialize::serialize(&serde_json::Value::Object(out), &mut ser)
        .expect("serializing a serde_json::Value to a Vec cannot fail");
    let mut text = String::from_utf8(buf).expect("serde_json writes UTF-8");
    text.push('\n');
    text
}

/// [`to_config_json_string`] for an internal tree, in the JSON layout.
pub fn tree_to_config_json_string(root: &toml::Value, schema_ref: Option<&str>) -> String {
    to_config_json_string(&internal_to_json_document(root), schema_ref)
}

/// Read a JSON config document for a read-modify-write.
/// Absent or blank is an empty object. Text that does not parse is an error, so a writer never replaces a file it cannot read.
pub fn read_json_document(
    path: &Path,
) -> std::io::Result<serde_json::Map<String, serde_json::Value>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(serde_json::Map::new()),
        Err(e) => return Err(e),
    };
    parse_json_document(path, &text)
}

/// Parse JSON config text for a read-modify-write. Blank is an empty object.
pub fn parse_json_document(
    path: &Path,
    text: &str,
) -> std::io::Result<serde_json::Map<String, serde_json::Value>> {
    if text.trim().is_empty() {
        return Ok(serde_json::Map::new());
    }
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(serde_json::Value::Object(map)) => Ok(map),
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "refusing to overwrite {}: the root is not a JSON object",
                path.display()
            ),
        )),
        Err(e) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "refusing to overwrite {}: JSON parse error at line {}, column {}: {}; fix the syntax error first",
                path.display(),
                e.line(),
                e.column(),
                json_error_kind(&e)
            ),
        )),
    }
}

/// Read, edit and atomically write a JSON config file. A leaf symlink is followed.
/// The written file names the schema when `schema_ref` is given.
pub fn edit_json_document<T>(
    path: &Path,
    schema_ref: Option<&str>,
    edit: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> std::io::Result<T>,
) -> std::io::Result<T> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let dest = crate::fs_atomic::bind_follow_destination(path)?;
    let mut doc = read_json_document(&dest.path)?;
    // Keep the `$schema` the file already names when the caller gives none
    let existing_ref = doc
        .get(SCHEMA_KEY)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let out = edit(&mut doc)?;
    let text = to_config_json_string(&doc, schema_ref.or(existing_ref.as_deref()));
    crate::fs_atomic::write_atomically_bound(&dest, &text, None)?;
    Ok(out)
}

/// The `$schema` value for a config file at `path`: the relative schema ref for a file in the user home, else none.
pub fn schema_ref_for(path: &Path) -> Option<&'static str> {
    let home = xai_dirs::resolve_grok_home()?;
    (path.parent() == Some(home.as_path())).then_some(USER_SCHEMA_REF)
}

/// Get a nested JSON object, creating each missing level. A non-object on the way is replaced by an object.
pub fn json_object_at<'a>(
    root: &'a mut serde_json::Map<String, serde_json::Value>,
    path: &[&str],
) -> &'a mut serde_json::Map<String, serde_json::Value> {
    let mut cur = root;
    for seg in path {
        let slot = cur
            .entry(seg.to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if !slot.is_object() {
            *slot = serde_json::Value::Object(serde_json::Map::new());
        }
        cur = slot
            .as_object_mut()
            .expect("the slot was just made an object");
    }
    cur
}

/// The JSON path and the TOML path for the config file `stem` in `dir`.
pub fn config_file_pair(dir: &Path, stem: &str) -> (PathBuf, PathBuf) {
    (
        dir.join(format!("{stem}.json")),
        dir.join(format!("{stem}.toml")),
    )
}

/// The file to read for config `stem` in `dir`: the JSON file, or the TOML
/// file when only that exists. With neither, the JSON path, which a writer
/// then creates. A legacy TOML file that sits next to a JSON file is ignored,
/// and a warning names it.
pub fn resolve_config_file(dir: &Path, stem: &str) -> PathBuf {
    let (json, toml) = config_file_pair(dir, stem);
    if json.exists() {
        if toml.is_file() {
            tracing::warn!(
                ignored = %toml.display(),
                used = %json.display(),
                "a legacy TOML config sits next to its JSON replacement; grok reads only the JSON file. Delete the TOML file."
            );
        }
        return json;
    }
    if toml.is_file() {
        return toml;
    }
    json
}

/// The suffix a migrated legacy file gets, so the original stays available.
pub const MIGRATED_SUFFIX: &str = "migrated";

/// Convert the legacy `<stem>.toml` in a directory that grok owns to `<stem>.json`, then rename the TOML file to `<stem>.toml.migrated`.
/// Returns the path to read: the JSON file after a migration, or the TOML file when the migration could not write.
/// A TOML file that does not parse is not migrated, and the error goes to the caller.
pub fn migrate_legacy_config(
    dir: &Path,
    stem: &str,
    schema_ref: Option<&str>,
) -> std::io::Result<PathBuf> {
    let (json, toml) = config_file_pair(dir, stem);
    if json.exists() || !toml.is_file() {
        return Ok(resolve_config_file(dir, stem));
    }
    let source = std::fs::read_to_string(&toml)?;
    let tree = if source.trim().is_empty() {
        empty_table()
    } else {
        parse_toml_source(&toml, &source)?
    };
    let text = to_config_json_string(&internal_to_json_document(&tree), schema_ref);
    match crate::fs_atomic::write_atomically_if_absent(
        &json,
        &text,
        crate::fs_atomic::unix_file_mode(&toml),
    ) {
        Ok(()) => {}
        // Another process migrated first; its file has the same content
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => {
            tracing::warn!(
                legacy = %toml.display(),
                error = %e,
                "could not write the JSON config that replaces a legacy TOML config; reading the TOML file for now"
            );
            return Ok(toml);
        }
    }
    let mut migrated = toml.clone().into_os_string();
    migrated.push(".");
    migrated.push(MIGRATED_SUFFIX);
    match std::fs::rename(&toml, &migrated) {
        Ok(()) => {
            tracing::info!(from = %toml.display(), to = %json.display(), "migrated a legacy TOML config to JSON")
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(
            legacy = %toml.display(),
            error = %e,
            "migrated a legacy TOML config to JSON but could not rename the TOML file; grok ignores it"
        ),
    }
    Ok(json)
}

/// Write `config.schema.json` into `dir` when it is absent or differs from this build's schema.
pub fn ensure_schema_file(dir: &Path) -> std::io::Result<()> {
    let path = dir.join(SCHEMA_FILENAME);
    if std::fs::read_to_string(&path).is_ok_and(|current| current == CONFIG_SCHEMA_JSON) {
        return Ok(());
    }
    crate::fs_atomic::write_atomically(&path, CONFIG_SCHEMA_JSON, None)
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
