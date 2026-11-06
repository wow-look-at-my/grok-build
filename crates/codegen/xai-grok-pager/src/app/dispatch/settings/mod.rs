pub(in crate::app::dispatch) mod dashboard;
pub(in crate::app::dispatch) mod setters;
pub(in crate::app::dispatch) mod ui;

use super::status::toast_persist_failure;
use crate::app::actions::Effect;
use crate::app::app_view::AppView;
use crate::app::dispatch::settings::ui::save_success_toast;
use crate::settings::SettingValue;
use xai_grok_shell::agent::config::Feature;

pub(in crate::app::dispatch) fn handle_setting_persisted(
    app: &mut AppView,
    key: &str,
    value: SettingValue,
) {
    if let ("dashboard_preview", SettingValue::Bool(enabled)) = (key, value) {
        // An older save must not close a preview the user has re-enabled.
        if app.current_ui.dashboard_preview_enabled() != enabled {
            return;
        }
        if let Some(dashboard) = app.dashboard.as_mut() {
            dashboard.set_preview_enabled(enabled, &mut app.agents);
        }
        app.show_toast(&save_success_toast("Dashboard preview", enabled));
    }
}

/// One `[features]` write finished. The row that issued it settles and may issue the intent queued behind it;
/// `subagent_model_inheritance` is the only row today.
pub(in crate::app::dispatch) fn handle_feature_override_persisted(
    app: &mut AppView,
    feature: Feature,
    result: Result<Option<bool>, String>,
) -> Vec<Effect> {
    let persisted = match result {
        Ok(saved) => Some(saved),
        Err(error) => {
            tracing::warn!(target: "settings", key = feature.key(), %error, "setting persist failed");
            toast_persist_failure(app, feature.key(), &error);
            None
        }
    };
    let effects = if feature == app.subagent_model_inheritance.feature {
        setters::settle_subagent_model_inheritance_write(app, persisted)
    } else {
        vec![]
    };
    ui::refresh_open_settings_modals(app);
    effects
}

/// Set one `[model."<id>"] context_window` limit. The mirror changes now. A failed write puts it back.
pub(in crate::app::dispatch) fn set_model_context_window(
    app: &mut AppView,
    model_id: String,
    label: String,
    tokens: Option<u64>,
) -> Vec<Effect> {
    let previous = app.model_context_limits.get(&model_id).copied();
    if previous == tokens {
        return vec![];
    }
    tracing::info!(
        target: "settings",
        key = crate::settings::context_limits::CONTEXT_WINDOW_LIMIT_KEY,
        model = %model_id,
        ?tokens,
        ?previous,
        "setting changed",
    );
    set_mirror(app, &model_id, tokens);
    ui::refresh_open_settings_modals(app);
    vec![Effect::PersistModelContextWindow {
        model_id,
        label,
        tokens,
        previous,
    }]
}

/// Remove every configured limit. The reset of the context limit row sends this.
pub(in crate::app::dispatch) fn clear_model_context_windows(app: &mut AppView) -> Vec<Effect> {
    let limits = std::mem::take(&mut app.model_context_limits);
    ui::refresh_open_settings_modals(app);
    limits
        .into_iter()
        .map(|(model_id, previous)| Effect::PersistModelContextWindow {
            label: model_id.clone(),
            model_id,
            tokens: None,
            previous: Some(previous),
        })
        .collect()
}

fn set_mirror(app: &mut AppView, model_id: &str, tokens: Option<u64>) {
    match tokens {
        Some(tokens) => app.model_context_limits.insert(model_id.to_owned(), tokens),
        None => app.model_context_limits.remove(model_id),
    };
}

/// One `[model."<id>"] context_window` write finished.
pub(in crate::app::dispatch) fn handle_model_context_window_persisted(
    app: &mut AppView,
    persisted: crate::app::actions::ModelContextWindowWrite,
    result: Result<(), String>,
) -> Vec<Effect> {
    let crate::app::actions::ModelContextWindowWrite {
        model_id,
        label,
        tokens,
        previous,
    } = persisted;
    if let Err(error) = result {
        tracing::warn!(
            target: "settings",
            key = crate::settings::context_limits::CONTEXT_WINDOW_LIMIT_KEY,
            model = %model_id,
            %error,
            "setting persist failed; rolled back",
        );
        // A later write of the same model owns the mirror.
        if app.model_context_limits.get(&model_id).copied() == tokens {
            set_mirror(app, &model_id, previous);
            ui::refresh_open_settings_modals(app);
        }
        toast_persist_failure(app, "context window limit", &error);
        return vec![];
    }
    app.show_toast(&context_window_saved_toast(&label, tokens));
    vec![]
}

/// The toast text for a saved limit.
pub(in crate::app::dispatch) fn context_window_saved_toast(
    label: &str,
    tokens: Option<u64>,
) -> String {
    match tokens {
        Some(tokens) => format!(
            "\u{2713} Context window limit for {label}: {}",
            crate::settings::context_limits::short_tokens(tokens)
        ),
        None => format!("\u{2713} Context window limit for {label} removed"),
    }
}
