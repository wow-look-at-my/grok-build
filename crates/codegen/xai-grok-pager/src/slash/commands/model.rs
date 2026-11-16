//! `/model` (alias `/m`) switches the model and optionally its context window and reasoning effort.
//! A trailing space after the picked model re-opens the dropdown into a window sub-menu when the model offers several, then the effort sub-menu.

use agent_client_protocol as acp;
use xai_grok_shell::sampling::types::{
    ReasoningEffortOption, endpoint_meta, favorite_meta, loaded_in_vram_meta, provider_meta,
    supports_reasoning_effort_meta,
};

use super::model_order::{is_batch_model, newest_first};

use crate::acp::model_state::ModelState;
use crate::app::actions::{Action, ModelChoice};
use crate::slash::command::{
    AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand, slash_meta,
};
use crate::slash::commands::context_window::{
    format_window, parse_window_token, supported_window, unknown_window_error, window_arg_items,
};
use crate::slash::commands::effort_levels::build_effort_arg_items;

/// Switch the active model (and optionally its reasoning effort).
pub struct ModelCommand;

impl SlashCommand for ModelCommand {
    slash_meta! {
        name: "model",
        aliases: ["m"],
        description: "Switch the active model",
        usage: "/model <name> [window] [effort]",
        takes_args: true,
        args_required: true,
        session_scoped: true,
        // The dashboard offers `/model` to pick the model for the next spawned agent (intercepted in `dispatch_dashboard_dispatch_slash`).
        offered_when_session_less: true,
        arg_placeholder: "<model> [window] [effort]",
    }

    fn suggest_args(&self, ctx: &AppCtx, args_query: &str) -> Option<Vec<ArgItem>> {
        if ctx.models.is_empty() {
            return None;
        }

        match sub_phase(ctx.models, args_query) {
            Some(SubPhase::Rows(items)) => Some(items),
            Some(SubPhase::Done) => None,
            None => Some(build_model_items(ctx.models)),
        }
    }

    fn search_args(&self, ctx: &AppCtx, args_query: &str) -> Option<Vec<ArgItem>> {
        if ctx.models.is_empty() {
            return None;
        }
        match sub_phase(ctx.models, args_query) {
            Some(SubPhase::Rows(items)) => Some(items),
            Some(SubPhase::Done) | None => Some(build_model_items(ctx.models)),
        }
    }

    fn keeps_arg_order_on_ties(&self) -> bool {
        true
    }

    fn preselected_arg(&self, ctx: &AppCtx, args_query: &str) -> Option<String> {
        // A typed filter leaves the opening row to the match ranking
        let fresh = |prefix: &str| args_query.trim_end().eq_ignore_ascii_case(prefix);
        match chained_phase(ctx.models, args_query)? {
            ChainedPhase::Window { model_id, prefix } if fresh(&prefix) => {
                let window = ctx.models.window_after_switch_to(&model_id)?;
                Some(window_insert_text(ctx.models, &model_id, &prefix, window))
            }
            ChainedPhase::Effort { model_id, prefix } if fresh(&prefix) => {
                let option = ctx.models.preselected_effort_option_for(&model_id)?;
                Some(effort_insert_text(&prefix, &option))
            }
            ChainedPhase::Window { .. } | ChainedPhase::Effort { .. } | ChainedPhase::Done => None,
        }
    }

    fn run(&self, ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let trimmed = args.trim();
        if trimmed.is_empty() {
            return CommandResult::Error("Usage: /model <name> [window] [effort]".into());
        }

        // Prefer an exact full-string catalog match first.
        if let Some(message) = ambiguous_name(ctx.models, trimmed) {
            return CommandResult::Error(message);
        }
        if let Some(id) = ctx.models.resolve_by_name_or_id(trimmed) {
            return CommandResult::Action(Action::SetDefaultModel(id));
        }

        // `/model <name> <provider>` picks one of several providers serving a
        // name. It sits before the effort phase, so a name the catalog shares
        // reaches the provider split instead of the ambiguity error.
        if let Some((name, provider)) = trimmed.rsplit_once(char::is_whitespace) {
            let (name, provider) = (name.trim_end(), provider.trim());
            if !name.is_empty()
                && ctx.models.route_label_for(provider).is_some()
                && let Some(id) = ctx
                    .models
                    .resolve_by_name_or_id_with_provider(name, provider)
            {
                return CommandResult::Action(Action::SetDefaultModel(id));
            }
        }

        // A name several models share needs a provider before a window or effort.
        if let Some((prefix, _)) = trimmed.rsplit_once(char::is_whitespace)
            && let Some(message) = ambiguous_name(ctx.models, prefix.trim_end())
        {
            return CommandResult::Error(message);
        }
        // A trailing window or effort after the model is a session switch
        if let Some((id, rest)) = split_model_rest(ctx.models, trimmed) {
            let windows = ctx.models.context_window_options_for(&id);
            let (window, effort_token) = match split_window_effort(rest, &windows) {
                Ok(split) => split,
                Err(message) => return CommandResult::Error(message),
            };

            // A window alone keeps the current model's effort, like `/context-window`
            if let Some(window) = window
                && effort_token.is_empty()
            {
                let effort = ctx
                    .models
                    .reasoning_effort
                    .filter(|_| ctx.models.current.as_ref() == Some(&id));
                return CommandResult::Action(Action::SwitchModel(ModelChoice {
                    model_id: id,
                    effort,
                    context_window_selection: Some(window),
                }));
            }
            return match ctx.models.resolve_effort_for_model(&id, effort_token) {
                Ok(effort) => CommandResult::Action(Action::SwitchModel(ModelChoice {
                    model_id: id,
                    effort: Some(effort),
                    context_window_selection: window,
                })),
                Err(err) => CommandResult::Error(err.message()),
            };
        }

        CommandResult::Error(format!("Unknown model: {trimmed}"))
    }
}

