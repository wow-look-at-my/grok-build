# Configuration

Grok reads settings from config files, environment variables, and CLI flags. This page covers the common options. The field list for `config.json`, `managed_config.toml`, and `requirements.toml` is [26-config-reference.md](26-config-reference.md) (extracted to `~/.grok/docs/user-guide/` on launch).

---

## Precedence

Settings resolve highest-priority first:

1. **CLI flags** (e.g. `--yolo`, `--model`, `--sandbox`)
2. **Environment variables** (e.g. `XAI_API_KEY`, `GROK_MEMORY`)
3. **`requirements.toml` / MDM** (org-enforced. Clamps every config layer below, including the overlay)
4. **`GROK_CONFIG` / `GROK_CONFIG_PATH` overlay** (above `config.json` and managed, below `requirements.toml` / MDM)
5. **config.json** (`~/.grok/config.json`)
6. **`managed_config.toml`** (org-deployed defaults. Below `config.json`)
7. **Built-in defaults**

Within the config-file tier, the layers merge lowest-to-highest: `managed_config.toml` → `config.json` → `GROK_CONFIG` overlay → `requirements.toml` / MDM. So `requirements.toml` and MDM clamp **both** your `config.json` and the overlay.

`GROK_CONFIG` / `GROK_CONFIG_PATH` (tier 4) are config **overlays**: a merged config layer, not direct-setting environment variables like `XAI_API_KEY` (tier 2). They set config keys (subject to the allowlist below), so read them as part of the config-file tier rather than the env-var tier.

### Injecting config with `GROK_CONFIG`

A harness or ACP client that launches `grok agent stdio` can inject settings without writing a `config.json` or relocating `$GROK_HOME`:

- **`GROK_CONFIG`**: an inline JSON object overlay.
- **`GROK_CONFIG_PATH`**: an *additional* file overlay (not a replacement for `config.json`), a JSON or TOML file read by its extension (`.json` → JSON, else TOML). `GROK_CONFIG` wins if both are set. An empty `GROK_CONFIG` is treated as unset, and a malformed one logs a warning and falls through to `GROK_CONFIG_PATH`.

The overlay is **deep-merged** on top of your `config.json` (it overrides only the keys it sets), placed above the user/managed layers but **below** `requirements.toml` / MDM. As a result, an enterprise pin still wins. A malformed blob is ignored with a warning. This mirrors `CODEX_CONFIG` from the `codex-acp` adapter (a JSON object merged into the session config). Grok is ACP-native. As a result, the overlay lives in the agent itself.

It only affects settings read from the merged config. It is **not** a permission-escalation path. The overlay is confined, fail-closed, to an **allowlist** of soft settings (`models`, `features`, a narrowed `toolset`, and a `shell_environment_policy` limited to its filter fields, which select among env names the launcher already controls and cannot inject an env value into tool subprocesses). Every other table is dropped at the choke point. As a result, the overlay cannot spawn commands, set auth policy, redirect network traffic, elevate trust, or add a discovery source. Even on the allowlisted settings, a specific set of security gates read the raw disk layers rather than the overlay.

The `ConfigLayers::env_overlay` rustdoc is the canonical list of what the overlay can and cannot reach and which gates read it overlay-free. See also the [internal environment-variables reference](../internal/22-environment-variables.md). Use `GROK_DEFAULT_SELECTED_PERMISSION` for headless permission control. For example, to set the default reasoning effort:

```bash
GROK_CONFIG='{"models": {"default_reasoning_effort": "high"}}' grok agent stdio
```

---

## config.json (main configuration)

Location: `~/.grok/config.json`. If the file is missing, Grok uses its built-in defaults, so you only need to set the values you want to override.

### General settings

```json
{
  "agents": {
    "selection": {}
  },
  "features": {
    "codebase_indexing": true,
    "feedback": true,
    "lsp_tools": false,
    "remote_fetch": true,
    "telemetry": false,
    "two_pass_compaction": true
  },
  "models": {
    "default": "grok-4.5",
    "extra_headers": {
      "X-Request-Tags": "team=example,env=prod"
    },
    "inference_idle_timeout_secs": 600,
    "max_completion_tokens": 8192,
    "max_retries": 8,
    "stream_tool_calls": true,
    "subagent_rate_limit_max_attempts": 8,
    "temperature": 0.7,
    "top_p": 0.95,
    "web_search": "grok-4.5"
  },
  "session": {
    "auto_compact_threshold_percent": 85,
    "load_envrc": true
  },
  "tools": {
    "respect_gitignore": false
  },
  "ui": {
    "collapsed_edit_blocks": false,
    "default_selected_permission": "always_allow_all_sessions",
    "follow_up_behavior": "queue",
    "group_tool_verbs": true,
    "max_thoughts_width": 120,
    "page_flip_on_send": true,
    "remember_tool_approvals": true,
    "show_thinking_blocks": true,
    "thinking_summaries": true,
    "thinking_summary_history_min_count": 5,
    "thinking_summary_history_window_secs": 120
  }
}
```

### Default agent

Interactive `grok` uses `agents.selection` in `config.json` when you do not pass `--plan`, `--ask-user`, or `--agent-profile`:

```json
{
  "agents": {
    "selection": {
      "name": "my-custom-agent"
    }
  }
}
```

`definition` is a markdown file with YAML frontmatter. `name` is a built-in or discovered agent (`~/.grok/agents/`, `.grok/agents/`). If the named agent is missing, Grok uses `GROK_AGENT`, then the built-in default. `--agent-profile`, `--plan`, and `--ask-user` still override that session. Field list: [26-config-reference.md](26-config-reference.md).

#### Default selected permission

When the agent asks to run a command (or take some other tool action), the approval menu highlights one row by default. `ui.default_selected_permission` sets which row that is on the **first** prompt of a session.

| Value | Preselected row |
|-------|-----------------|
| `always_allow_all_sessions` (default) | The "Always allow on all sessions" row. |
| `allow_command_always` | The "Always allow this command" row. |
| `allow_once` | The "Yes" / allow-once row. |
| `reject` | The reject row. |

```json
{
  "ui": {
    "default_selected_permission": "allow_once"
  }
}
```

