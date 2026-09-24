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

        if let Some(items) = sub_phase_items(ctx.models, args_query) {
            return Some(items);
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
        if let Some(items) = sub_phase_items(ctx.models, args_query) {
            return Some(items);
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
        if let Some(message) = ambiguous_name(ctx.models, trimmed) {
            return CommandResult::Error(message);
        }
        if let Some(id) = ctx.models.resolve_by_name_or_id(trimmed) {
            return CommandResult::Action(Action::SetDefaultModel(id));
        }

        // Trailing effort token + reasoning model → session-scoped switch
        // (not persisted as default). Resolve via the shared gate so a rejected
        // level (e.g. `none` on grok-4.5) surfaces the effort error with the
        // model's offered ids — not "Unknown model: … none".
        if let Some((prefix, _)) = split_trailing_token(trimmed)
            && let Some(message) = ambiguous_name(ctx.models, prefix)
        {
            return CommandResult::Error(message);
        }
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

/// Whether `query` is `token`, then whitespace, then anything.
fn leads_with(query: &str, token: &str) -> bool {
    query.len() > token.len()
        && query.is_char_boundary(token.len())
        && query[..token.len()].eq_ignore_ascii_case(token)
        && query[token.len()..].starts_with(char::is_whitespace)
}

/// The effort or route rows `args_query` leads into, if any.
///
/// The longer typed token wins. On a tie the route phase wins: a shared name
/// can equal another model's id, and the group row inserts that name.
fn sub_phase_items(models: &ModelState, args_query: &str) -> Option<Vec<ArgItem>> {
    let effort = detect_effort_phase(models, args_query);
    let route = detect_route_phase(models, args_query);
    match (effort, route) {
        (Some((_, effort_len)), Some((group, route_len))) if route_len >= effort_len => {
            Some(build_route_items(models, &group))
        }
        (Some((id, _)), _) => Some(build_effort_items(models, &id)),
        (None, Some((group, _))) => Some(build_route_items(models, &group)),
        (None, None) => None,
    }
}

/// The model and the matched length when `args_query` is
/// `"<reasoning-model> ..."`. Longest-name-first to disambiguate names that
/// share a prefix.
fn detect_effort_phase(models: &ModelState, args_query: &str) -> Option<(acp::ModelId, usize)> {
    // A model is typed by its id, or by its name when no other model has it. A
    // shared name leads to the route phase instead.
    let mut candidates: Vec<(&acp::ModelId, &str)> = models
        .available
        .iter()
        .filter(|(_, info)| supports_reasoning_effort(info))
        .flat_map(|(id, info)| {
            let name = (!name_is_shared(models, id)).then_some((id, info.name.as_str()));
            std::iter::once((id, id.0.as_ref())).chain(name)
        })
        .collect();
    candidates.sort_by_key(|(_, name)| std::cmp::Reverse(name.len()));

    candidates
        .into_iter()
        .find(|(_, name)| leads_with(args_query, name))
        .map(|(id, name)| (id.clone(), name.len()))
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

/// Every model whose display name is `name`, in catalog order.
fn models_named<'a>(models: &'a ModelState, name: &str) -> Vec<&'a acp::ModelId> {
    models
        .available
        .iter()
        .filter(|(_, info)| info.name.eq_ignore_ascii_case(name))
        .map(|(id, _)| id)
        .collect()
}

/// An error when `query` is a name several models share and no model's id.
/// The name alone cannot say which route to take.
fn ambiguous_name(models: &ModelState, query: &str) -> Option<String> {
    if models
        .available
        .keys()
        .any(|id| id.0.as_ref().eq_ignore_ascii_case(query))
    {
        return None;
    }
    let group = models_named(models, query);
    if group.len() < 2 {
        return None;
    }
    let ids: Vec<&str> = group.iter().map(|id| id.0.as_ref()).collect();
    Some(format!(
        "Several providers serve '{query}'. Pick one: {}",
        ids.join(", ")
    ))
}

/// The models behind a shared name, and the name's length, when `args_query`
/// is `"<shared name> ..."`. Longest name first, so a name that is a prefix of
/// another does not win.
fn detect_route_phase<'a>(
    models: &'a ModelState,
    args_query: &str,
) -> Option<(Vec<&'a acp::ModelId>, usize)> {
    let mut names: Vec<&str> = models
        .available
        .iter()
        .filter(|(id, _)| name_is_shared(models, id))
        .map(|(_, info)| info.name.as_str())
        .collect();
    names.sort_by_key(|name| std::cmp::Reverse(name.len()));
    names
        .into_iter()
        .find(|name| leads_with(args_query, name))
        .map(|name| (models_named(models, name), name.len()))
}

