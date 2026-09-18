//! The harness model slots: every place the harness picks a model.
//!
//! A slot is one job the harness sends to a model. Each slot has a
//! `[models]` key in `config.toml`, an environment variable, and a row in
//! the settings modal. Nothing in the harness may choose a model that is
//! not a slot here. A new model call adds a slot in the same change.
//!
//! An unset slot INHERITS the session model, except where
//! [`ModelSlot::compiled_default`] names one. A slot that inherits costs
//! nothing and follows a `/model` switch.

/// One model-choosing job in the harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelSlot {
	/// Stable id. It is the `[models]` key, the settings-modal key suffix,
	/// and the environment variable's lowercase tail.
	pub id: &'static str,
	/// Settings-modal row label.
	pub label: &'static str,
	/// Settings-modal row description, and the docs one-liner.
	pub description: &'static str,
	/// Environment variable that overrides every other source.
	pub env: &'static str,
	/// Settings-modal search keywords, all lowercase.
	pub keywords: &'static [&'static str],
	/// What an unset slot falls back to.
	pub fallback: SlotFallback,
}

/// What a slot resolves to when nothing sets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotFallback {
	/// The session's own model, whatever `/model` last selected.
	SessionModel,
	/// A compiled default from `default_models.json`.
	Compiled,
	/// A default the consumer owns, with its own guards. The slot resolves
	/// to nothing and the consumer's existing fallback runs. Used where the
	/// default is not simply a model id — `prompt_suggestion` picks between
	/// a client hint and a built-in, and drops the call when the model is
	/// outside the catalog.
	ConsumerDefault,
}

impl ModelSlot {
	/// The settings-modal registry key for this slot.
	///
	/// The key is a `&'static str` because the registry stores metadata
	/// without allocating. [`slot_setting_keys`] holds the one static
	/// string per slot; this looks it up by id.
	pub fn setting_key(&self) -> &'static str {
		slot_setting_keys()
			.iter()
			.find(|(id, _)| *id == self.id)
			.map(|(_, key)| *key)
			.expect("every slot has a setting key")
	}

	/// The compiled default for this slot, or `None` when the slot
	/// inherits the session model.
	pub fn compiled_default(&self) -> Option<&'static str> {
		match self.fallback {
			SlotFallback::SessionModel | SlotFallback::ConsumerDefault => None,
			SlotFallback::Compiled => Some(match self.id {
				"web_search" => crate::default_web_search_model(),
				"image_description" => crate::default_image_description_model(),
				"session_summary" => crate::default_session_summary_model(),
				_ => crate::default_model(),
			}),
		}
	}
}