After you answer the first prompt the cursor turns **sticky**: each later prompt preselects whatever you last confirmed (pick "No" once and subsequent prompts start on their reject row), carrying across edit / bash / MCP prompts. This is until you start. So this setting only picks the starting point.

Values match case-insensitively. An unset or unrecognized value falls back to `always_allow_all_sessions`. The `allow_command_always` row is always scoped to the specific action being approved (command / tool / domain / edit-session), not a global allow-everything — that is what `always_allow_all_sessions` is for. Note the per-command "Always allow" rows appear while `ui.remember_tool_approvals` is enabled (the default. Set it to `false` to hide them). See [22-permissions-and-safety.md](22-permissions-and-safety.md).

You can also override this with `GROK_DEFAULT_SELECTED_PERMISSION`, which is handy for headless or agent test runs that must not mutate `config.json`. Precedence: env var → `config.json` → `always_allow_all_sessions`.

#### Snap prompt to top on send

By default, sending a prompt scrolls it to the top of the viewport so the response starts on a fresh page. Set `ui.page_flip_on_send` to `false` (or toggle **Snap prompt to top on send** in `/settings` → Appearance) to leave the scroll position alone when you send. It takes effect on the next send — no restart.

#### Scrolling

`ui` settings tune mouse-wheel and trackpad scrolling. All apply immediately and are editable from the settings pane (`/settings` → **Scroll speed** / **Scroll input** / **Scroll lines** / **Invert scroll**).

| Key | Values (default) | Behavior |
|-----|------------------|----------|
| `scroll_speed` | `1`–`100` (`50`) | Speed multiplier for wheel and trackpad. `50` = 1.0x, `1` = 0.1x, `100` = 6.0x. |
| `scroll_mode` | `auto` \| `wheel` \| `trackpad` (`auto`) | Wheel-vs-trackpad detection is heuristic (terminal scroll events carry no magnitude); force one when auto-detection misreads your device — e.g. a wheel notch that jumps too far, or a trackpad that feels stepped. |
| `scroll_lines` | `1`–`10` (unset) | Lines per scroll tick, applied to **both** wheel and trackpad. While unset, each terminal's own profile applies (e.g. a conservative 1 line/event under tmux). Committing any value — even `3`, the number the settings pane shows — switches permanently to that explicit override. |
| `invert_scroll` | `false` \| `true` (`false`) | Reverse vertical scroll direction ("natural" scrolling). |

```json
{
  "ui": {
    "invert_scroll": false,
    "scroll_mode": "auto",
    "scroll_speed": 50
  }
}
```

Each setting also has an environment-variable override, applied on first load only (again, handy for headless / test runs): `GROK_SCROLL_SPEED`, `GROK_SCROLL_MODE`, `GROK_INVERT_SCROLL` (`1`/`true`/`0`/`false`), and `GROK_SCROLL_LINES`. Precedence: env var → `config.json` → default. Unrecognized values fall back to the default, and out-of-range numbers clamp.

#### Slow output

Some inference engines drop to a crawl in the middle of a response. Grok watches the output rate. When a model call stays too slow, Grok starts a backup of that call and keeps the slow one streaming. Whichever gets ahead is the answer, and a slow call that speeds back up cancels its backup. Tune it in `/settings` → **Slow output**, or in `ui`. A change applies to the next model call of every running session.

The `N tok/s` counter beside the timers appears as soon as the measured rate settled, usually a few hundred milliseconds into the response. It shows that same rate the floor judges. And a stall brings it down rather than clearing it. This happens once it appears it stays for the rest of the model call: a speed-up or a slow-down changes the number. While the row waits for the model, after the call ends, and during a retry, there is no stream. The counter is hidden.

| Key | Values (default) | Behavior |
|-----|------------------|----------|
| `min_output_tokens_per_sec` | `0`–`500` (`15`) | The floor. `0` turns detection off. `models.definitions.<id>.min_output_tokens_per_sec` overrides it for one model. |
| `output_rate_sustained_secs` | `1`–`600` (`10`) | How long the rate must stay under the floor before a backup starts. |
| `output_rate_window_secs` | `2`–`120` (`10`) | How many seconds of output the rate is averaged over. |
| `output_rate_max_retries` | `0`–`5` (`2`) | How many backups one call can start. After that the response is kept at whatever rate it runs. `0` starts none. |
| `ttft_timeout_secs` | `0`–`1800` (`120`) | Time-to-first-token limit. A call with no output this many seconds after it was sent is reissued, on the same retry budget. `0` turns it off. `models.providers.<id>.ttft_timeout_secs` overrides it for a provider, and `models.definitions.<id>.ttft_timeout_secs` for one model. |

```json
{
  "ui": {
    "min_output_tokens_per_sec": 15,
    "output_rate_max_retries": 2,
    "output_rate_sustained_secs": 10,
    "output_rate_window_secs": 10,
    "ttft_timeout_secs": 120
  }
}
```

The older `agents.output_rate_floor` object (`window_secs`, `max_retries`) still works. A `ui` key wins where both are set.

### Tool configuration

```json
{
  "tools": {
    "ask_user_question": {
      "timeout_enabled": true,
      "timeout_secs": 1800
    },
    "bash": {
      "output_byte_limit": 20000,
      "timeout_secs": 120.0
    },
    "web_fetch": {
      "allow_local": false,
      "allowed_domains": [
        "docs.rs",
        "example.invalid"
      ],
      "proxy_endpoint": "https://proxy.example.com"
    },
    "web_search": {
      "allowed_domains": [
        "docs.example.invalid",
        "arxiv.org"
      ]
    }
  }
}
```

`allow_local` is off by default (SSRF fail-closed). Turn it on (or set `GROK_WEB_FETCH_ALLOW_LOCAL=1`) and `web_fetch` may reach **explicit** loopback hosts only — private, link-local, and cloud-metadata ranges stay blocked. Resolution: config file > env > default off.

