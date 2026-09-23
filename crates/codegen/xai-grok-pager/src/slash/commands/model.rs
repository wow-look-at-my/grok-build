//! `/model` (alias `/m`) — switch model + (optionally) reasoning effort.
//! Chained autocomplete: pick a reasoning-supported model → trailing space
//! re-opens the dropdown into a `low|medium|high|xhigh` sub-menu.

use agent_client_protocol as acp;
use xai_grok_shell::sampling::types::{
    endpoint_meta, favorite_meta, loaded_in_vram_meta, provider_meta,
    supports_reasoning_effort_meta,
};

use crate::acp::model_state::ModelState;
use crate::app::actions::Action;
use crate::slash::command::{AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand};
use crate::slash::commands::effort_levels::build_effort_arg_items;

/// Switch the active model (and optionally its reasoning effort).
pub struct ModelCommand;

impl SlashCommand for ModelCommand {
    fn name(&self) -> &str {
        "model"
    }

    fn aliases(&self) -> &[&str] {
        &["m"]
    }

    fn description(&self) -> &str {
        "Switch the active model"
    }

    fn session_scoped(&self) -> bool {
        true
    }

    fn offered_when_session_less(&self) -> bool {
        // The dashboard offers `/model` to pick the model for the next
        // spawned agent (intercepted in `dispatch_dashboard_dispatch_slash`).
        true
    }

    fn usage(&self) -> &str {
        "/model <name> [effort]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn args_required(&self) -> bool {
        true
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("<model> [effort]")
    }

    fn suggest_args(&self, ctx: &AppCtx, args_query: &str) -> Option<Vec<ArgItem>> {
        if ctx.models.is_empty() {
            return None;
        }

        // Effort phase if input is "<reasoning-model> ", else model phase.
        if let Some(model_id) = detect_effort_phase(ctx.models, args_query) {
            return Some(build_effort_items(ctx.models, &model_id));
        }
        // The opening list is the favorites. A typed query lists every model,
        // so a provider with hundreds of them still answers a search for one
        // nobody marked. The caller ranks what it gets back.
        let favorites_only = args_query.trim().is_empty();
        Some(build_model_items(ctx.models, favorites_only))
    }

    fn search_args(&self, ctx: &AppCtx, args_query: &str) -> Option<Vec<ArgItem>> {
        if ctx.models.is_empty() {
            return None;
        }
        if let Some(model_id) = detect_effort_phase(ctx.models, args_query) {
            return Some(build_effort_items(ctx.models, &model_id));
        }
        Some(build_model_items(ctx.models, false))
    }

    fn run(&self, ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let trimmed = args.trim();
        if trimmed.is_empty() {
            return CommandResult::Error("Usage: /model <name> [effort]".into());
        }

        // Prefer an exact full-string catalog match first. Model display names
        // often contain spaces ("Grok 4.5"); if we split on the last token
        // first, a shorter catalog entry ("Grok") would steal the prefix and
        // treat "4.5" as an effort level.
        if let Some(id) = ctx.models.resolve_by_name_or_id(trimmed) {
            return CommandResult::Action(Action::SetDefaultModel(id));
        }

        // Trailing effort token + reasoning model → session-scoped switch
        // (not persisted as default). Resolve via the shared gate so a rejected
        // level (e.g. `none` on grok-4.5) surfaces the effort error with the
        // model's offered ids — not "Unknown model: … none".
        if let Some((prefix, token)) = split_trailing_token(trimmed)
            && let Some(id) = resolve_model(ctx.models, prefix)
            && ctx
                .models
                .available
                .get(&id)
                .map(supports_reasoning_effort)
                .unwrap_or(false)
        {
            return match ctx.models.resolve_effort_for_model(&id, token) {
                Ok(effort) => CommandResult::Action(Action::SwitchModel {
                    model_id: id,
                    effort: Some(effort),
                }),
                Err(err) => CommandResult::Error(err.message()),
            };
        }

        CommandResult::Error(format!("Unknown model: {trimmed}"))
    }
}

