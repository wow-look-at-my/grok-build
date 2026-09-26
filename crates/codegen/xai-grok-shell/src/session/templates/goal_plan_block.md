A structured plan for this goal is on disk — the source of truth for "done". Read it first and keep it open.

Plan: {PLAN_PATH}

{CHECKLIST_BULLET}
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
