A structured plan for this goal is on disk — the source of truth for "done". Read it first and keep it open.

Plan: {PLAN_PATH}

{CHECKLIST_BULLET}
- Execute item by item. When you deviate, append a bullet to the plan's single `## Deviations` section — add to that section. Do not start a new one, and do not edit the plan's existing items. Keep it TERSE: ONE bullet per deviation (what changed + why). This also covers not a progress log, so do not restate the plan or dump test counts / "all fixed" / "verification re-run" / "superseding" notes there.
- Before claiming completion, run the plan's `## Verification plan` yourself and confirm its observations hold. Checking is not doing: a step reads back what you built, and never authorizes work the objective did not ask for. Use the project's existing test runner and entry point, and RUN them. Write no check script or harness of your own. Collecting evidence is the verifier's job, not yours: save nothing about your runs and never read the session transcript. Fix any missing observation before calling the goal complete.