/// Look up a model by case-insensitive display name OR model id match.
fn resolve_model(models: &ModelState, name: &str) -> Option<acp::ModelId> {
    models.resolve_by_name_or_id(name)
}

fn supports_reasoning_effort(info: &acp::ModelInfo) -> bool {
    supports_reasoning_effort_meta(info.meta.as_ref())
}

/// Split `args` into `(prefix, last_token)` on the final whitespace run.
/// Returns `None` when there is no interior whitespace to split on. The token is
/// resolved to an effort against the picked model's options by the caller.
fn split_trailing_token(args: &str) -> Option<(&str, &str)> {
    let (prefix, last) = args.rsplit_once(char::is_whitespace)?;
    let prefix = prefix.trim_end();
    if prefix.is_empty() || last.is_empty() {
        return None;
    }
    Some((prefix, last))
}

/// Returns the matched model id when `args_query` is `"<reasoning-model> ..."`.
/// Longest-name-first to disambiguate names that share a prefix.
fn detect_effort_phase(models: &ModelState, args_query: &str) -> Option<acp::ModelId> {
    // A model is typed by its name or by its id; a shared name is typed by id.
    let mut candidates: Vec<(&acp::ModelId, &str)> = models
        .available
        .iter()
        .filter(|(_, info)| supports_reasoning_effort(info))
        .flat_map(|(id, info)| [(id, id.0.as_ref()), (id, info.name.as_str())])
        .collect();
    candidates.sort_by_key(|(_, name)| std::cmp::Reverse(name.len()));

    for (id, name) in candidates {
        if args_query.len() > name.len()
            && args_query.is_char_boundary(name.len())
            && args_query[..name.len()].eq_ignore_ascii_case(name)
            && args_query[name.len()..].starts_with(char::is_whitespace)
        {
            return Some(id.clone());
        }
    }
    None
}

fn is_favorite(info: &acp::ModelInfo) -> bool {
    favorite_meta(info.meta.as_ref())
}

/// Where a model routes: its provider, else its endpoint host.
fn route_label(info: &acp::ModelInfo) -> Option<String> {
    let meta = info.meta.as_ref();
    match (provider_meta(meta), endpoint_meta(meta)) {
        (Some(provider), Some(host)) => Some(format!("{provider} ({host})")),
        (Some(provider), None) => Some(provider.to_owned()),
        (None, Some(host)) => Some(host.to_owned()),
        (None, None) => None,
    }
}

/// Whether another model in the catalog has this model's display name.
fn name_is_shared(models: &ModelState, id: &acp::ModelId) -> bool {
    let Some(info) = models.available.get(id) else {
        return false;
    };
    models
        .available
        .iter()
        .any(|(other, o)| other != id && o.name.eq_ignore_ascii_case(&info.name))
}

/// The text that selects this model on the command line: the name when the
/// name resolves back to this model, else the unique id. A name that another
/// model shares, or that is another model's id, resolves elsewhere.
fn pick_token(models: &ModelState, id: &acp::ModelId) -> String {
    match models.available.get(id) {
        Some(info) if models.resolve_by_name_or_id(&info.name).as_ref() == Some(id) => {
            info.name.clone()
        }
        _ => id.0.to_string(),
    }
}

/// The suffix that tells a row apart from the others with its name: where it
/// routes, or its id when the route does not differ either.
fn disambiguator(models: &ModelState, id: &acp::ModelId, info: &acp::ModelInfo) -> String {
    let Some(route) = route_label(info) else {
        return id.0.to_string();
    };
    let route_is_shared = models.available.iter().any(|(other, o)| {
        other != id
            && o.name.eq_ignore_ascii_case(&info.name)
            && route_label(o).as_deref() == Some(route.as_str())
    });
    if route_is_shared {
        id.0.to_string()
    } else {
        route
    }
}

