use super::*;

/// The objective of a goal made from an approved plan. It names the plan's
/// headline, without the `Plan:` prefix the goal planner writes.
pub(crate) fn approved_plan_objective(plan: &str) -> String {
    let headline = plan
        .lines()
        .find_map(|line| line.trim().strip_prefix("# "))
        .map(|h| h.trim().strip_prefix("Plan:").unwrap_or(h).trim())
        .filter(|h| !h.is_empty());
    match headline {
        Some(headline) => format!("Implement the approved plan: {headline}"),
        None => "Implement the approved plan".to_string(),
    }
}

impl SessionActor {
    /// Make a goal from the plan the user just approved. Returns the
    /// goal-start reminder, or `None` when no goal was made.
    ///
    /// A subagent and a session without the goal harness get no goal. A goal
    /// that is already active stays in place: approving a plan never replaces
    /// it.
    pub(super) async fn setup_goal_from_approved_plan(&self, plan_content: &str) -> Option<String> {
        use crate::session::goal_tracker::{GoalPauseReason, GoalStatus};
        if self.startup_hints.is_subagent || !self.goal_harness_enabled() {
            return None;
        }
        if self.goal_tracker.lock().status() == Some(GoalStatus::Active) {
            tracing::info!("approved plan: a goal is already active, so no goal was made");
            return None;
        }
        let objective = approved_plan_objective(plan_content);
        let goal_id = self.create_goal_orchestration(&objective, None).await;
        let (plan_path, baseline_path) = {
            let tracker = self.goal_tracker.lock();
            (tracker.plan_path(), tracker.plan_baseline_path())
        };
        if let Err(err) = tokio::fs::write(&plan_path, plan_content).await {
            tracing::error!(
                error = %err,
                path = %plan_path.display(),
                "approved plan: could not write the goal plan"
            );
            self.auto_pause_goal_if_matches_with_message(
				&goal_id,
				GoalPauseReason::User,
				format!(
					"Could not save the approved plan to {}: {err}. Resume with /goal to plan it again.",
					plan_path.display()
				),
			)
			.await;
            return None;
        }
        // Without a baseline the verifier cannot diff later plan edits.
        let baseline = match tokio::fs::write(&baseline_path, plan_content).await {
            Ok(()) => Some(baseline_path),
            Err(err) => {
                tracing::error!(
                    error = %err,
                    path = %baseline_path.display(),
                    "approved plan: could not write the plan baseline; PLAN_CHANGES will render (none)"
                );
                None
            }
        };
        let current_tokens = self.chat_state_handle.get_total_tokens().await as i64;
        let (tokens_used, finished_marginal) = self.goal_tokens(current_tokens);
        {
            let mut tracker = self.goal_tracker.lock();
            let goal = tracker
                .snapshot_mut()
                .filter(|goal| goal.goal_id == goal_id && goal.status == GoalStatus::Active)?;
            goal.plan_file = Some(plan_path);
            goal.plan_baseline_file = baseline;
            self.goal_notify_sender().emit_goal_updated(
                &mut tracker,
                tokens_used,
                finished_marginal,
            );
        }
        tracing::info!(goal_id = %goal_id, "approved plan: goal created from the plan");
        Some(
            self.render_goal_start_reminder(&objective, |o| o.plan_file.as_deref())
                .await,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::approved_plan_objective;

    #[test]
    fn the_objective_names_the_plan_headline() {
        assert_eq!(
            approved_plan_objective("# Plan: Add JWT auth\n\n## Acceptance criteria\n1. x\n"),
            "Implement the approved plan: Add JWT auth"
        );
        assert_eq!(
            approved_plan_objective("Intro line\n\n# Cache the API\n"),
            "Implement the approved plan: Cache the API"
        );
    }

    #[test]
    fn a_plan_without_a_headline_still_gets_an_objective() {
        // `## ` is a section, not the headline.
        assert_eq!(
            approved_plan_objective("## Context\nSome text\n"),
            "Implement the approved plan"
        );
        assert_eq!(
            approved_plan_objective("# Plan:   \n"),
            "Implement the approved plan"
        );
    }
}