/// Trailing space on reasoning models: it signals "more input expected" to
/// the prompt widget, so Enter advances to the effort phase instead of
/// submitting.
fn chained_insert(token: &str, info: &acp::ModelInfo) -> String {
    if supports_reasoning_effort(info) {
        format!("{token} ")
    } else {
        token.to_owned()
    }
}

/// The row description: the model's own, else where it routes.
fn row_description(info: &acp::ModelInfo) -> String {
    match info.description.as_deref() {
        Some(d) if !d.trim().is_empty() => d.to_owned(),
        _ => route_label(info)
            .map(|route| format!("via {route}"))
            .unwrap_or_default(),
    }
}

/// One row per route for the `/model` route phase. A route that differs from
/// the others only in its id shows its id.
fn build_route_items(models: &ModelState, group: &[&acp::ModelId]) -> Vec<ArgItem> {
    let labels: Vec<String> = group
        .iter()
        .map(|id| {
            models
                .available
                .get(*id)
                .and_then(route_label)
                .unwrap_or_else(|| id.0.to_string())
        })
        .collect();
    let mut items = Vec::with_capacity(group.len());
    for (idx, id) in group.iter().enumerate() {
        let Some(info) = models.available.get(*id) else {
            continue;
        };
        let shared_label = labels.iter().filter(|l| **l == labels[idx]).count() > 1;
        let mut display = if shared_label {
            id.0.to_string()
        } else {
            labels[idx].clone()
        };
        if models.current.as_ref() == Some(*id) {
            display.push_str(" (current)");
        }
        items.push(ArgItem {
            display,
            match_text: format!("{} {}", labels[idx], id.0),
            insert_text: chained_insert(id.0.as_ref(), info),
            description: info.description.clone().unwrap_or_default(),
            loaded_in_vram: loaded_in_vram_meta(info.meta.as_ref()),
        });
    }
    items
}