/// Every model-choosing job in the harness, in settings-modal order.
pub const HARNESS_MODEL_SLOTS: &[ModelSlot] = &[
	ModelSlot {
		id: "web_search",
		label: "Web search model",
		description: "Model that synthesizes web-search results.",
		env: "GROK_MODEL_WEB_SEARCH",
		keywords: &["web", "search", "model", "synthesis", "browse"],
		fallback: SlotFallback::Compiled,
	},
	ModelSlot {
		id: "image_description",
		label: "Image description model",
		description: "Vision model that transcribes images you paste or attach.",
		env: "GROK_MODEL_IMAGE_DESCRIPTION",
		keywords: &["image", "vision", "describe", "picture", "screenshot", "model"],
		fallback: SlotFallback::Compiled,
	},
	ModelSlot {
		id: "session_summary",
		label: "Session title model",
		description: "Model that names the session in the session list.",
		env: "GROK_MODEL_SESSION_SUMMARY",
		keywords: &["session", "summary", "title", "name", "model"],
		fallback: SlotFallback::Compiled,
	},
	ModelSlot {
		id: "prompt_suggestion",
		label: "Prompt suggestion model",
		description: "Model behind the tab-autocomplete ghost text. Keep it small and fast.",
		env: "GROK_PROMPT_SUGGESTIONS_MODEL",
		keywords: &["prompt", "suggestion", "autocomplete", "ghost", "tab", "model"],
		fallback: SlotFallback::ConsumerDefault,
	},
	ModelSlot {
		id: "permission_classifier",
		label: "Permission classifier model",
		description: "Model that approves or blocks tool calls in Auto permission mode.",
		env: "GROK_MODEL_PERMISSION_CLASSIFIER",
		keywords: &["permission", "classifier", "auto", "approve", "block", "model"],
		fallback: SlotFallback::SessionModel,
	},
	ModelSlot {
		id: "laziness_classifier",
		label: "Laziness classifier model",
		description: "Model that judges whether an idle turn stopped short of the task.",
		env: "GROK_MODEL_LAZINESS_CLASSIFIER",
		keywords: &["laziness", "classifier", "idle", "stop", "model"],
		fallback: SlotFallback::SessionModel,
	},
	ModelSlot {
		id: "compaction",
		label: "Compaction model",
		description: "Model that summarizes the conversation when the context window fills.",
		env: "GROK_MODEL_COMPACTION",
		keywords: &["compaction", "compact", "summarize", "context", "window", "model"],
		fallback: SlotFallback::SessionModel,
	},
	ModelSlot {
		id: "recap",
		label: "Recap model",
		description: "Model behind the \"where was I\" session recap.",
		env: "GROK_MODEL_RECAP",
		keywords: &["recap", "resume", "summary", "model"],
		fallback: SlotFallback::SessionModel,
	},
	ModelSlot {
		id: "turn_summary",
		label: "Turn summary model",
		description: "Model that writes the one-line summary of each finished turn.",
		env: "GROK_MODEL_TURN_SUMMARY",
		keywords: &["turn", "summary", "one-line", "model"],
		fallback: SlotFallback::SessionModel,
	},
	ModelSlot {
		id: "side_note",
		label: "Side note model (/btw)",
		description: "Model that handles a `/btw` side note without interrupting the turn.",
		env: "GROK_MODEL_SIDE_NOTE",
		keywords: &["btw", "side", "note", "aside", "model"],
		fallback: SlotFallback::SessionModel,
	},
	ModelSlot {
		id: "todo_capture",
		label: "Todo capture model (/todo)",
		description: "Model that turns a `/todo` request into todo-list items.",
		env: "GROK_MODEL_TODO_CAPTURE",
		keywords: &["todo", "capture", "task", "list", "model"],
		fallback: SlotFallback::SessionModel,
	},
	ModelSlot {
		id: "memory_flush",
		label: "Memory flush model",
		description: "Model that writes the session's long-term memory entries.",
		env: "GROK_MODEL_MEMORY_FLUSH",
		keywords: &["memory", "flush", "remember", "model"],
		fallback: SlotFallback::SessionModel,
	},
	ModelSlot {
		id: "goal_planner",
		label: "Goal planner model",
		description: "Model that writes the plan for a `/goal`.",
		env: "GROK_MODEL_GOAL_PLANNER",
		keywords: &["goal", "planner", "plan", "model"],
		fallback: SlotFallback::SessionModel,
	},
	ModelSlot {
		id: "goal_strategist",
		label: "Goal strategist model",
		description: "Model that re-reads a stalled goal and proposes a new angle.",
		env: "GROK_MODEL_GOAL_STRATEGIST",
		keywords: &["goal", "strategist", "strategy", "model"],
		fallback: SlotFallback::SessionModel,
	},
	ModelSlot {
		id: "goal_skeptic",
		label: "Goal skeptic model",
		description: "Model for every adversarial skeptic that verifies a goal. \
		              A slow model here slows every goal check.",
		env: "GROK_MODEL_GOAL_SKEPTIC",
		keywords: &[
			"goal",
			"skeptic",
			"verifier",
			"classifier",
			"achievement",
			"panel",
			"model",
		],
		fallback: SlotFallback::SessionModel,
	},
	ModelSlot {
		id: "goal_summarizer",
		label: "Goal summary model",
		description: "Model that writes the closing summary when a goal completes.",
		env: "GROK_MODEL_GOAL_SUMMARIZER",
		keywords: &["goal", "summarizer", "summary", "model"],
		fallback: SlotFallback::SessionModel,
	},
	ModelSlot {
		id: "subagent_default",
		label: "Subagent default model",
		description: "Model every subagent runs on unless its own type pins one.",
		env: "GROK_MODEL_SUBAGENT_DEFAULT",
		keywords: &["subagent", "agent", "task", "default", "model"],
		fallback: SlotFallback::SessionModel,
	},
];

