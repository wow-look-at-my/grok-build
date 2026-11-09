# Status-bar notes

- The top-right group is built in `render_status_bar` (`crates/codegen/xai-grok-pager/src/app/agent_view/render.rs`). Push order is screen order. The group is right-aligned. The dashboard button is always the last item.
- One item carries the todo count. With no goal it is the badge, a green check then `done/total` (`render_todo_badge_spans`). While a goal exists the count moves inside the goal chip (`goal_status_line`), and the badge is not drawn.
- The goal chip shows no phase, tokens, budget, elapsed time or spinner. It adds one state word when the goal does not simply run (`goal_state_word`). Only a running goal is gold. Failed and interrupted are a red chip. Every other state is gray.
- A running goal with no todo item is invalid. The chip returns `None`, and `report_goal_without_todos` logs one error line per goal. The shell keeps a goal from reaching that state: see the lite-goal notes in `AGENTS.md` and `goal-plan-to-todos-notes.md`.
- The context item is tokens used, then the percent used. The percent is measured against the auto-compact point, the window times `ContextInfo.auto_compact_threshold_percent`, so it reaches its maximum when compaction fires.
- The cache-invalidated warning takes the cost's slot while it shows. The cost has no `session` suffix.
- With several sessions the switcher's middle is the dashboard button. It shows the position until the pointer is over it, then the dashboard glyph at the same width. `hit_dashboard` is the switcher's rect minus the chevrons. With one session the bare glyph is the button.
- The bar draws no plan chip and no hovered link URL.

```
◆ 2 │ [Goal: 4/5] │ 110K · 65% │ $4.20 │ ‹ 2/5 ›
◆ 2 │ ✔ 4/5 │ 110K · 65% │ cache invalidated 12s ago │ ☰
```
