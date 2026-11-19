# The todo list cannot be discarded or overwritten

- A todo is the user's. Nothing can delete one: an item leaves the actionable set only by becoming `Completed` or `Cancelled`, both of which name it by id. Sending that id with new content changes text.
- Every `todo_write` is a merge, and an item the call omits survives with its status untouched. `merge: false` used to clear the list and keep only what the call resent. This is how a status update that forgot the flag erased the user's list.
- `merge` is still accepted on the wire and ignored. It is `#[schemars(skip)]` now that both values behave the same — advertising it will describe a choice the tool no longer offers.
- `TodoState` has no `clear` and no remove of any shape. The guarantee lives in the data structure so a later caller cannot reach around it.
- The list only grows, and every `todo_write` echoes all of it. So `summarize_todo_state` echoes a completed or cancelled item as its first line, cut at `FINISHED_ITEM_ECHO_CHARS`. The state keeps the full text. The post-compaction reminder already collapses finished items to counts.
- A goal start (`create_goal_orchestration`) and a goal completion call `archive_finished_todos`. It sets `TodoItem::archived` on every finished item. That item stays in the state, but the echo shows only a count, and the todo pane and goal popup hide it (`a` shows it). A status update to open work clears the flag.
- opencode's `todowrite` sends a whole list with no ids, so it merges by ITEM TEXT, not by position. Position is not identity. Keying on it let a reordered or shorter list write one row's text over another's, which loses work as surely as a delete.