/// One row per model name. Reasoning models get a trailing space in
/// `insert_text` so the prompt widget chains into the effort sub-menu.
///
/// A name that several models share is one row. It inserts the name and a
/// space, which opens the route phase: one row per provider serving it.
///
/// `favorites_only` narrows the list to the models the config marked, plus the
/// current one — a picker that hides what the session is running reads as a
/// model that went missing. A catalog with no favorite in it lists everything,
/// so an unconfigured session sees the whole catalog as before.
fn build_model_items(models: &ModelState, favorites_only: bool) -> Vec<ArgItem> {
    let current_id = models.current.as_ref();
    let narrow = favorites_only && models.available.values().any(is_favorite);
    let mut items: Vec<ArgItem> = Vec::with_capacity(models.available.len());
    let mut grouped: Vec<String> = Vec::new();
    for (id, info) in &models.available {
        if name_is_shared(models, id) {
            let key = info.name.to_lowercase();
            if grouped.contains(&key) {
                continue;
            }
            let group = models_named(models, &info.name);
            let infos: Vec<&acp::ModelInfo> = group
                .iter()
                .filter_map(|id| models.available.get(*id))
                .collect();
            let has_current = group.iter().any(|g| current_id == Some(*g));
            if narrow && !has_current && !infos.iter().any(|i| is_favorite(i)) {
                continue;
            }
            grouped.push(key);
            let routes: Vec<String> = group
                .iter()
                .zip(&infos)
                .map(|(id, i)| {
                    provider_meta(i.meta.as_ref())
                        .or_else(|| endpoint_meta(i.meta.as_ref()))
                        .unwrap_or(id.0.as_ref())
                        .to_owned()
                })
                .collect();
            let loaded = infos
                .iter()
                .filter_map(|i| loaded_in_vram_meta(i.meta.as_ref()))
                .reduce(|a, b| a || b);
            items.push(ArgItem {
                display: if has_current {
                    format!("{} (current)", info.name)
                } else {
                    info.name.clone()
                },
                match_text: info.name.clone(),
                insert_text: format!("{} ", info.name),
                description: format!("{} providers: {}", group.len(), routes.join(", ")),
                loaded_in_vram: loaded,
            });
            continue;
        }

        let is_current = current_id == Some(id);
        if narrow && !is_current && !is_favorite(info) {
            continue;
        }
        let mut display = info.name.clone();
        if is_current {
            display.push_str(" (current)");
        }

        let token = pick_token(models, id);
        items.push(ArgItem {
            display,
            insert_text: chained_insert(&token, info),
            match_text: token,
            description: row_description(info),
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
    fn a_shared_name_is_one_row_that_opens_the_route_phase() {
        let state = state_with_a_shared_name();
        let items = ModelCommand.search_args(&ctx_for(&state), "").unwrap();
        let displays: Vec<&str> = items.iter().map(|i| i.display.as_str()).collect();
        assert_eq!(displays, vec!["Grok 4.7", "grok-4.7"]);
        assert_eq!(
            items[1].insert_text, "grok-4.7 ",
            "the trailing space chains into the route phase"
        );
        assert_eq!(items[1].description, "2 providers: local, relay");
    }

    #[test]
    fn the_route_phase_lists_each_provider() {
        let state = state_with_a_shared_name();
        let items = ModelCommand
            .suggest_args(&ctx_for(&state), "grok-4.7 ")
            .unwrap();
        let displays: Vec<&str> = items.iter().map(|i| i.display.as_str()).collect();
        assert_eq!(
            displays,
            vec!["local (localhost:18080)", "relay (relay.example)"]
        );
    }

    #[test]
    fn a_shared_name_alone_is_refused_with_the_choices() {
        let mut state = ModelState::default();
        for (id, info) in [
            routed_model("local/m", "m", Some("local"), "localhost:18080"),
            routed_model("relay/m", "m", Some("relay"), "relay.example"),
        ] {
            state.available.insert(id, info);
        }
        let mut ctx = dummy_exec_ctx(&state);
        match ModelCommand.run(&mut ctx, "m") {
            CommandResult::Error(msg) => {
                assert!(msg.contains("local/m") && msg.contains("relay/m"), "{msg}");
            }
            other => panic!("expected an ambiguity error, got {other:?}"),
        }
    }

    #[test]
    fn the_group_row_opens_routes_even_when_its_name_is_a_reasoning_models_id() {
        let mut state = ModelState::default();
        let (bid, binfo) = model_with_reasoning("grok-4.7", "Grok 4.7");
        state.available.insert(bid, binfo);
        for (id, info) in [
            routed_model("local/grok-4.7", "grok-4.7", Some("local"), "l"),
            routed_model("relay/grok-4.7", "grok-4.7", Some("relay"), "r"),
        ] {
            state.available.insert(id, info);
        }
        let items = ModelCommand
            .suggest_args(&ctx_for(&state), "grok-4.7 ")
            .unwrap();
        let inserts: Vec<&str> = items.iter().map(|i| i.insert_text.as_str()).collect();
        assert_eq!(inserts, vec!["local/grok-4.7", "relay/grok-4.7"]);
    }

    #[test]
    fn an_id_that_is_also_a_shared_name_selects_the_id() {
        // The report's catalog: `grok-4.7` is the built-in's id and the
        // listed copies' shared name. The id is exact, so it wins.
        let state = state_with_a_shared_name();
        let mut ctx = dummy_exec_ctx(&state);
        match ModelCommand.run(&mut ctx, "grok-4.7") {
            CommandResult::Action(Action::SetDefaultModel(id)) => {
                assert_eq!(id.0.as_ref(), "grok-4.7");
            }
            other => panic!("expected the built-in by id, got {other:?}"),
        }
    }

    #[test]
    fn two_routes_with_the_same_label_show_their_ids() {
        let mut state = ModelState::default();
        for (id, info) in [
            routed_model("a/m", "m", None, "same.host"),
            routed_model("b/m", "m", None, "same.host"),
        ] {
            state.available.insert(id, info);
        }
        let items = ModelCommand.suggest_args(&ctx_for(&state), "m ").unwrap();
        let displays: Vec<&str> = items.iter().map(|i| i.display.as_str()).collect();
        assert_eq!(displays, vec!["a/m", "b/m"]);
    }

    #[test]
    fn each_route_row_selects_its_own_model() {
        let state = state_with_a_shared_name();
        let items = ModelCommand
            .suggest_args(&ctx_for(&state), "grok-4.7 ")
            .unwrap();
        for (item, expected) in items.iter().zip(["local/grok-4.7", "relay/grok-4.7"]) {
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

        let routes = ModelCommand.suggest_args(&ctx_for(&state), "R ").unwrap();
        assert_eq!(
            routes[1].insert_text, "relay/r ",
            "a reasoning route chains on into effort"
        );
        let items = ModelCommand
            .suggest_args(&ctx_for(&state), &routes[1].insert_text)
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