/// One row per logical model. Reasoning models get a trailing space in
/// `insert_text` so the prompt widget chains into the effort sub-menu.
///
/// Two rows with one name show where each routes, and select by id.
///
/// `favorites_only` narrows the list to the models the config marked, plus the
/// current one — a picker that hides what the session is running reads as a
/// model that went missing. A catalog with no favorite in it lists everything,
/// so an unconfigured session sees the whole catalog as before.
fn build_model_items(models: &ModelState, favorites_only: bool) -> Vec<ArgItem> {
    let current_id = models.current.as_ref();
    let narrow = favorites_only && models.available.values().any(is_favorite);
    let mut items: Vec<ArgItem> = Vec::with_capacity(models.available.len());
    for (id, info) in &models.available {
        let is_current = current_id == Some(id);
        if narrow && !is_current && !is_favorite(info) {
            continue;
        }
        let supports = supports_reasoning_effort(info);

        let mut display = if name_is_shared(models, id) {
            format!("{} · {}", info.name, disambiguator(models, id, info))
        } else {
            info.name.clone()
        };
        if is_current {
            display.push_str(" (current)");
        }

        let token = pick_token(models, id);
        // Trailing space on reasoning models: signals "more input
        // expected" to the prompt widget so Enter advances to effort
        // phase instead of submitting.
        let insert_text = if supports {
            format!("{token} ")
        } else {
            token.clone()
        };

        let description = match info.description.as_deref() {
            Some(d) if !d.trim().is_empty() => d.to_owned(),
            _ => route_label(info)
                .map(|route| format!("via {route}"))
                .unwrap_or_default(),
        };

        items.push(ArgItem {
            display,
            match_text: token,
            insert_text,
            description,
            // Only a provider that reports residency answers this, so the dot
            // appears beside local models and nowhere else.
            loaded_in_vram: loaded_in_vram_meta(info.meta.as_ref()),
        });
    }
    items
}