fn supports_reasoning_effort(info: &acp::ModelInfo) -> bool {
    supports_reasoning_effort_meta(info.meta.as_ref())
}

/// The arg picker title for the `/model` sub-menu that `args_query` has reached.
pub(crate) fn picker_title(models: &ModelState, args_query: &str) -> &'static str {
    if winning_route(models, args_query).is_some() {
        return "Pick provider";
    }
    match chained_phase(models, args_query) {
        Some(ChainedPhase::Window { .. }) => "Pick context window",
        Some(ChainedPhase::Effort { .. }) => "Pick reasoning effort",
        Some(ChainedPhase::Done) | None => "Pick model",
    }
}

/// Which chained sub-menu the typed args have reached.
enum ChainedPhase {
    Window {
        model_id: acp::ModelId,
        prefix: String,
    },
    Effort {
        model_id: acp::ModelId,
        prefix: String,
    },
    Done,
}

/// The sub-menu for `args_query`, or `None` while the user is still picking a model.
fn chained_phase(models: &ModelState, args_query: &str) -> Option<ChainedPhase> {
    let (model_id, len) = detect_chained_prefix(models, args_query)?;
    let (key, rest) = args_query.split_at(len);
    let windows = models.context_window_options_for(&model_id);
    let reasoning = models
        .available
        .get(&model_id)
        .is_some_and(supports_reasoning_effort);

    let phase = match committed_first_token(rest) {
        // A committed window advances the chain to the effort phase
        Some(first)
            if parse_window_token(first).is_some_and(|window| windows.contains(&window)) =>
        {
            if reasoning {
                ChainedPhase::Effort {
                    model_id,
                    prefix: format!("{key} {first}"),
                }
            } else {
                ChainedPhase::Done
            }
        }
        // Any other committed token is a typed effort
        Some(_) if reasoning => ChainedPhase::Effort {
            model_id,
            prefix: key.to_string(),
        },
        // A longer model name may still match ("Foo Bar" after "Foo")
        Some(_) => return None,
        None if windows.len() > 1 => {
            // A partial token that can't start any window row ("Grok 4.7 hi") is a typed effort
            if starts_a_window(&windows, rest.trim_start()) {
                ChainedPhase::Window {
                    model_id,
                    prefix: key.to_string(),
                }
            } else if reasoning {
                ChainedPhase::Effort {
                    model_id,
                    prefix: key.to_string(),
                }
            } else {
                return None;
            }
        }
        None if reasoning => ChainedPhase::Effort {
            model_id,
            prefix: key.to_string(),
        },
        None => ChainedPhase::Done,
    };
    Some(phase)
}

/// The first token is committed once whitespace follows it.
fn committed_first_token(rest: &str) -> Option<&str> {
    let rest_trimmed = rest.trim_start();
    match rest_trimmed.split_once(char::is_whitespace) {
        Some((first, _)) => Some(first),
        None if !rest_trimmed.is_empty() && rest.ends_with(char::is_whitespace) => {
            Some(rest_trimmed)
        }
        None => None,
    }
}

/// Whether a partial token can still grow into one of the window rows (an empty partial always can).
fn starts_a_window(windows: &[u64], partial: &str) -> bool {
    let partial = partial.to_ascii_lowercase();
    windows.iter().any(|&window| {
        format_window(window).starts_with(&partial) || window.to_string().starts_with(&partial)
    })
}

/// Whether `query` is `token`, then whitespace, then anything.
fn leads_with(query: &str, token: &str) -> bool {
    query.len() > token.len() && query.is_char_boundary(token.len()) && {
        // The `is_char_boundary` clause above runs first and short-circuits.
        let (head, tail) = query.split_at(token.len());
        head.eq_ignore_ascii_case(token) && tail.starts_with(char::is_whitespace)
    }
}

/// The rows of the sub-menu that the typed args have reached.
enum SubPhase {
    Rows(Vec<ArgItem>),
    /// The args already name everything the model takes.
    Done,
}

