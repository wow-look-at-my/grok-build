# Goal verification runs in the background

- A full goal's `candidate_complete` and an upheld give-up claim start the skeptic panel as a background task (`verify_goal_candidate` in `acp_session_impl/goal.rs`). The turn ends at once. The user can keep using the session while the panel runs.
- `GoalBackground::verification_running` is the hold `GoalCheckHold::Verification` (`goal_hold.rs`). While it is set, no round runs the goal checker and no continuation is queued.
- The panel has no parent prompt and no foreground wait. A turn cancel does not reach it. A user Stop that cancels every subagent still ends it as `UserCancelled`.
- The panel task applies the verdict, tells the user about a rejection with its gaps, and sends `SessionCommand::GoalVerificationSettled`. `resume_goal_after_verification` then queues the continuation of a goal that is still active. An upheld claim's panel note goes in front of the directive.
- A `/goal` set, resume, pause or clear sent while the panel runs is queued (`defer_goal_command_while_verifying`). The panel task puts each one back in the input queue, in order, when it ends. `/goal status` runs at once.
- A test actor with no `Arc` runs the panel inside the turn, as before.
