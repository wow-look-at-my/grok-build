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
- A plan step states what must be TRUE. It is not a permit to perform it. Each
  `## Verification plan` step carries a reach label. Run the `[artifact]` ones,
  which read files, logs, hashes, build output and source here. Do NOT run a
  `[live-system]` one unless the user's own words named that action. Print its
  exact command line in one line instead and say it is the user's call. That
  hand-back satisfies the criterion as `awaiting-user` and does not block
  completion. A step with no label is treated as `[live-system]`.
- Before claiming completion, run the plan's `[artifact]` `## Verification plan`
  steps yourself and confirm their observations hold. SAVE durable proof:
  commit real tests that drive the shipped code in-repo, and write the captured run output to your scratch dir
  (the one the goal rules name; never shared `/tmp/...`). Fix any missing
  observation before calling the goal complete.