/// The route, window or effort sub-menu `args_query` leads into, or `None` while the user is still picking a model.
fn sub_phase(models: &ModelState, args_query: &str) -> Option<SubPhase> {
    if let Some(group) = winning_route(models, args_query) {
        return Some(SubPhase::Rows(build_route_items(models, &group)));
    }
    Some(match chained_phase(models, args_query)? {
        ChainedPhase::Window { model_id, prefix } => {
            SubPhase::Rows(build_window_items(models, &model_id, &prefix))
        }
        ChainedPhase::Effort { model_id, prefix } => {
            SubPhase::Rows(build_effort_items(models, &model_id, &prefix))
        }
        ChainedPhase::Done => SubPhase::Done,
    })
}

/// The models behind the shared name `args_query` leads with, when the route phase beats the chained sub-menu.
/// The longer typed token wins. On a tie the route phase wins: a shared name can equal another model's id, and the group row inserts that name.
fn winning_route<'a>(models: &'a ModelState, args_query: &str) -> Option<Vec<&'a acp::ModelId>> {
    let (group, route_len) = detect_route_phase(models, args_query)?;
    match detect_chained_prefix(models, args_query) {
        Some((_, chained_len)) if chained_len > route_len => None,
        _ => Some(group),
    }
}

/// Whether picking this model chains into a sub-menu (windows or effort).
fn has_chained_args(models: &ModelState, id: &acp::ModelId, info: &acp::ModelInfo) -> bool {
    supports_reasoning_effort(info) || models.context_window_options_for(id).len() > 1
}

/// The model and the matched length when `args_query` is `"<model with a sub-menu> ..."`.
/// Longest-name-first to disambiguate names that share a prefix.
fn detect_chained_prefix(models: &ModelState, args_query: &str) -> Option<(acp::ModelId, usize)> {
    // A model is typed by its id, or by its name when no other model has it. A shared name leads to the route phase instead.
    let mut candidates: Vec<(&acp::ModelId, &str)> = models
        .available
        .iter()
        .filter(|(id, info)| has_chained_args(models, id, info))
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

<<<<<<< fork
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
||||||| base
fn split_model_effort<'a>(
    models: &'a ModelState,
    args: &'a str,
) -> Option<(acp::ModelId, &'a str)> {
    let (id, _, rest) = longest_reasoning_prefix(models, args)?;
    let token = rest.trim();
    if token.is_empty() {
        None
=======
fn split_model_rest<'a>(models: &'a ModelState, args: &'a str) -> Option<(acp::ModelId, &'a str)> {
    let (id, _, rest) = longest_chained_prefix(models, args)?;
    let rest = rest.trim();
    if rest.is_empty() {
        None
>>>>>>> upstream
    } else {
<<<<<<< fork
        token.to_owned()
||||||| base
        Some((id.clone(), token))
=======
        Some((id.clone(), rest))
>>>>>>> upstream
    }
}

<<<<<<< fork
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
        let Some(label) = labels.get(idx) else {
            continue;
        };
        let shared_label = labels.iter().filter(|l| *l == label).count() > 1;
        let mut display = if shared_label {
            id.0.to_string()
        } else {
            label.clone()
        };
        if models.current.as_ref() == Some(*id) {
            display.push_str(" (current)");
        }
        items.push(ArgItem {
            display,
            match_text: format!("{} {}", label, id.0),
            insert_text: chained_insert(id.0.as_ref(), info),
            description: info.description.clone().unwrap_or_default(),
            loaded_in_vram: loaded_in_vram_meta(info.meta.as_ref()),
            favorite: is_favorite(info),
        });
    }
    items
||||||| base
fn matched_reasoning_prefix(
    models: &ModelState,
    args_query: &str,
) -> Option<(acp::ModelId, String)> {
    let (id, key, _) = longest_reasoning_prefix(models, args_query)?;
    Some((id.clone(), key.to_string()))
=======
/// A window is always the first token, and the remaining words are the effort.
fn split_window_effort<'a>(
    rest: &'a str,
    windows: &[u64],
) -> Result<(Option<std::num::NonZeroU64>, &'a str), String> {
    let (first, tail) = match rest.split_once(char::is_whitespace) {
        Some((first, tail)) => (first, tail.trim()),
        None => (rest, ""),
    };
    if windows.is_empty() || parse_window_token(first).is_none() {
        return Ok((None, rest));
    }
    match supported_window(first, windows) {
        Some(window) => Ok((Some(window), tail)),
        None => Err(unknown_window_error(first, windows)),
    }
>>>>>>> upstream
}