/// One row per effort level for the `/model` chained effort phase.
/// `insert_text` is `"ModelName high"` so selecting a row completes both tokens.
fn build_effort_items(models: &ModelState, model_id: &acp::ModelId) -> Vec<ArgItem> {
    if !models.available.contains_key(model_id) {
        return Vec::new();
    }
    let model_name = pick_token(models, model_id);
    let is_current_model = models.current.as_ref() == Some(model_id);
    let options = models.reasoning_effort_options_for(model_id);
    build_effort_arg_items(
        &options,
        models.reasoning_effort,
        is_current_model,
        |option| format!("{model_name} {}", option.id),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use xai_grok_shell::sampling::types::ReasoningEffort;

    fn model_with_reasoning(id: &str, name: &str) -> (acp::ModelId, acp::ModelInfo) {
        let id = acp::ModelId::new(Arc::from(id));
        let mut meta = serde_json::Map::new();
        meta.insert(
            "supportsReasoningEffort".into(),
            serde_json::Value::Bool(true),
        );
        let info = acp::ModelInfo::new(id.clone(), name.to_string())
            .meta(serde_json::Value::Object(meta).as_object().cloned());
        (id, info)
    }

    fn plain_model(id: &str, name: &str) -> (acp::ModelId, acp::ModelInfo) {
        let id = acp::ModelId::new(Arc::from(id));
        let info = acp::ModelInfo::new(id.clone(), name.to_string());
        (id, info)
    }

    fn favorite_model(id: &str, name: &str) -> (acp::ModelId, acp::ModelInfo) {
        let id = acp::ModelId::new(Arc::from(id));
        let mut meta = serde_json::Map::new();
        meta.insert(
            xai_grok_shell::sampling::types::FAVORITE_META_KEY.into(),
            serde_json::Value::Bool(true),
        );
        let info = acp::ModelInfo::new(id.clone(), name.to_string())
            .meta(serde_json::Value::Object(meta).as_object().cloned());
        (id, info)
    }

    fn ctx_for(state: &ModelState) -> AppCtx<'_> {
        AppCtx {
            models: state,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            billing_surface_visible: true,
            usage_command_visible: true,
            workflows_available: true,
            screen_mode: crate::app::ScreenMode::Fullscreen,
        }
    }

    /// Three models, one of them marked, and the session is running an
    /// unmarked one.
    fn state_with_a_favorite() -> ModelState {
        let mut state = ModelState::default();
        let (fid, finfo) = favorite_model("kept", "Kept");
        let (cid, cinfo) = plain_model("running", "Running");
        let (oid, oinfo) = plain_model("crowd-1", "Crowd One");
        state.available.insert(fid, finfo);
        state.available.insert(cid.clone(), cinfo);
        state.available.insert(oid, oinfo);
        state.current = Some(cid);
        state
    }

    #[test]
    fn the_opening_list_is_the_favorites_plus_the_running_model() {
        let state = state_with_a_favorite();
        let items = ModelCommand.suggest_args(&ctx_for(&state), "").unwrap();
        let names: Vec<&str> = items.iter().map(|i| i.match_text.as_str()).collect();
        assert_eq!(names, vec!["Kept", "Running"]);
    }

    #[test]
    fn a_typed_query_searches_past_the_favorites() {
        let state = state_with_a_favorite();
        // The caller ranks the rows, so the command's job is to offer every
        // model the moment anything is typed.
        let items = ModelCommand.suggest_args(&ctx_for(&state), "cro").unwrap();
        let names: Vec<&str> = items.iter().map(|i| i.match_text.as_str()).collect();
        assert_eq!(names, vec!["Kept", "Running", "Crowd One"]);
    }

    #[test]
    fn the_modal_picker_is_handed_every_model_to_search() {
        let state = state_with_a_favorite();
        // The modal picker asks one time and filters its own copy, so this is
        // the only chance it gets to see a model that is not a favorite.
        let items = ModelCommand.search_args(&ctx_for(&state), "").unwrap();
        let names: Vec<&str> = items.iter().map(|i| i.match_text.as_str()).collect();
        assert_eq!(names, vec!["Kept", "Running", "Crowd One"]);
    }

    fn routed_model(
        id: &str,
        name: &str,
        provider: Option<&str>,
        endpoint: &str,
    ) -> (acp::ModelId, acp::ModelInfo) {
        let id = acp::ModelId::new(Arc::from(id));
        let mut meta = serde_json::Map::new();
        if let Some(provider) = provider {
            meta.insert("provider".into(), provider.into());
        }
        meta.insert("endpoint".into(), endpoint.into());
        let info = acp::ModelInfo::new(id.clone(), name.to_string()).meta(Some(meta));
        (id, info)
    }

    /// The catalog from the report: one built-in model and the same slug
    /// listed by two providers, with no description on the listed copies.
    fn state_with_a_shared_name() -> ModelState {
        let mut state = ModelState::default();
        for (id, info) in [
            routed_model("grok-4.7", "Grok 4.7", None, "api.x.ai"),
            routed_model(
                "local/grok-4.7",
                "grok-4.7",
                Some("local"),
                "localhost:18080",
            ),
            routed_model("relay/grok-4.7", "grok-4.7", Some("relay"), "relay.example"),
        ] {
            state.available.insert(id, info);
        }
        state
    }

    #[test]
    fn rows_that_share_a_name_say_where_each_routes() {
        let state = state_with_a_shared_name();
        let items = ModelCommand.search_args(&ctx_for(&state), "").unwrap();
        let displays: Vec<&str> = items.iter().map(|i| i.display.as_str()).collect();
        assert_eq!(
            displays,
            vec![
                "Grok 4.7",
                "grok-4.7 · local (localhost:18080)",
                "grok-4.7 · relay (relay.example)",
            ]
        );
        assert_eq!(
            items[1].description, "via local (localhost:18080)",
            "a listed model with no description still says what it is"
        );
    }

    #[test]
    fn each_row_that_shares_a_name_selects_its_own_model() {
        let state = state_with_a_shared_name();
        let items = ModelCommand.search_args(&ctx_for(&state), "").unwrap();
        for (item, expected) in items[1..].iter().zip(["local/grok-4.7", "relay/grok-4.7"]) {
            assert_eq!(item.insert_text, expected);
            let mut ctx = dummy_exec_ctx(&state);
            match ModelCommand.run(&mut ctx, &item.insert_text) {
                CommandResult::Action(Action::SetDefaultModel(id)) => {
                    assert_eq!(id.0.as_ref(), expected);
                }
                other => panic!("expected SetDefaultModel({expected}), got {other:?}"),
            }
        }
    }

    #[test]
    fn a_shared_reasoning_name_chains_into_effort_by_id() {
        let mut state = ModelState::default();
        let (a, ainfo) = model_with_reasoning("local/r", "R");
        let (b, binfo) = model_with_reasoning("relay/r", "R");
        state.available.insert(a, ainfo);
        state.available.insert(b, binfo);

        let items = ModelCommand
            .suggest_args(&ctx_for(&state), "relay/r ")
            .unwrap();
        assert_eq!(items[0].insert_text, "relay/r xhigh");
    }

    #[test]
    fn an_id_beats_another_models_name() {
        let mut state = ModelState::default();
        let (a, ainfo) = plain_model("first", "second");
        let (b, binfo) = plain_model("second", "Second Model");
        state.available.insert(a, ainfo);
        state.available.insert(b.clone(), binfo);
        assert_eq!(state.resolve_by_name_or_id("second"), Some(b));

        let items = ModelCommand.search_args(&ctx_for(&state), "").unwrap();
        assert_eq!(
            items[0].insert_text, "first",
            "a name that is another model's id selects by this model's id"
        );
    }

    #[test]
    fn a_catalog_with_no_favorite_lists_everything() {
        let mut state = ModelState::default();
        let (a, ainfo) = plain_model("one", "One");
        let (b, binfo) = plain_model("two", "Two");
        state.available.insert(a, ainfo);
        state.available.insert(b, binfo);

        let items = ModelCommand.suggest_args(&ctx_for(&state), "").unwrap();
        assert_eq!(items.len(), 2, "an unconfigured session loses no model");
    }

    static EMPTY_BUNDLE: crate::app::bundle::BundleState = crate::app::bundle::BundleState {
        has_cache: false,
        version: String::new(),
        personas: Vec::new(),
        roles: Vec::new(),
        agents: Vec::new(),
        skills: Vec::new(),
        persona_details: Vec::new(),
        role_details: Vec::new(),
    };

    fn dummy_exec_ctx(models: &ModelState) -> CommandExecCtx<'_> {
        CommandExecCtx {
            models,
            session_id: None,
            bundle_state: &EMPTY_BUNDLE,
            screen_mode: crate::app::ScreenMode::Inline,
            billing_surface_visible: true,
            usage_command_visible: true,
            pager_state: crate::settings::PagerLocalSnapshot {
                multiline_mode: false,
                yolo_mode: false,
                ..crate::settings::PagerLocalSnapshot::default()
            },
        }
    }

    #[test]
    fn split_trailing_token_splits_on_final_whitespace() {
        assert_eq!(
            split_trailing_token("Reasoning X high"),
            Some(("Reasoning X", "high"))
        );
        assert_eq!(
            split_trailing_token("reasoning-x  xhigh"),
            Some(("reasoning-x", "xhigh"))
        );
        // No interior whitespace → nothing to split off.
        assert!(split_trailing_token("reasoning-x-pro").is_none());
    }

    #[test]
    fn empty_query_returns_one_row_per_logical_model() {
        let mut state = ModelState::default();
        let (rid, rinfo) = model_with_reasoning("reasoning-x", "Reasoning X");
        let (pid, pinfo) = plain_model("grok-4.5", "Grok 4.5");
        state.available.insert(rid, rinfo);
        state.available.insert(pid, pinfo);

        let cmd = ModelCommand;
        let ctx = AppCtx {
            models: &state,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            billing_surface_visible: true,
            usage_command_visible: true,
            workflows_available: true,
            screen_mode: crate::app::ScreenMode::Fullscreen,
        };
        let items = cmd.suggest_args(&ctx, "").unwrap();
        assert_eq!(items.len(), 2, "model phase: one row per logical model");

        // Reasoning model has trailing space in insert_text -- this is the
        // signal the prompt widget reads to keep the dropdown open after
        // Enter so the effort sub-menu can render.
        let reasoning = items
            .iter()
            .find(|i| i.match_text == "Reasoning X")
            .unwrap();
        assert_eq!(reasoning.insert_text, "Reasoning X ");

        // Plain model has no trailing space -- Enter commits immediately.
        let plain = items.iter().find(|i| i.match_text == "Grok 4.5").unwrap();
        assert_eq!(plain.insert_text, "Grok 4.5");
    }

    #[test]
    fn trailing_space_after_reasoning_model_enters_effort_phase() {
        let mut state = ModelState::default();
        let (id, info) = model_with_reasoning("reasoning-x", "Reasoning X");
        state.available.insert(id, info);

        let cmd = ModelCommand;
        let ctx = AppCtx {
            models: &state,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            billing_surface_visible: true,
            usage_command_visible: true,
            workflows_available: true,
            screen_mode: crate::app::ScreenMode::Fullscreen,
        };
        // Args query has a trailing space -> effort phase. Items come out
        // ordered xhigh -> low (strongest first) per EFFORT_LEVELS.
        let items = cmd.suggest_args(&ctx, "Reasoning X ").unwrap();
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].insert_text, "Reasoning X xhigh");
        assert_eq!(items[1].insert_text, "Reasoning X high");
        assert_eq!(items[2].insert_text, "Reasoning X medium");
        assert_eq!(items[3].insert_text, "Reasoning X low");
        // Display is just the level so the user sees a clean column.
        assert_eq!(items[0].display, "xhigh");
        // match_text carries the sort-key prefix that forces the matcher's
        // alphabetical tiebreak to render rows in EFFORT_LEVELS order.
        assert!(items[0].match_text.starts_with("a "));
        assert!(items[3].match_text.starts_with("d "));
    }

    #[test]
    fn partial_effort_query_still_in_effort_phase() {
        let mut state = ModelState::default();
        let (id, info) = model_with_reasoning("reasoning-x", "Reasoning X");
        state.available.insert(id, info);

        let cmd = ModelCommand;
        let ctx = AppCtx {
            models: &state,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            billing_surface_visible: true,
            usage_command_visible: true,
            workflows_available: true,
            screen_mode: crate::app::ScreenMode::Fullscreen,
        };
        // Still in effort phase; matcher upstream narrows to high / xhigh.
        let items = cmd.suggest_args(&ctx, "Reasoning X h").unwrap();
        assert_eq!(items.len(), 4);
    }

    #[test]
    fn partial_model_query_stays_in_model_phase() {
        let mut state = ModelState::default();
        let (id, info) = model_with_reasoning("reasoning-x", "Reasoning X");
        state.available.insert(id, info);

        let cmd = ModelCommand;
        let ctx = AppCtx {
            models: &state,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            billing_surface_visible: true,
            usage_command_visible: true,
            workflows_available: true,
            screen_mode: crate::app::ScreenMode::Fullscreen,
        };
        // No trailing space, user is still typing the model name.
        let items = cmd.suggest_args(&ctx, "Reason").unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].insert_text, "Reasoning X ");
    }

    #[test]
    fn run_parses_model_plus_effort_when_supported() {
        let mut state = ModelState::default();
        let (id, info) = model_with_reasoning("reasoning-x", "Reasoning X");
        state.available.insert(id, info);
        let mut ctx = dummy_exec_ctx(&state);
        let result = ModelCommand.run(&mut ctx, "Reasoning X xhigh");
        match result {
            CommandResult::Action(Action::SwitchModel { model_id, effort }) => {
                assert_eq!(model_id.0.as_ref(), "reasoning-x");
                assert_eq!(effort, Some(ReasoningEffort::Xhigh));
            }
            other => panic!("expected SwitchModel with effort, got {other:?}"),
        }
    }

    #[test]
    fn run_rejects_unoffered_effort_with_effort_error_not_unknown_model() {
        // Regression: previously `resolve_effort_token_for` returned None and
        // the handler fell through to `Unknown model: Reasoning X none`.
        let mut state = ModelState::default();
        let (id, info) = model_with_reasoning("reasoning-x", "Reasoning X");
        state.available.insert(id, info);
        let mut ctx = dummy_exec_ctx(&state);
        let result = ModelCommand.run(&mut ctx, "Reasoning X none");
        match result {
            CommandResult::Error(msg) => {
                assert!(
                    msg.contains("unknown effort level 'none'"),
                    "expected effort error, got {msg}"
                );
                assert!(
                    msg.contains("use one of:"),
                    "expected offered levels in message, got {msg}"
                );
                assert!(
                    !msg.to_lowercase().contains("unknown model"),
                    "must not misreport as unknown model: {msg}"
                );
                let offered = msg.split_once("; ").map(|(_, r)| r).unwrap_or("");
                assert!(
                    !offered.contains("none"),
                    "must not list none as offered: {msg}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn run_prefers_full_multi_word_model_name_over_prefix_plus_effort() {
        // Catalog has both "Grok" (reasoning) and "Grok 4.5". `/model Grok 4.5`
        // must select the full name, not treat "4.5" as an effort on "Grok".
        let mut state = ModelState::default();
        let (short_id, short_info) = model_with_reasoning("grok", "Grok");
        let (long_id, long_info) = model_with_reasoning("grok-4.5", "Grok 4.5");
        state.available.insert(short_id, short_info);
        state.available.insert(long_id.clone(), long_info);
        let mut ctx = dummy_exec_ctx(&state);
        let result = ModelCommand.run(&mut ctx, "Grok 4.5");
        match result {
            CommandResult::Action(Action::SetDefaultModel(resolved_id)) => {
                assert_eq!(resolved_id, long_id);
            }
            other => panic!("expected SetDefaultModel(Grok 4.5), got {other:?}"),
        }
    }

    #[test]
    fn run_rejects_effort_for_non_reasoning_model() {
        let mut state = ModelState::default();
        let (id, info) = plain_model("grok-4.5", "Grok 4.5");
        state.available.insert(id, info);
        let mut ctx = dummy_exec_ctx(&state);
        let result = ModelCommand.run(&mut ctx, "Grok 4.5 high");
        // Falls through to "is the whole string a model name?" — which
        // it isn't, so we get an Unknown error.
        assert!(matches!(result, CommandResult::Error(_)));
    }

    /// The bare `/model <name>` form dispatches
    /// `Action::SetDefaultModel(<ModelId>)` instead of the legacy
    /// `Action::SwitchModel { effort: None }`. The dispatcher routes
    /// the typed setter through both `Effect::SwitchModel`
    /// (session-level mutation) AND `Effect::PersistSetting`
    /// (next-session default).
    ///
    /// The payload is the typed `acp::ModelId` (resolved at the slash
    /// boundary), not a String.
    #[test]
    fn run_bare_model_name_dispatches_set_default_model() {
        let mut state = ModelState::default();
        let (id, info) = plain_model("grok-4.5", "Grok 4.5");
        state.available.insert(id.clone(), info);
        let mut ctx = dummy_exec_ctx(&state);
        let result = ModelCommand.run(&mut ctx, "Grok 4.5");
        match result {
            CommandResult::Action(Action::SetDefaultModel(resolved_id)) => {
                assert_eq!(resolved_id, id);
            }
            other => panic!("expected Action::SetDefaultModel(<id>), got {other:?}"),
        }
    }

    /// Case-insensitive matching against the catalog: `/model grok 4.5`
    /// resolves to the same `ModelId` as `/model Grok 4.5`.
    #[test]
    fn run_set_default_model_resolves_case_insensitively() {
        let mut state = ModelState::default();
        let (id, info) = plain_model("grok-4.5", "Grok 4.5");
        state.available.insert(id.clone(), info);
        let mut ctx = dummy_exec_ctx(&state);
        let result = ModelCommand.run(&mut ctx, "grok 4.5");
        match result {
            CommandResult::Action(Action::SetDefaultModel(resolved_id)) => {
                assert_eq!(resolved_id, id);
            }
            other => panic!("expected Action::SetDefaultModel(<id>), got {other:?}"),
        }
    }
}