`tools.web_search` constrains the `web_search` tool's domains — the allowlist/blocklist the search itself runs under (not a post-filter). `allowed_domains` and `excluded_domains` are **mutually exclusive**. If you set both, the allowlist wins and the blocklist is dropped with a warning. An empty or absent list is unbounded. This applies to both the backend-hosted search (models with server-side search) and the client-side fallback. A configured policy is **authoritative**: it cannot be bypassed by the model.

The model's own per-call `allowed_domains` is ignored when you set `allowed_domains` or `excluded_domains` here (so a blocklist is a real block). The model's per-call allowlist only applies when you configured nothing. Resolution: requirements → user `config.json` → managed → default (unset). Config is read at session start, so edit it before starting a session — changes do not apply mid-session.

`tools.ask_user_question` is honored across **requirements.toml**, **managed config**, and your user **`config.json`**. Precedence: requirements → env (`GROK_ASK_USER_QUESTION_TIMEOUT_ENABLED` / `GROK_ASK_USER_QUESTION_TIMEOUT_SECS`) → user config → managed → defaults. Set `"timeout_enabled": false` in your user config to disable the automatic questionnaire timeout for yourself. `timeout_secs` must be a positive integer. You can also toggle `timeout_enabled` from `/settings` → **Ask-Question timeout** (under Agent & Approval). Changes apply to newly started sessions.

### Authentication

See [Authentication](02-authentication.md) for the full story.

```json
{
  "login": {
    "auth_provider_command": "/usr/local/bin/my-auth-provider",
    "auth_provider_label": "Acme Corp",
    "auth_token_ttl": 3600,
    "oidc": {
      "client_id": "0oa1b2c3d4e5f6g7h8i9",
      "issuer": "https://acme.okta.com"
    }
  }
}
```

### Harness model slots

The harness sends requests to a model for many jobs beside your own turns. Each job is a slot in `[models]`. Set a slot and that job runs on the model you name. Leave it alone and the job inherits your session model, so it follows `/model`.

Every slot also has a picker in the settings modal, under **Models**, or shares its group's picker (see [Model groups](#model-groups)). A slot is read when a session starts. So a change applies to the next session.

```json
{
  "models": {
    "compaction": "grok-4.5-fast",
    "goal_skeptic": "grok-4.5-fast",
    "permission_classifier": "grok-4.5-fast"
  }
}
```

| Slot | What it runs | Unset |
| --- | --- | --- |
| `web_search` | Web-search synthesis | Compiled default |
| `image_description` | Transcribing images you attach | Compiled default |
| `session_summary` | The session's title | Compiled default |
| `prompt_suggestion` | Tab-autocomplete ghost text | Built-in small model |
| `permission_classifier` | Approving tool calls in Auto mode | Session model |
| `laziness_classifier` | Judging an idle turn | Session model |
| `compaction` | Summarizing when the window fills | Session model |
| `recap` | The "where was I" recap | Session model |
| `turn_summary` | The one-line summary per turn | Session model |
| `thinking_summary` | The short summary under a thinking block | Session model |
| `side_note` | `/btw` | Session model |
| `todo_capture` | `/todo` | Session model |
| `memory_flush` | Writing long-term memory | Session model |
| `goal_planner` | The `/goal` plan | Session model |
| `goal_strategist` | A new angle on a stalled goal | Session model |
| `goal_skeptic` | Every skeptic in the verification panel | Session model |
| `goal_summarizer` | The closing summary of a goal | Session model |

Each slot takes an environment variable too, which wins over the config file. The name is the slot in upper case under `GROK_MODEL_`, so `goal_skeptic` reads `GROK_MODEL_GOAL_SKEPTIC`. The exception is `prompt_suggestion`, whose variable keeps its older name `GROK_PROMPT_SUGGESTIONS_MODEL`.

Two older, narrower keys still win over their slot where you set them: `agents.auto_mode.classifier_model` over `permission_classifier`, and `memory.flush_model` over `memory_flush`.

A slot naming a model your account cannot reach logs a warning and falls back to the session model, rather than failing the call.

### Model groups

A model group is a set of slots that share one model and one settings row. A slot in a group has no row of its own. The settings modal shows the group's row in its place. A group with no slots does not show. A slot is in one group at most.

Groups ship by default:

| Group | Label | Slots |
| --- | --- | --- |
| `summarization` | Summarization model | `session_summary`, `prompt_suggestion`, `recap`, `turn_summary`, `thinking_summary`, `goal_summarizer` |
| `dense_summarization` | Dense summarization model | `web_search`, `compaction`, `image_description` |
| `query` | Query model | `side_note`, `todo_capture` |

Change, unroll or add groups under `models.groups.<id>`. Each key is optional. On a default group, a key you leave out keeps its default value.

```json
{
  "models": {
    "groups": {
      "planning": {
        "description": "Model for the goal planner and strategist.",
        "label": "Planning model",
        "members": [
          "goal_planner",
          "goal_strategist"
        ]
      },
      "query": {
        "members": []
      },
      "summarization": {
        "model": "grok-4.5-fast"
      }
    }
  }
}
```

A slot that your own group lists leaves the default group it was in. So `planning` above can take `recap` without an edit to `summarization`. Two of your groups that list the same slot, an unknown slot, or a group id with characters other than `a-z`, `0-9`. `_` and `-` stop the start with an error that names the problem.

A slot reads its model in this order: the slot's environment variable, then its group's `model`, then its own `[models]` entry, then its default. A group with no `model` changes nothing, so each slot keeps its own entry and default. A pick on a group's settings row writes `[model_groups.<id>] model` and removes the `[models]` entry of each slot in the group.

### Custom models

Add custom model endpoints to use alternative providers or self-hosted models. Each endpoint is a `[model.<id>]` entry. The base URL must include any provider-specific prefix such as `/v1`. Grok appends `/chat/completions` or `/responses`.

```json
{
  "models": {
    "definitions": {
      "my-model": {
        "api_key": "sk-...",
        "base_url": "https://api.example.com/v1",
        "context_window": 128000,
        "description": "Model description",
        "env_http_headers": {
          "X-Tenant": "TENANT_TOKEN"
        },
        "env_key": "XAI_API_KEY",
        "max_completion_tokens": 8192,
        "model": "model-id",
        "name": "Display Name",
        "query_params": {
          "api-version": "2026-07-22"
        },
        "temperature": 0.7,
        "top_p": 0.95
      }
    }
  }
}
```

