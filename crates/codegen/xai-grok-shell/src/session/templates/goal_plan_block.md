A structured plan for this goal is on disk — the source of truth for "done".
Read it first and keep it open.

Plan: {PLAN_PATH}

- The plan's steps are ALREADY on your todo list — the goal planner added one
  item per step, in order, so do not re-read this plan to populate it. Work them
  in order and keep each item's status current through `{TODO_TOOL}`.
- If the plan has a `## Task checklist`, work it in order and flip each
  `- [ ]` to `- [x]` in the plan file as you complete it — the harness mines
  the first unchecked box as your next-step nudge, so a stale checklist
  produces stale nudges.
- Execute item by item; when you deviate, append a bullet to the plan's single
  `## Deviations` section — add to that one section; don't start a new one, and
  don't edit the plan's existing items. Keep it TERSE: ONE bullet per deviation
  (what changed + why); not a progress log, so don't restate the plan or dump
  test counts / "all fixed" / "verification re-run" / "superseding" notes there.
- Before claiming completion, run the plan's `## Verification plan` yourself and
  confirm its observations hold. Checking is not doing: a step reads back what
  you built, and never authorizes work the objective did not ask for. Commit
  real tests that drive the shipped code in-repo, and RUN them. The harness
  records each run for the verifier, so save no proof files. Fix any missing
  observation before calling the goal complete.