/// The settings-modal key for each slot, as `(slot id, setting key)`.
///
/// The settings registry stores `&'static str` keys, so each one is spelled
/// out here rather than built at run time.
pub fn slot_setting_keys() -> &'static [(&'static str, &'static str)] {
	&[
		("web_search", "models.web_search"),
		("image_description", "models.image_description"),
		("session_summary", "models.session_summary"),
		("prompt_suggestion", "models.prompt_suggestion"),
		("permission_classifier", "models.permission_classifier"),
		("laziness_classifier", "models.laziness_classifier"),
		("compaction", "models.compaction"),
		("recap", "models.recap"),
		("turn_summary", "models.turn_summary"),
		("side_note", "models.side_note"),
		("todo_capture", "models.todo_capture"),
		("memory_flush", "models.memory_flush"),
		("goal_planner", "models.goal_planner"),
		("goal_strategist", "models.goal_strategist"),
		("goal_skeptic", "models.goal_skeptic"),
		("goal_summarizer", "models.goal_summarizer"),
		("subagent_default", "models.subagent_default"),
	]
}

/// The slot a settings-modal key names, or `None` for any other key.
pub fn slot_for_setting_key(key: &str) -> Option<&'static ModelSlot> {
	let id = slot_setting_keys()
		.iter()
		.find(|(_, k)| *k == key)
		.map(|(id, _)| *id)?;
	slot_by_id(id)
}

/// The slot with this id.
pub fn slot_by_id(id: &str) -> Option<&'static ModelSlot> {
	HARNESS_MODEL_SLOTS.iter().find(|s| s.id == id)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn every_slot_has_a_setting_key_and_back() {
		for slot in HARNESS_MODEL_SLOTS {
			let key = slot.setting_key();
			assert_eq!(
				slot_for_setting_key(key).map(|s| s.id),
				Some(slot.id),
				"setting key {key} must round-trip to slot {}",
				slot.id
			);
		}
		assert_eq!(
			slot_setting_keys().len(),
			HARNESS_MODEL_SLOTS.len(),
			"slot_setting_keys and HARNESS_MODEL_SLOTS drifted"
		);
	}

	#[test]
	fn slot_ids_and_env_vars_are_unique() {
		let mut ids: Vec<&str> = HARNESS_MODEL_SLOTS.iter().map(|s| s.id).collect();
		ids.sort_unstable();
		let before = ids.len();
		ids.dedup();
		assert_eq!(before, ids.len(), "duplicate slot id");

		let mut envs: Vec<&str> = HARNESS_MODEL_SLOTS.iter().map(|s| s.env).collect();
		envs.sort_unstable();
		let before = envs.len();
		envs.dedup();
		assert_eq!(before, envs.len(), "duplicate slot env var");
	}

	#[test]
	fn keywords_are_lowercase_and_non_empty() {
		for slot in HARNESS_MODEL_SLOTS {
			assert!(!slot.keywords.is_empty(), "{} has no keywords", slot.id);
			for kw in slot.keywords {
				assert!(!kw.is_empty(), "{} has an empty keyword", slot.id);
				assert_eq!(
					*kw,
					kw.to_lowercase(),
					"{} keyword {kw} must be lowercase",
					slot.id
				);
			}
		}
	}

	#[test]
	fn compiled_fallback_slots_name_a_model_and_inherit_slots_do_not() {
		for slot in HARNESS_MODEL_SLOTS {
			match slot.fallback {
				SlotFallback::Compiled => assert!(
					slot.compiled_default().is_some_and(|m| !m.is_empty()),
					"{} claims a compiled default and has none",
					slot.id
				),
				SlotFallback::SessionModel | SlotFallback::ConsumerDefault => assert!(
					slot.compiled_default().is_none(),
					"{} falls back at the consumer and must name no compiled default",
					slot.id
				),
			}
		}
	}
}