Credential resolution: `api_key` > `env_key` > signed-in session token > `XAI_API_KEY`. See [Custom Models](11-custom-models.md#request-query-parameters) for `query_params` and `env_http_headers`, and [Sandbox Mode](18-sandbox.md#shell-environment-policy) for `[shell_environment_policy]`, which restricts the environment variables tool subprocesses inherit.

Several models behind one endpoint share a `[model_providers.<id>]` block instead of repeating the URL, the key, the backend and the headers on each one:

```json
{
  "models": {
    "definitions": {
      "acme-fast": {
        "model": "acme-fast-1",
        "model_provider": "acme"
      }
    },
    "providers": {
      "acme": {
        "api_backend": "responses",
        "base_url": "https://gateway.acme.com/v1",
        "env_key": "ACME_API_KEY"
      }
    }
  }
}
```

A model's own field always wins. The provider fills in the rest. See [Provider Defaults](11-custom-models.md#provider-defaults).

To override a built-in model, use its name as the section key and set only the fields you need:

```json
{
  "models": {
    "definitions": {
      "grok-4": {
        "6": {
          "api_key": "my-api-key"
        }
      }
    }
  }
}
```

### MCP servers

Configure external tool integrations over the Model Context Protocol.

```json
{
  "mcp": {
    "servers": {
      "github": {
        "args": [
          "-y",
          "@modelcontextprotocol/server-github"
        ],
        "command": "npx",
        "enabled": true,
        "env": {
          "GITHUB_PERSONAL_ACCESS_TOKEN": "ghp_xxx"
        },
        "startup_timeout_sec": 30,
        "tool_timeout_sec": 6000,
        "tool_timeouts": {
          "create_issue": 120
        }
      },
      "my-streamable-server": {
        "headers": {
          "x-mcp-session-id": "{{session_id}}"
        },
        "url": "https://mcp.example.com/api/mcp"
      },
      "postgres": {
        "args": [
          "-y",
          "@modelcontextprotocol/server-postgres",
          "postgresql://user:pass@localhost/db"
        ],
        "command": "npx"
      }
    }
  }
}
```

Remote (HTTP/SSE) servers receive a default `User-Agent: grok-cli/<version>` header. A valid `User-Agent` entry in `headers` overrides it (Figma servers receive bare `grok-cli`). See [MCP servers](07-mcp-servers.md) for details.

MCP servers can also be set per-project in `.grok/config.json`. Project-scoped config contributes `[mcp_servers]`, `[plugins]`, and `[permission]` rules. Every other section loads only from `~/.grok/config.json`.

Priority for `[mcp_servers]` and `[plugins]`: `.grok/config.json` (current dir) > `<repo-root>/.grok/config.json` > `~/.grok/config.json`. `[permission]` rules are not overridden by priority — they merge across all files with `deny` > `ask` > `allow` (see [22-permissions-and-safety.md](22-permissions-and-safety.md)).

### Memory

Persist knowledge across sessions. Memory is on by default and uses the v2 pipeline. An explicit `[memory] enabled = false` turns it off even when a managed remote setting enables it. Notes recorded by earlier versions are carried over automatically. See [13-memory.md](13-memory.md).

```json
{
  "memory": {
    "embedding": {
      "dimensions": 1024
    },
    "enabled": true,
    "initial_injection": {
      "enabled": true,
      "min_score": 0.9
    },
    "search": {
      "max_results": 6,
      "min_score": 0.7
    },
    "session": {
      "save_on_end": true
    },
    "watcher": {
      "enabled": true
    }
  }
}
```

### Subagents

```json
{
  "agents": {
    "subagents": {
      "enabled": true,
      "models": {
        "explore": "grok-4.6"
      },
      "sampling_limit": 12,
      "toggle": {
        "explore": true,
        "plan": false
      },
      "usage_frequency": "default"
    }
  }
}
```

To pin the model a subagent uses, set its entry under `[subagents.models]`. Settings has one row per spawnable type, including workspace and plugin agents, and each row writes that type's entry. `usage_frequency` does not gate the tool -- it tunes how strongly the system prompt and tool wording nudge the model toward delegating. See [Subagents and Personas](16-subagents.md#usage-frequency).

### Goal mode and background workflows

The host-owned workflow engine evaluates `/goal` rounds and drives completion verification, whatever the background-workflows setting. Whether `/goal` is available at all is a separate switch (the goal feature setting).

Background workflows — the `workflow` tool, named `.grok/workflows/*.rhai` scripts, `/deep-research`, and `/workflow` launches — are **on by default**. Disable with config, env, or remote settings.

```json
{
  "agents": {
    "workflows": {
      "enabled": false
    }
  }
}
```

Project workflows are discovered from `<repo-root>/.grok/workflows/`. User workflows from `~/.grok/workflows/`. Discovery and invocation key off the script's `meta.name`, so keep each filename aligned with its `meta.name`. Built-ins win over project names, and project names win over user names, so keep names unique across scopes.

#### Which model runs the `/goal` roles

Every `/goal` role has a slot in `[models]`. See [Harness model slots](#harness-model-slots). Reach for `goal_skeptic` when verification feels slow. It sets the model for every skeptic in the panel.

`[goal]` takes a second form of pin. That form carries an agent type beside the model. Use it when a role needs a different harness flavor:

```json
{
  "agents": {
    "goal": {
      "follow_remote_role_models": true,
      "planner_model": {
        "agent_type": "grok-build-plan",
        "model": "grok-build"
      },
      "use_current_model_only": true
    }
  }
}
```

A `[goal]` pin wins over the matching `[models]` slot. `follow_remote_role_models` is off by default, so a server-side pin never replaces the model you selected. `use_current_model_only` is the kill switch: it overrides local pins too, even for a goal already in flight. Both read an env var as well (`GROK_GOAL_FOLLOW_REMOTE_ROLE_MODELS`, `GROK_GOAL_USE_CURRENT_MODEL_ONLY`).

Each launch gets a session-unique display handle such as `deep-research-2`. That handle is what you see in the `/workflow runs` dashboard and pass to `/workflow pause`, `resume`, or `stop` — the internal run IDs never surface in commands. A numbered handle is not a reusable definition name. As a result, the dashboard disables **save** until you pick a new unique `meta.name` and save the edited script yourself. See [Slash Commands](04-slash-commands.md) for examples.

### Skills

```json
{
  "extensions": {
    "skills": {
      "disabled": [
        "wip-skill"
      ],
      "ignore": [
        "~/my-team-skills/wip"
      ],
      "paths": [
        "~/my-team-skills"
      ]
    }
  }
}
```

### Harness compatibility

Control vendor compatibility for Cursor and Claude. Every cell defaults to `true`. Session cells stay staged and inert until a foreign-session scanner consumes them. Each tool needs both its `sessions` cell and the matching `resume-claude` or `resume-cursor` skill — a missing skill means zero foreign-session filesystem I/O.

```json
{
  "extensions": {
    "compat": {
      "claude": {
        "agents": true,
        "hooks": true,
        "mcps": true,
        "rules": true,
        "sessions": true,
        "skills": true
      },
      "cursor": {
        "agents": true,
        "hooks": true,
        "mcps": true,
        "rules": true,
        "sessions": true,
        "skills": true
      }
    }
  }
}
```

For Claude and Cursor, `rules` and `agents` are independent. Turning off named instruction files does not disable the home or project rules directory, and turning off rules does not disable named files. Claude's `agents` cell gates home-level `~/.claude/` named files and project `<dir>/.claude/CLAUDE*.md`. Generic top-level `Claude.md`, `CLAUDE.md`, and `CLAUDE.local.md` stay recognized. Project rule paths are scanned at every directory from the repo root down to the current one.

Each cell can be set via environment variable or `config.json`. See the environment-variables reference for the names. Resolution: env var > config.json > default (on).

`grok inspect` reports cells that still need session-start resolution as `?` until a value is available. Cells with an explicit env or TOML value use that value. Affected discovery entries report `compatibilityStatus: "unresolved"` in JSON and `[compat unresolved]` in human output.

### Plugins

```json
{
  "extensions": {
    "plugins": {
      "disabled": [
        "user/a1b2c3d4/noisy-plugin"
      ],
      "paths": [
        "~/my-plugins/custom-tools"
      ]
    }
  }
}
```

### Hints

`[hints]` holds small persisted UI preferences: remembered answers and modal layout. Grok writes these for you as you use the TUI. However, you can edit or delete them by hand. Removing a key restores the default.

`[hints]` is read from the **effective config merge**, with the usual precedence: system managed → user `managed_config.toml` → user `config.json` → user `requirements.toml` → system `requirements.toml`, higher layers winning. The TUI only ever **writes** these to your user `~/.grok/config.json`.

```json
{
  "ui": {
    "hints": {
      "fork_worktree_mode": "ask",
      "memory_modal_fullscreen": false,
      "new_session_worktree_mode": "never"
    }
  }
}
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `memory_modal_fullscreen` | bool | `false` | Remembers whether the memory modal was last opened fullscreen. |
| `new_session_worktree_mode` | string | `"never"` | Worktree prompt for `/new`: `ask` shows the popup, `always` creates a worktree, `never` skips it. |
| `fork_worktree_mode` | string | `"ask"` | Worktree prompt for `/fork`: `ask`, `always`, or `never`. |

### Notifications

Fire terminal notifications when the agent finishes a turn or needs approval. They use terminal-native protocols (OSC 9, OSC 99, OSC 777, or default focus-gates BEL) and, so they only fire when you are not looking at the terminal.

```json
{
  "ui": {
    "notifications": {
      "condition": "unfocused",
      "events": [
        "turn_complete",
        "approval_required"
      ],
      "idle_threshold_secs": 3,
      "method": "auto",
      "progress_bar": true,
      "sleep_prevention": true,
      "title": {
        "enabled": true,
        "items": [
          "action-required",
          "spinner",
          "activity",
          "session-name",
          "grok"
        ]
      }
    }
  }
}
```

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `method` | string | `"auto"` | Notification protocol. `auto` picks the best for your terminal. |
| `condition` | string | `"unfocused"` | When to notify: `unfocused` (only when the terminal lost focus), `always`, or `never`. |
| `idle_threshold_secs` | integer | `3` | Minimum seconds unfocused before a notification fires. |
| `events` | array | `["turn_complete", "approval_required"]` | Events that trigger notifications. Options: `turn_complete`, `approval_required`, `session_ready`, `task_complete`, `agent_error`. |
| `sleep_prevention` | bool | `true` | Keep the display awake while the agent works (macOS/Linux). |
| `progress_bar` | bool | `true` | Show a progress indicator in the terminal tab (OSC 9;4). |
| `title.enabled` | bool | `true` | Set the terminal title to reflect agent state. |
| `title.items` | array | (see above) | Items shown in the title bar. Options: `action-required`, `spinner`, `activity`, `session-name`, `cwd`, `model`, `turn-timer`, `grok`. |

#### Terminal support matrix

| Terminal | Auto Protocol | Focus Tracking | Progress Bar |
|----------|---------------|----------------|--------------|
| iTerm2 | OSC 9 | Yes | Yes |
| Kitty | OSC 99 | Yes | No |
| Ghostty | OSC 777 | Yes | Yes |
| WezTerm | OSC 9 | Yes | Yes |
| Warp | OSC 9 | Yes | No |
| Alacritty | BEL | Yes | No |
| VS Code | BEL | Yes | No |
| Apple Terminal | BEL | No | No |
| VTE (GNOME Terminal) | OSC 777 | Yes | No |
| Grok Desktop | None (native) | N/A | N/A |
| Unknown | BEL | No | No |

With `method = "auto"`, Grok detects the terminal brand and picks the best protocol. Set `method` explicitly to override that.

#### Notification hooks

Run your own commands when events fire. Hooks receive `$GROK_EVENT`, `$GROK_MESSAGE`, and `$GROK_SESSION_ID` in the environment.

```json
{
  "ui": {
    "notifications": {
      "hooks": [
        {
          "command": "terminal-notifier -title 'Grok' -message '$GROK_MESSAGE'",
          "events": [
            "turn_complete",
            "approval_required"
          ],
          "only_unfocused": true,
          "timeout_secs": 10
        },
        {
          "command": "curl -s -d '$GROK_MESSAGE' ntfy.sh/my-grok-alerts",
          "events": [
            "turn_complete"
          ],
          "only_unfocused": true,
          "timeout_secs": 10
        },
        {
          "command": "afplay /System/Library/Sounds/Glass.aiff",
          "events": [
            "turn_complete"
          ],
          "only_unfocused": true,
          "timeout_secs": 5
        }
      ]
    }
  }
}
```

| Hook Option | Type | Default | Description |
|-------------|------|---------|-------------|
| `command` | string | (required) | Shell command to run. |
| `events` | array | `[]` | Events that trigger this hook (empty = all events). |
| `only_unfocused` | bool | `true` | Only fire when the terminal has lost focus. |
| `timeout_secs` | integer | `10` | Kill the hook process after this many seconds. |

#### Troubleshooting

Run `/doctor` in the affected session. It shows the detected notification and focus issues, the relevant configuration file, and the steps to resolve them. An explicit `method = "bel"` is treated as intentional. `method = "none"` turns off notification and focus findings.

**Sleep prevention not taking effect:** on macOS, sleep prevention uses `IOPMAssertionCreateWithName` via CoreFoundation. On Linux, `systemd-inhibit` (which must be on `$PATH`). Make sure the relevant tool is available. Prevention is only active during agent turns and releases automatically when the turn ends.

### Status line

An optional row at the bottom of the full-screen pager, disabled by default. Opt in with `[ui.status_line]`:

```json
{
  "ui": {
    "status_line": {
      "items": [
        "cwd",
        "model",
        "context"
      ],
      "type": "builtin"
    }
  }
}
```

The other keys are `items` (which built-in segments to show, in order), `command`, `padding`, and `refresh_interval` (in seconds. Re-runs a `command` row on a timer, so an incident page or a CI status reaches an idle session). The [Status Line guide](25-status-line.md) documents all of them, along with the JSON contract a `command` script reads on stdin and an example script.

### Keyboard shortcuts

Keyboard shortcuts are **not** configurable — all bindings are built in. See [Keyboard Shortcuts](03-keyboard-shortcuts.md) for the complete reference.

### Telemetry

These are independent knobs (see [Monitoring Usage](24-monitoring-usage.md#related-settings)):

- **`[features] telemetry`** / `GROK_TELEMETRY_ENABLED` — the product-analytics master switch. `/privacy` does not change it.
- **Coding data, retention, and training** — the Settings row `/privacy` opens. Coding-data sharing, separate from telemetry.
- **`[telemetry] trace_upload`** / `GROK_TELEMETRY_TRACE_UPLOAD` — session traces. Follows telemetry when unset.
- **`[telemetry] otel_*`** / `GROK_EXTERNAL_OTEL` — external OTEL to your own collector (below).

When telemetry is on, enterprises running their own collector can redirect it or turn parts off under `[telemetry]`:

```json
{
  "telemetry": {
    "events_api_key": "your-collector-token",
    "events_url": "https://telemetry.your-company.com/events",
    "mixpanel_enabled": false,
    "trace_upload": false
  }
}
```

Set these only to point telemetry at your own infrastructure or to switch parts off. The built-in endpoint and credentials are managed by Grok — leave them unset to use the defaults.

The same `[telemetry]` table also configures the **external OpenTelemetry stream**, an independent opt-in (it does not require the telemetry toggle above) that ships a curated, content-free usage schema. This is to your *own* OTLP collector. Collector auth comes from `OTEL_EXPORTER_OTLP_HEADERS` and is never stored on disk. See [Monitoring & Usage](24-monitoring-usage.md) for the full schema, env vars, and privacy model.

```json
{
  "telemetry": {
    "otel_certificate": "/etc/ssl/corp-ca.pem",
    "otel_client_certificate": "/etc/ssl/client.crt",
    "otel_client_key": "/etc/ssl/client.key",
    "otel_enabled": true,
    "otel_endpoint": "https://collector.corp.example:4318",
    "otel_log_assistant_responses": false,
    "otel_log_tool_content": false,
    "otel_log_tool_details": true,
    "otel_log_user_prompts": false,
    "otel_logs_exporter": "otlp",
    "otel_metrics_exporter": "otlp",
    "otel_protocol": "http/protobuf"
  }
}
```

Listed `[telemetry] otel_*` keys in signed `requirements.toml` **pin** over process env (destination lock). `managed_config.toml` does not. There is no `headers` key — collector tokens stay in `OTEL_EXPORTER_OTLP_HEADERS`. See [Monitoring & Usage](24-monitoring-usage.md).

### Removed keys

Grok has no built-in updater. To update, run the installer again. These `[cli]` keys still load. As a result, an old `config.json` keeps working. However, they have no effect: `auto_update`, `channel`, `dismissed_version`, `npm_registry`, `minimum_version`, `maximum_version`, `required_minimum_version` and `required_maximum_version`. The `GROK_*_VERSION` environment overrides for those bounds are ignored too.

`[ui] screen_mode` also still loads and has no effect. Grok has one render mode. `[ui] alt_screen` decides whether it draws on the alternate screen.

### Enterprise deployment

A complete config for enterprise use:

```json
{
  "features": {
    "telemetry": false
  },
  "login": {
    "auth_provider_command": "/usr/local/bin/my-company-auth-provider",
    "auth_provider_label": "Acme Corp",
    "auth_token_ttl": 3600
  },
  "models": {
    "default": "company-grok",
    "definitions": {
      "company-grok": {
        "base_url": "https://grok-proxy.acme.com/",
        "context_window": 128000,
        "model": "grok-4.6",
        "name": "Grok 4.6 (Proxy)"
      }
    }
  }
}
```

---

## pager.toml (appearance configuration)

Location: `~/.grok/pager.toml`. This controls the TUI's look and feel. Changes apply on restart.

### Terminal

```toml
[terminal]
alt_screen = "auto"                   # fullscreen mode: "auto", "always", "never"
```

- `auto` (default): use the alternate screen when the terminal supports it.
- `always`: always use the alternate screen.
- `never`: run inline in the terminal's main scrollback buffer.

### Animation

```toml
[animation]
fps = 30                              # animation frame rate (ticks per second)
wave_rows = 32                        # rows per wave cycle for accent animation
```

### Prompt

```toml
[prompt]
collapse_unfocused = true             # collapse prompt when scrollback is focused
mouse_hover = true                    # show hover highlight on the prompt widget
show_prefix = true                    # show the prompt prefix character
```

Compact mode is not persisted here — control it at runtime with `[ui] compact_mode` or the `/compact-mode` command.

### Scrollback

```toml
[scrollback.layout]
outer_vpad = 1                        # vertical padding
outer_hpad_left = 2                   # left horizontal padding
outer_hpad_right = 2                  # right horizontal padding
block_pad_left = 2                    # padding inside block, left of content
block_pad_right = 2                   # padding inside block, right of content

[scrollback.scrollbar]
enabled = true                        # show scrollbar
gap_left = 0                          # gap between content and scrollbar
gap_right = 0                         # gap between scrollbar and screen edge

[scrollback.scroll]
margin = 0                            # minimum context lines above/below selection
min_page_fraction = 0                 # minimum scroll as % of viewport (0-100)
follow_indicator = "center"           # ▼/▲ scroll indicators: "center" or "none"
follow_auto_select = true             # auto-select latest entry in follow mode
follow_by_overscroll = true           # scrolling past bottom engages follow mode
anchor_on_fold = true                 # keep block position when folding
respect_manual_folds = true           # opt-in (default: false): keep manually folded blocks as-is during streaming/finish; expanding while following stops auto-scroll

[scrollback.display]
sticky_headers = true                 # pin user prompts as sticky headers
tab_width = 4                         # spaces per tab character
expandable_indicator = true           # show expand indicator on foldable entries
expandable_indicator_running = true   # show indicator on running entries
expandable_indicator_char = "›"       # character for the expand indicator (default: "›")
selection_buttons = false             # show copy/view buttons on selection
line_under_last_entry = false         # horizontal line below last entry
group_selection_split = true          # split selection box for expanded blocks
highlight_overlays_border = false     # highlight extends over selection box border
dim_accent = 0.5                      # dimming factor for collapsed accents (0.0-1.0)
```

`respect_manual_folds` is off by default. Turn it on and a block you fold by hand is pinned: streaming updates. Finish events (a thinking block ending, say) leave its fold state alone, and expanding a block while follow-mode tails new content stops the auto-scroll. As a result, the view stays put. Follow resumes via `Shift+G`, `j` at the last entry, scrolling past the bottom, or sending a new prompt. `Shift+E` clears all pins.

`Ctrl+E` clears pins on thinking blocks.

### Block configuration

```toml
[scrollback.blocks.edit]
indent = true                         # indent diff content
vpad = false                          # vertical padding
# expanded_by_default = true          # Unset follows Collapsed edit blocks. When that setting is on,
                                      # edits start collapsed even if this line is true.
dual_line_numbers = false             # two-column line numbers (old + new)
# line_summary = false                # show +N/-M in the collapsed header; unset follows the same flag
hunk_separator = "…"                  # separator between diff hunks (default: "…")

[scrollback.blocks.prompt]
vpad = true                           # vertical padding
show_prefix = true                    # show prompt prefix character
min_lines = 2                         # minimum content lines in sticky mode

[scrollback.blocks.thinking]
animate = true                        # animated accent while thinking
truncated_lines = 3                   # lines in truncated mode
```

### Plugins

```json
{
  "disable_plugins": false
}
```

---

## Environment variables

The key ones. See the README for the complete list.

### Authentication

| Variable | Description |
|----------|-------------|
| `XAI_API_KEY` | API key from console.example.invalid |
| `GROK_AUTH_PROVIDER_COMMAND` | External auth binary path |
| `GROK_AUTH_PROVIDER_LABEL` | Display name on TUI login screen |
| `GROK_AUTH_TOKEN_TTL` | Token lifetime in seconds |
| `GROK_AUTH_EARLY_INVALIDATION_SECS` | Seconds before expiry to refresh (default: 300) |
| `GROK_OIDC_ISSUER` | OIDC issuer URL |
| `GROK_OIDC_CLIENT_ID` | OIDC client ID |

### Models

| Variable | Description |
|----------|-------------|
| `GROK_MODEL_<SLOT>` | Pin one harness model slot. See [Harness model slots](#harness-model-slots) |
| `GROK_PROMPT_SUGGESTIONS_MODEL` | The `prompt_suggestion` slot, under its older name |

### Endpoints

| Variable | Description |
|----------|-------------|
| `GROK_CLI_CHAT_PROXY_BASE_URL` | API proxy base URL. No default: unset, nothing is sent to a proxy |
| `GROK_XAI_API_BASE_URL` | xAI API base URL. No default |
| `GROK_ALLOWED_ENDPOINTS` | Comma-separated endpoints a model request may reach, added to `[endpoints] allowed_endpoints` |

### Allowed endpoints

Grok sends a model request only to an endpoint you list. This covers chat and completion calls, model listings, web search, image and video generation, embeddings, voice and a pricing catalog you configure. The list starts empty, so a fresh config reaches nothing but the default pricing catalog (below):

```json
{
  "endpoints": {
    "allowed_endpoints": [
      "api.example.com",
      "localhost:11434",
      "*.corp.example",
      "https://gateway.example.com/v1"
    ]
  }
}
```

| Entry | Allows |
|-------|--------|
| `api.example.com` | Any port and path on this host |
| `localhost:11434` | This host and port only |
| `*.corp.example` | Every subdomain of `corp.example` |
| `https://gateway.example.com/v1` | This scheme, host, port and path prefix |

A refused request is never sent. Its error names the entry to add. Grok has one built-in endpoint: the pricing catalog. Every other model, tool or service with no URL you configured has no URL at all. The bash tool is not affected: a `curl` it runs goes where you point it.

### Pricing catalog

A model whose endpoint reports no cost (Anthropic, for one) is priced from `https://modelinfo.pazer.ai`. The request is a `GET` that carries only the model id. It needs no allowlist entry.

```json
{
  "models": {
    "pricing": {
      "catalog_url": "https://modelinfo.pazer.ai",
      "lookup_enabled": true
    }
  }
}
```

A `[model.<id>] pricing` table you write always wins over the catalog.

### Features

| Variable | Description |
|----------|-------------|
| `GROK_MEMORY` | Enable (`1`) or disable (`0`) cross-session memory |
| `GROK_SUBAGENTS` | Enable (`1`) or disable (`0`) subagents |
| `GROK_WORKFLOWS` | Enable (`1`) or disable (`0`) background workflows |
| `GROK_WEB_FETCH` | Enable (`1`) or disable (`0`) the web_fetch tool |
| `GROK_WEB_FETCH_ALLOW_LOCAL` | Allow `web_fetch` to explicit loopback hosts only (`localhost` / `127.0.0.0/8` / `::1`). Same as `[toolset.web_fetch] allow_local`. Default off; private/metadata stay blocked. |
| `GROK_AGENT` | Custom agent definition path or name |
| `GROK_SANDBOX` | Sandbox profile (off, workspace, devbox, read-only, strict; or a custom profile name) |
| `GROK_EXIT_TIMEOUT_SECS` | Seconds after a quit is requested before the process is force-exited if teardown hangs (default: 20, `0` disables; a hard exit follows 5s later) |

### Logging

| Variable | Description |
|----------|-------------|
| `GROK_LOG_FILE` | Write logs to this file path (used verbatim as the path) |
| `RUST_LOG` | Log level filter (e.g. `debug`); controls the `GROK_LOG_FILE` log and headless stderr output |

### Paths

| Variable | Description |
|----------|-------------|
| `GROK_HOME` | Override config directory (default: `~/.grok`) |
| `GROK_RESPECT_GITIGNORE` | Force gitignore filtering on (`1`) or off (`0`); overrides `[tools] respect_gitignore` |

### Telemetry

| Variable | Description |
|----------|-------------|
| `GROK_TELEMETRY_ENABLED` | Enable/disable telemetry |
| `GROK_TELEMETRY_TRACE_UPLOAD` | Enable/disable session trace upload |
| `GROK_TELEMETRY_MIXPANEL_ENABLED` | Enable/disable Mixpanel specifically |
| `GROK_EXTERNAL_OTEL` | External OTEL to your collector (see [24-monitoring-usage.md](24-monitoring-usage.md)) |
| `GROK_FEEDBACK_ENABLED` | Enable/disable feedback system |

---

## File locations

| Path | Description |
|------|-------------|
| `~/.grok/config.json` | Main configuration file |
| `~/.grok/pager.toml` | TUI appearance configuration |
| `~/.grok/auth.json` | Authentication credentials (auto-managed) |
| `~/.grok/sessions/` | Persisted sessions (organized by working directory) |
| `~/.grok/memory/` | Cross-session memory files and index |
| `~/.grok/skills/` | User-scoped skill definitions |
| `~/.grok/plugins/` | User-scoped plugins |
| `~/.grok/agents/` | User-scoped agent definitions |
| `~/.grok/lsp.json` | LSP server configuration (user-scoped) |
| `~/.grok/logs/` | Internal log files (e.g. `unified.jsonl`, MCP server logs) |
| `.grok/config.json` | Project-scoped MCP servers, plugins, and permission rules |
| `.grok/skills/` | Project-scoped skill definitions |
| `.grok/plugins/` | Project-scoped plugins |
| `.grok/agents/` | Project-scoped agent definitions |
| `.grok/hooks/` | Project-scoped hooks |
| `.grok/lsp.json` | LSP server configuration |

### How Grok saves `config.json`

Writes to **`~/.grok/config.json`** (`/settings` and other user-config saves) follow a leaf symlink. The atomic rename writes the referent (a file in your dotfiles repo). The link stays a link. If the link dangls, the write creates the referent as a regular file.

Writes to a **project** `.grok/config.json` (MCP / plugin / permission edits) **replace** a leaf symlink with a regular file. That keeps a later save from following the link out of the repository.

A user `config.json` that cannot be parsed is not overwritten. Fix the syntax (or restore a backup) and save again.

---

## Project-scoped configuration

Some settings can be set per-project by placing files in `.grok/` inside your repository:

| File | What it configures |
|------|--------------------|
| `.grok/config.json` | MCP servers, plugins, permission rules, and the `[mcp] max_output_bytes` tool-result cap (other sections load only from `~/.grok/config.json`) |
| `.grok/skills/` | Project-specific skills |
| `.grok/hooks/` | Project-specific lifecycle hooks |
| `.grok/agents/` | Project-specific agent definitions |
| `.grok/lsp.json` | LSP server configuration |
| `.grok/sandbox.json` | Custom sandbox profiles |
| `AGENTS.md` | Project instructions (system prompt) |

Project-scoped MCP servers override global ones with the same name (full replacement, not a merge).

---

## LSP servers

Language servers power passive diagnostics and the optional `lsp` tool (see the [`lsp_tools`](#general-settings) feature flag). Definitions come from multiple sources and merge by server name:

| Source | Location | Scope |
|--------|----------|-------|
| User | `~/.grok/lsp.json` | All projects |
| Project | `.grok/lsp.json` | Current repository |
| Plugin | A trusted plugin's `.lsp.json` file, or an inline `lspServers` block in its `plugin.json` | Wherever the plugin is enabled |

When the same server name comes from more than one source, it resolves highest-priority first:

1. **Project** — `.grok/lsp.json`
2. **User** — `~/.grok/lsp.json`
3. **Plugins** — file-based `.lsp.json`, then inline `lspServers`, in plugin load order

Project and user entries replace lower-priority ones of the same name. Plugin entries only add servers whose names are not already defined by a local file. Do this so a local `lsp.json` always wins over a plugin. Plugin LSP servers load only after the plugin is trusted (see [Plugins](09-plugins.md)).
