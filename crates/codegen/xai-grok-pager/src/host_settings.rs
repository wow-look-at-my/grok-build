//! The settings writer the unsandboxed host worker serves for a confined session.

use xai_grok_sandbox::ci_host::{SettingWriteRequest, SettingWriteResponse};

/// Serve one `setting` request from the host worker.
pub fn handle_worker_request(request: &SettingWriteRequest) -> SettingWriteResponse {
    // Resolving through the allowlist both rejects an unknown key and yields the
    // `'static` key the settings writers take.
    let Some(key) = xai_grok_sandbox::ci_host::ALLOWED_SETTING_KEYS
        .iter()
        .copied()
        .find(|allowed| *allowed == request.key)
    else {
        return SettingWriteResponse::failed(format!(
            "`{}` is not an allowlisted setting key",
            request.key
        ));
    };
    let value = match crate::app::effects::helpers::setting_value_from_wire(key, &request.value) {
        Ok(value) => value,
        Err(error) => return SettingWriteResponse::failed(error),
    };
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return SettingWriteResponse::failed("could not start the settings runtime".to_string());
    };
    match runtime.block_on(crate::app::effects::helpers::write_setting_in_process(
        key, value,
    )) {
        Ok(()) => SettingWriteResponse::done(),
        Err(error) => SettingWriteResponse::failed(error),
    }
}
