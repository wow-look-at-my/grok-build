# JSON config notes

Every config file that a person writes is JSON. `schemas/config.schema.json` describes `config.json`. grok writes the same schema to `$GROK_HOME/config.schema.json` at startup, and `config.json` names it with `"$schema": "./config.schema.json"`.

`xai_grok_config::format` is the format boundary. The extension picks the parser: `.json` is JSON, and anything else is the legacy TOML. A `null` member means unset. Every writer goes through `parse_config_for_edit` and `render_config`, which refuse a `.toml` path.

`xai_grok_config::layout::LAYOUT` maps each JSON section to its internal key. Each row moves a whole subtree, and a key that no row names keeps its path. The merge, the requirements tiers, campaigns and every reader use the internal keys. A new setting needs a layout row only when it belongs in a different JSON section. `layout_tests.rs` checks that the table round-trips. After a layout change, regenerate the schema with `GROK_UPDATE_CONFIG_SCHEMA=1 cargo test -p xai-grok-shell --lib config_schema`.

The server signs the `$GROK_HOME` managed-config and requirements caches as TOML bytes. Those caches stay TOML and keep the internal keys.

At startup `prepare_user_config_dir` converts the legacy TOML files in `$GROK_HOME` to JSON and keeps each original as `<name>.toml.migrated`. A TOML file that does not parse stays where it is, and the error names it. grok never renames a file in a project. It reads a legacy `.grok/config.toml` in place with a warning, and converts it when a grok command writes that project config.