<<<<<<< fork
/// One row per model name. Reasoning models get a trailing space in
/// `insert_text` so the prompt widget chains into the effort sub-menu. A name
/// that several models share is one row. It inserts the name and a space,
/// which opens the route phase: one row per provider serving it.
/// Every model is listed, except a batch route the session is not running.
/// The ones the config marked come first, then the newest of each family.
||||||| base
/// One row per logical model.
/// Reasoning models get a trailing space in `insert_text` so the prompt widget chains into the effort sub-menu.
=======
/// One row per logical model.
>>>>>>> upstream
fn build_model_items(models: &ModelState) -> Vec<ArgItem> {
    let current_id = models.current.as_ref();
    let mut items: Vec<ArgItem> = Vec::with_capacity(models.available.len());
    let mut grouped: Vec<String> = Vec::new();
    for (id, info) in &models.available {
<<<<<<< fork
        if is_batch_model(id.0.as_ref()) && current_id != Some(id) {
            continue;
        }
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
                favorite: infos.iter().any(|i| is_favorite(i)),
            });
            continue;
        }
||||||| base
        let is_current = current_id == Some(id);
        let supports = supports_reasoning_effort(info);

        let display = if is_current {
            format!("{} (current)", info.name)
        } else {
            info.name.clone()
        };
=======
        let is_current = current_id == Some(id);
        let chains = has_chained_args(models, id, info);

        let display = if is_current {
            format!("{} (current)", info.name)
        } else {
            info.name.clone()
        };
>>>>>>> upstream

<<<<<<< fork
        let is_current = current_id == Some(id);
        let mut display = info.name.clone();
        if is_current {
            display.push_str(" (current)");
        }
||||||| base
        // A trailing space on reasoning models signals "more input expected" to the prompt widget
        // Enter then advances to the effort phase instead of submitting
        let insert_text = if supports {
            format!("{} ", info.name)
        } else {
            info.name.clone()
        };
=======
        // The trailing space makes Enter advance to the window or effort phase instead of submitting
        let insert_text = if chains {
            format!("{} ", info.name)
        } else {
            info.name.clone()
        };
>>>>>>> upstream

        let token = pick_token(models, id);
        items.push(ArgItem {
            display,
            insert_text: chained_insert(&token, info),
            match_text: token,
            description: row_description(info),
            // Only a provider that reports residency answers this, so the dot appears beside local models and nowhere else.
            loaded_in_vram: loaded_in_vram_meta(info.meta.as_ref()),
            favorite: is_favorite(info),
        });
    }
    let plain_name = |item: &ArgItem| {
        let name = item.display.strip_suffix(" (current)");
        name.unwrap_or(&item.display).to_owned()
    };
    items.sort_by(|a, b| {
        b.favorite
            .cmp(&a.favorite)
            .then_with(|| newest_first(&plain_name(a), &plain_name(b)))
    });
    items
}

/// One row per selectable window for the `/model` chained window phase.
fn build_window_items(models: &ModelState, model_id: &acp::ModelId, prefix: &str) -> Vec<ArgItem> {
    let windows = models.context_window_options_for(model_id);
    // Only the current model has an active window
    let active = (models.current.as_ref() == Some(model_id))
        .then(|| models.get_context_window())
        .flatten();
    let default = models.model_default_window(model_id);
    window_arg_items(&windows, active, default, |_, window| {
        window_insert_text(models, model_id, prefix, window)
    })
}

fn window_insert_text(
    models: &ModelState,
    model_id: &acp::ModelId,
    prefix: &str,
    window: u64,
) -> String {
    let chains_to_effort = models
        .available
        .get(model_id)
        .is_some_and(supports_reasoning_effort);
    let label = format_window(window);
    if chains_to_effort {
        format!("{prefix} {label} ")
    } else {
        format!("{prefix} {label}")
    }
}

/// One row per effort level for the `/model` chained effort phase.
/// `prefix` is the name or catalog id the user typed. `insert_text` is `"{prefix} {effort}"`.
fn build_effort_items(models: &ModelState, model_id: &acp::ModelId, prefix: &str) -> Vec<ArgItem> {
    if !models.available.contains_key(model_id) {
        return Vec::new();
    }
    let is_current_model = models.current.as_ref() == Some(model_id);
    let options = models.reasoning_effort_options_for(model_id);
    build_effort_arg_items(
        &options,
        models.reasoning_effort,
        is_current_model,
        |option| effort_insert_text(prefix, option),
    )
}

