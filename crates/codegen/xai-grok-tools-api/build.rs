fn main() {
    let generated = xai_proto_build::configure()
        .type_attribute(
            ".", // match every message & enum
            "#[derive(serde::Serialize, serde::Deserialize)]",
        )
        // ToolConfigEntry is embedded in external JSON contracts (Computer Hub
        // `session.bind` metadata and agent-config JSON) where sparse payloads
        // must deserialize. Defaults are applied per optional field (not
        // type-level) so the required `id` field still fails deserialization
        // when missing instead of silently becoming "". See tests/wire_shape.rs.
        .field_attribute(
            ".xai.grok.tools.v1.ToolConfigEntry.params_json",
            "#[serde(default)]",
        )
        .field_attribute(
            ".xai.grok.tools.v1.ToolConfigEntry.name_override",
            "#[serde(default)]",
        )
        .field_attribute(
            ".xai.grok.tools.v1.ToolConfigEntry.params_name_overrides",
            "#[serde(default)]",
        )
        .field_attribute(
            ".xai.grok.tools.v1.ToolConfigEntry.behavior_version",
            "#[serde(default)]",
        )
        .field_attribute(
            ".xai.grok.tools.v1.ToolConfigEntry.description_override",
            "#[serde(default)]",
        )
        .field_attribute(
            ".xai.grok.tools.v1.FinalizeToolServerConfigRequest.client_callback_addr",
            "#[serde(default)]",
        )
        .field_attribute(
            ".xai.grok.tools.v1.FinalizeToolServerConfigRequest.session_id",
            "#[serde(default)]",
        )
        .field_attribute(
            ".xai.grok.tools.v1.FinalizeToolServerConfigRequest.client_callback_secret",
            "#[serde(default)]",
        )
        .field_attribute(
            ".xai.grok.tools.v1.FinalizeToolServerConfigResponse.callback_status",
            "#[serde(default)]",
        )
        .compile_protos(&["proto/grok-tools.proto"], &["proto/"]);
    // A build script has no caller to hand an error to, so the failure is
    // printed and the script exits non-zero. That is the report; a panic would
    // say the same thing with a backtrace attached.
    if let Err(err) = generated {
        eprintln!("grok-tools.proto codegen failed: {err}");
        std::process::exit(1);
    }
}