fn effort_insert_text(prefix: &str, option: &ReasoningEffortOption) -> String {
    format!("{prefix} {}", option.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU64;
    use std::sync::Arc;
    use xai_grok_shell::sampling::types::ReasoningEffort;
    use xai_grok_test_support::acp_fixtures;

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
            saved_workflows: &[],
            workflow_runs: &[],
            current_title: None,
        }
    }

    /// Models, one of them marked, and the session is running an
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
    fn the_opening_list_is_every_model_with_favorites_first() {
        let mut state = ModelState::default();
        let (a, ainfo) = plain_model("one", "One");
        let (b, binfo) = plain_model("two", "Two");
        let (f, finfo) = favorite_model("kept", "Kept");
        state.available.insert(a, ainfo);
        state.available.insert(b, binfo);
        state.available.insert(f, finfo);

        let items = ModelCommand.suggest_args(&ctx_for(&state), "").unwrap();
        let rows: Vec<(&str, bool)> = items
            .iter()
            .map(|i| (i.match_text.as_str(), i.favorite))
            .collect();
        assert_eq!(rows, vec![("Kept", true), ("One", false), ("Two", false)]);
    }

    #[test]
    fn a_typed_query_searches_past_the_favorites() {
        let state = state_with_a_favorite();
        // The caller ranks the rows, so the command's job is to offer every model the moment anything is typed.
        let items = ModelCommand.suggest_args(&ctx_for(&state), "cro").unwrap();
        let names: Vec<&str> = items.iter().map(|i| i.match_text.as_str()).collect();
        assert_eq!(names, vec!["Kept", "Crowd One", "Running"]);
    }

    #[test]
    fn the_modal_picker_is_handed_every_model_to_search() {
        let state = state_with_a_favorite();
        // The modal picker asks one time and filters its own copy.
        let items = ModelCommand.search_args(&ctx_for(&state), "").unwrap();
        let names: Vec<&str> = items.iter().map(|i| i.match_text.as_str()).collect();
        assert_eq!(names, vec!["Kept", "Crowd One", "Running"]);
    }

    #[test]
    fn a_batch_route_is_listed_only_while_the_session_runs_it() {
        let mut state = ModelState::default();
        let (live, live_info) = plain_model("opus-5.5", "Opus 5.5");
        let (batch, batch_info) = plain_model("opus-5.5:batch", "Opus 5.5 (batch)");
        state.available.insert(live, live_info);
        state.available.insert(batch.clone(), batch_info);

        let names = |state: &ModelState| -> Vec<String> {
            let items = ModelCommand.suggest_args(&ctx_for(state), "").unwrap();
            items.into_iter().map(|i| i.display).collect()
        };
        assert_eq!(names(&state), vec!["Opus 5.5"]);

        state.current = Some(batch);
        assert_eq!(
            names(&state),
            vec!["Opus 5.5", "Opus 5.5 (batch) (current)"]
        );
    }

    #[test]
    fn newer_models_lead_the_list_and_a_tied_search() {
        let mut state = ModelState::default();
        for (id, name) in [("opus-4.1", "Opus 4.1"), ("opus-5.5", "Opus 5.5")] {
            let (id, info) = plain_model(id, name);
            state.available.insert(id, info);
        }
        let items = ModelCommand.suggest_args(&ctx_for(&state), "").unwrap();
        let opening: Vec<&str> = items.iter().map(|i| i.display.as_str()).collect();
        assert_eq!(opening, vec!["Opus 5.5", "Opus 4.1"]);

        let mut ctrl = crate::slash::SlashController::with_builtins(std::path::PathBuf::from("."));
        let slash = crate::slash::SlashState::default();
        let text = "/model opus";
        ctrl.refresh(&slash, text, text.len(), &state);
        let snapshot = slash.snapshot();
        let found: Vec<&str> = snapshot
            .matches
            .iter()
            .map(|row| row.display.as_str())
            .collect();
        assert_eq!(found, vec!["Opus 5.5", "Opus 4.1"]);
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
    /// listed by providers, with no description on the listed copies.
    fn state_with_a_shared_name() -> ModelState {
        let mut state = ModelState::default();
        for (id, info) in [
            routed_model("grok-4.7", "Grok 4.7", None, "api.example.invalid"),
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
        // The report's catalog: `grok-4.7` is the built-in's id and the listed copies' shared name.
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

    fn model_with_windows_and_reasoning(id: &str, name: &str) -> (acp::ModelId, acp::ModelInfo) {
        let info = acp_fixtures::model_info_with_meta(
            id,
            name,
            serde_json::json!({
                "supportsReasoningEffort": true,
                "totalContextTokens": 256_000,
                "contextWindows": [256_000, 500_000],
            }),
        );
        (acp_fixtures::model_id(id), info)
    }

    fn app_ctx(models: &ModelState) -> AppCtx<'_> {
        AppCtx {
            models,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            billing_surface_visible: true,
            usage_command_visible: true,
            workflows_available: true,
            saved_workflows: &[],
            workflow_runs: &[],
            screen_mode: crate::app::ScreenMode::Fullscreen,
            current_title: None,
        }
    }

    #[test]
    fn split_model_rest_keeps_a_multi_word_label() {
        let mut state = ModelState::default();
        let (id, info) = model_with_reasoning("grok-4.7", "Grok 4.7");
        state.available.insert(id.clone(), info);
        assert_eq!(
            split_model_rest(&state, "Grok 4.7 Extra High")
                .map(|(model, token)| { (model.0.to_string(), token.to_string()) }),
            Some(("grok-4.7".to_string(), "Extra High".to_string()))
        );
        assert_eq!(
            split_model_rest(&state, "Grok 4.7 high").map(|(_, token)| token),
            Some("high")
        );
        assert!(split_model_rest(&state, "Grok 4.7").is_none());
        assert_eq!(
            split_model_rest(&state, "grok-4.7 Extra High")
                .map(|(model, token)| (model.0.to_string(), token.to_string())),
            Some(("grok-4.7".to_string(), "Extra High".to_string()))
        );
    }

    #[test]
    fn window_phase_sits_between_model_and_effort() {
        let mut state = ModelState::default();
        let (id, info) = model_with_windows_and_reasoning("grok-4.7", "Grok 4.7");
        state.available.insert(id, info);
        let cmd = ModelCommand;
        let ctx = app_ctx(&state);

        // A trailing space after the model opens the window phase
        let items = cmd
            .suggest_args(&ctx, "Grok 4.7 ")
            .expect("window rows after the model");
        let [first, second] = items.as_slice() else {
            panic!("expected 2 window rows: {items:?}");
        };
        assert_eq!(first.display, "256k");
        // The trailing space chains into the effort phase
        assert_eq!(second.insert_text, "Grok 4.7 500k ");

        // A committed window advances to the effort phase with the longer prefix
        let items = cmd
            .suggest_args(&ctx, "Grok 4.7 500k ")
            .expect("effort rows after a committed window");
        assert_eq!(
            items.first().map(|item| item.insert_text.as_str()),
            Some("Grok 4.7 500k xhigh")
        );

        // A partial that can't start any window row falls back to the effort rows
        let items = cmd
            .suggest_args(&ctx, "Grok 4.7 hi")
            .expect("effort rows for a typed effort filter");
        assert!(
            items.iter().any(|item| item.insert_text == "Grok 4.7 high"),
            "typed effort must keep the effort rows up: {items:?}"
        );

        // The fresh window menu preselects the default
        assert_eq!(
            cmd.preselected_arg(&ctx, "Grok 4.7 ").as_deref(),
            Some("Grok 4.7 256k ")
        );
    }

    #[test]
    fn picker_title_follows_the_chained_phase() {
        let mut state = ModelState::default();
        let (id, info) = model_with_windows_and_reasoning("grok-4.7", "Grok 4.7");
        state.available.insert(id, info);

        assert_eq!(picker_title(&state, ""), "Pick model");
        assert_eq!(picker_title(&state, "Grok 4.7 "), "Pick context window");
        assert_eq!(
            picker_title(&state, "Grok 4.7 500k "),
            "Pick reasoning effort"
        );
    }

    #[test]
    fn a_non_window_partial_keeps_the_model_rows() {
        let mut state = ModelState::default();
        let info = acp_fixtures::model_info_with_meta(
            "foo",
            "Foo",
            serde_json::json!({ "contextWindows": [256_000, 500_000] }),
        );
        state.available.insert(acp_fixtures::model_id("foo"), info);
        let (id, info) = plain_model("foo-bar", "Foo Bar");
        state.available.insert(id, info);
        let ctx = app_ctx(&state);

        let items = ModelCommand
            .suggest_args(&ctx, "Foo B")
            .expect("model rows while the partial can't start a window");

        assert!(
            items.iter().any(|item| item.insert_text == "Foo Bar"),
            "items={items:?}"
        );
    }

    #[test]
    fn run_parses_window_and_effort_after_the_model() {
        let mut state = ModelState::default();
        let (id, info) = model_with_windows_and_reasoning("grok-4.7", "Grok 4.7");
        state.available.insert(id, info);
        let mut ctx = dummy_exec_ctx(&state);

        match ModelCommand.run(&mut ctx, "Grok 4.7 500k") {
            CommandResult::Action(Action::SwitchModel(ModelChoice {
                model_id,
                effort,
                context_window_selection,
            })) => {
                assert_eq!(model_id.0.as_ref(), "grok-4.7");
                assert_eq!(
                    effort, None,
                    "a window alone on another model sends no effort"
                );
                assert_eq!(context_window_selection, NonZeroU64::new(500_000));
            }
            other => panic!("expected window-only switch, got {other:?}"),
        }

        match ModelCommand.run(&mut ctx, "Grok 4.7 500k high") {
            CommandResult::Action(Action::SwitchModel(ModelChoice {
                model_id,
                effort,
                context_window_selection,
            })) => {
                assert_eq!(model_id.0.as_ref(), "grok-4.7");
                assert_eq!(effort, Some(ReasoningEffort::High));
                assert_eq!(context_window_selection, NonZeroU64::new(500_000));
            }
            other => panic!("expected window+effort switch, got {other:?}"),
        }

        // An effort with no window leaves the window unset
        match ModelCommand.run(&mut ctx, "Grok 4.7 high") {
            CommandResult::Action(Action::SwitchModel(ModelChoice {
                effort,
                context_window_selection,
                ..
            })) => {
                assert_eq!(effort, Some(ReasoningEffort::High));
                assert_eq!(context_window_selection, None);
            }
            other => panic!("expected effort-only switch, got {other:?}"),
        }

        // A parseable but unoffered window gets the window error with the offered list
        match ModelCommand.run(&mut ctx, "Grok 4.7 1m") {
            CommandResult::Error(msg) => {
                assert!(msg.contains("unknown context window '1m'"), "msg={msg}");
                assert!(msg.contains("256k, 500k"), "msg={msg}");
            }
            other => panic!("expected window error, got {other:?}"),
        }
    }

    #[test]
    fn window_only_pick_keeps_the_current_models_effort() {
        let mut state = ModelState::default();
        let (id, info) = model_with_windows_and_reasoning("grok-4.7", "Grok 4.7");
        state.available.insert(id.clone(), info);
        state.current = Some(id);
        state.reasoning_effort = Some(ReasoningEffort::Low);
        let mut ctx = dummy_exec_ctx(&state);

        let result = ModelCommand.run(&mut ctx, "Grok 4.7 500k");

        assert!(
            matches!(
                result,
                CommandResult::Action(Action::SwitchModel(ModelChoice {
                    effort: Some(ReasoningEffort::Low),
                    ..
                }))
            ),
            "got {result:?}"
        );
    }

    #[test]
    fn picker_preselects_the_window_the_switch_uses() {
        let mut state = ModelState::default();
        let (current, current_info) = model_with_windows_and_reasoning("grok-4.7", "Grok 4.7");
        let (listed, listed_info) = model_with_windows_and_reasoning("grok-4.8", "Grok 4.8");
        let unlisted_info = acp_fixtures::model_info_with_meta(
            "grok-4.5",
            "Grok 4.5",
            serde_json::json!({
                "supportsReasoningEffort": true,
                "totalContextTokens": 256_000,
                "contextWindows": [128_000, 256_000],
            }),
        );
        state.available.insert(current.clone(), current_info);
        state.available.insert(listed, listed_info);
        state
            .available
            .insert(acp_fixtures::model_id("grok-4.5"), unlisted_info);
        state.current = Some(current);
        state.context_window_selection = Some(500_000);
        let ctx = app_ctx(&state);

        let listed_row = ModelCommand.preselected_arg(&ctx, "Grok 4.8 ");
        let unlisted_row = ModelCommand.preselected_arg(&ctx, "Grok 4.5 ");

        assert_eq!(listed_row.as_deref(), Some("Grok 4.8 500k "));
        assert_eq!(unlisted_row.as_deref(), Some("Grok 4.5 256k "));
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
            saved_workflows: &[],
            workflow_runs: &[],
            current_title: None,
        };
        let items = cmd.suggest_args(&ctx, "").unwrap();
        assert_eq!(items.len(), 2, "model phase: one row per logical model");

        // A reasoning model has a trailing space in insert_text
        // The prompt widget reads it to keep the dropdown open after Enter so the effort sub-menu can render
        let reasoning = items
            .iter()
            .find(|i| i.match_text == "Reasoning X")
            .unwrap();
        assert_eq!(reasoning.insert_text, "Reasoning X ");

        // A plain model has no trailing space, so Enter commits immediately
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
            saved_workflows: &[],
            workflow_runs: &[],
            current_title: None,
        };
        // The args query has a trailing space, so this is the effort phase
        // Items come out ordered xhigh to low (strongest first) per EFFORT_LEVELS
        let items = cmd.suggest_args(&ctx, "Reasoning X ").unwrap();
        assert_eq!(items.len(), 4);
        let [a, b, c, d] = items.as_slice() else {
            panic!("expected 4 items: {items:?}");
        };
        assert_eq!(a.insert_text, "Reasoning X xhigh");
        assert_eq!(b.insert_text, "Reasoning X high");
        assert_eq!(c.insert_text, "Reasoning X medium");
        assert_eq!(d.insert_text, "Reasoning X low");
        // Display is just the level so the user sees a clean column.
        assert_eq!(a.display, "xhigh");
        // match_text carries the sort-key prefix that forces the matcher's alphabetical tiebreak to render rows in EFFORT_LEVELS order
        assert!(a.match_text.starts_with("a "));
        assert!(d.match_text.starts_with("d "));
    }

    #[test]
    fn preselected_arg_targets_default_row_only_for_fresh_effort_menu() {
        let mut state = ModelState::default();
        let id = acp::ModelId::new(Arc::from("reasoning-x"));
        let info = acp::ModelInfo::new(id.clone(), "Reasoning X").meta(
            serde_json::json!({ "supportsReasoningEffort": true, "reasoningEffort": "high" })
                .as_object()
                .cloned(),
        );
        state.available.insert(id, info);

        let cmd = ModelCommand;
        let ctx = AppCtx {
            models: &state,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            billing_surface_visible: true,
            usage_command_visible: true,
            workflows_available: true,
            saved_workflows: &[],
            workflow_runs: &[],
            current_title: None,
        };
        // The preselection must name a row `suggest_args` actually builds, or the consumers fall back to row 0
        let high_row = cmd
            .suggest_args(&ctx, "Reasoning X ")
            .and_then(|items| items.get(1).map(|item| item.insert_text.clone()));
        assert_eq!(Some("Reasoning X high".to_owned()), high_row);
        assert_eq!(high_row, cmd.preselected_arg(&ctx, "Reasoning X "));
        assert_eq!(None, cmd.preselected_arg(&ctx, "Reasoning X h"));
        assert_eq!(None, cmd.preselected_arg(&ctx, ""));
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
            saved_workflows: &[],
            workflow_runs: &[],
            current_title: None,
        };
        // Still in effort phase; the matcher upstream narrows to high and xhigh
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
            saved_workflows: &[],
            workflow_runs: &[],
            current_title: None,
        };
        // No trailing space: the user is still typing the model name
        let items = cmd.suggest_args(&ctx, "Reason").unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(
            items.first().map(|item| item.insert_text.as_str()),
            Some("Reasoning X ")
        );
    }

    #[test]
    fn run_parses_model_plus_effort_when_supported() {
        let mut state = ModelState::default();
        let (id, info) = model_with_reasoning("reasoning-x", "Reasoning X");
        state.available.insert(id, info);
        let mut ctx = dummy_exec_ctx(&state);
        let result = ModelCommand.run(&mut ctx, "Reasoning X xhigh");
        match result {
            CommandResult::Action(Action::SwitchModel(ModelChoice {
                model_id, effort, ..
            })) => {
                assert_eq!(model_id.0.as_ref(), "reasoning-x");
                assert_eq!(effort, Some(ReasoningEffort::Xhigh));
            }
            other => panic!("expected SwitchModel with effort, got {other:?}"),
        }
    }

    #[test]
    fn run_selects_a_provider_for_a_name_several_serve() {
        // `/model <name> <provider>` picks the model a named provider serves.
        let mut state = ModelState::default();
        for (id, info) in [
            routed_model("local/m", "m", Some("local"), "localhost:18080"),
            routed_model("relay/m", "m", Some("relay"), "relay.example"),
        ] {
            state.available.insert(id, info);
        }
        let mut ctx = dummy_exec_ctx(&state);
        match ModelCommand.run(&mut ctx, "m relay") {
            CommandResult::Action(Action::SetDefaultModel(id)) => {
                assert_eq!(id.0.as_ref(), "relay/m");
            }
            other => panic!("expected the relay route, got {other:?}"),
        }
    }

    #[test]
    fn run_prefers_a_full_name_over_a_provider_split() {
        // "M Local" is a model's own name and "local" is also a provider, but the full name resolves.
        let mut state = ModelState::default();
        let (full, full_info) = plain_model("m-local", "M Local");
        state.available.insert(full.clone(), full_info);
        state.available.insert(
            acp::ModelId::new(Arc::from("local/other")),
            acp::ModelInfo::new(
                acp::ModelId::new(Arc::from("local/other")),
                "Other".to_string(),
            )
            .meta(Some(
                serde_json::json!({ "provider": "local" })
                    .as_object()
                    .cloned()
                    .unwrap(),
            )),
        );
        let mut ctx = dummy_exec_ctx(&state);
        match ModelCommand.run(&mut ctx, "M Local") {
            CommandResult::Action(Action::SetDefaultModel(id)) => {
                assert_eq!(id.0.as_ref(), "m-local");
            }
            other => panic!("expected SetDefaultModel(m-local), got {other:?}"),
        }
    }

    #[test]
    fn run_rejects_unoffered_effort_with_effort_error_not_unknown_model() {
        // Regression: previously `resolve_effort_token_for` returned None and the handler fell through to `Unknown model: Reasoning X none`
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
        // The catalog has both "Grok" (reasoning) and "Grok 4.5"
        // `/model Grok 4.5` must select the full name, not treat "4.5" as an effort on "Grok"
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
        // Falls through to "is the whole string a model name?", which it isn't, so we get an Unknown error
        assert!(matches!(result, CommandResult::Error(_)));
    }

    /// The bare `/model <name>` form dispatches `Action::SetDefaultModel(<ModelId>)` instead of the legacy `Action::SwitchModel { effort: None }`.
    /// The dispatcher routes it through both `Effect::SwitchModel` (session mutation) and `Effect::PersistSetting` (next-session default).
    /// The payload is the typed `acp::ModelId` (resolved at the slash boundary), not a String.
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

    /// Case-insensitive matching against the catalog: `/model grok 4.5` resolves to the same `ModelId` as `/model Grok 4.5`.
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
