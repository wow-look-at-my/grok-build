# Configuration reference

This file ships with the CLI and is extracted to `~/.grok/docs/user-guide/26-config-reference.md` on launch. It is the complete field list for `config.json`, managed config, and requirements. For conceptual guidance see [05-configuration.md](05-configuration.md).

## How to configure

Config files are JSON. `~/.grok/config.schema.json` (written by grok on launch) is the JSON Schema for them. `config.json` names it with `"$schema": "./config.schema.json"`, so an editor that reads JSON Schema completes and checks every key below. Each key is a dotted path: `mcp.servers.<name>.command` is `{"mcp": {"servers": {"<name>": {"command": ...}}}}`.

Different people write kinds of file configure Grok Build, and they.

| File | Who writes it | Where it lives | Use it to |
| --- | --- | --- | --- |
| `config.json` | The developer | `~/.grok/config.json`, and `.grok/config.json` in a project | Set personal defaults. Anything here can be changed by the person using the machine. |
| managed config | You, through the console or a deployment tool | `/etc/grok/managed_config.json`, or the console-synced `$GROK_HOME/managed_config.toml` | Ship a starting point to a fleet. A developer's own file overrides it. |
| requirements | You, signed | `/etc/grok/requirements.json`, macOS device management, or the console-synced, signed `$GROK_HOME/requirements.toml` | Set values a developer cannot change. Keys marked `pin` below hold against every other file, the environment, and the command line. |

Choose managed config for defaults you want people to be able to adjust, and requirements for the ones you do not. `xai_grok_config::layout::LAYOUT` maps each JSON section to its internal name. A legacy `/etc/grok/*.toml` file is still read when no `.json` file sits next to it.

Grok Build also reads these layers, later rows winning except where a requirements pin or the Managed column says otherwise.

1. Compiled defaults.
2. `/etc/grok/managed_config.json`, then `$GROK_HOME/managed_config.toml` (fleet defaults, console-synced).
3. `$GROK_HOME/config.json` (your settings. `/settings` writes here). Default `$GROK_HOME` is `~/.grok`. A legacy `config.toml` there converts to `config.json` on launch and is kept as `config.toml.migrated`.

5. `GROK_CONFIG` (inline JSON) or `GROK_CONFIG_PATH` (JSON or TOML file). Allowlisted keys only.
6. `$GROK_HOME/requirements.toml`, then `/etc/grok/requirements.json`, then macOS MDM `ai.x.grok`. Admin layer. Keys marked `pin` in the table cannot be overridden. Keys marked `yes` are also valid in this file.
7. `GROK_*` environment variables.
8. CLI flags such as `--model`, `--sandbox`, `--yolo`.

Run `grok inspect` or `grok inspect --json` to see which files and values won.

## config.json

User-level configuration lives in `$GROK_HOME/config.json` (default `~/.grok/config.json`. Windows `%USERPROFILE%\.grok\config.json`).

**Requirements** marks whether the same key can be set in requirements: `pin` cannot be overridden (including env and CLI where the resolver honors the pin). `yes` is accepted in that file. `—` is not read from requirements. **Managed** marks whether a fleet managed-config value stands (`fleet`) or the user's file wins (`user`).

### `agents`

| Key | Type / Values | Requirements | Managed | Details |
| --- | --- | --- | --- | --- |
| `agents.auto_mode.enabled` | `boolean` | `yes` | `user` | Enable Auto permission mode. |
| `agents.default_auto_mode` | `boolean` | `yes` | `user` | Start sessions in auto permission mode when no per-session override is set. |
| `agents.doom_loop_recovery.enabled` | `boolean` | `yes` | `user` | Resample confident tool-call loops; set false to disable. |
| `agents.goal.enabled` | `boolean` | `yes` | `user` | Enable `/goal`. |
| `agents.long_reasoning_reminder.delay` | `integer` | `yes` | `user` | Model calls to wait after the long call before the reminder. Default 1, clamped to 0–10. Also `delay` in the `GROK_LONG_REASONING_REMINDER` JSON object. |
| `agents.long_reasoning_reminder.enabled` | `boolean` | `yes` | `user` | Inject a mid-turn reminder to reason briefly after a model call with long hidden reasoning. Default false. Also `GROK_LONG_REASONING_REMINDER` (a bool word, or a JSON object in this table's shape). |
| `agents.long_reasoning_reminder.tokens` | `integer` | `yes` | `user` | Reasoning tokens in one model call that count as long. Default 1000, clamped to 100–200000. Also `tokens` in the `GROK_LONG_REASONING_REMINDER` JSON object. |
| `agents.prompt_suggestions.max_output_tokens` | `number` | `yes` | `user` | Visible-output tokens for the suggestion call; clamped to 16–256, default 64, with a separate reserve for reasoning. Remote-overridable. |
| `agents.prompt_suggestions.reasoning_effort` | `none / minimal / low / medium / high` | `yes` | `user` | Reasoning effort for the suggestion call; default and `none` disable reasoning, while other values use a supported model effort. Remote-overridable. |
| `agents.prompt_suggestions.temperature` | `number` | `yes` | `user` | Sampling temperature for the suggestion call (default 0.2). Remote-overridable. |
| `agents.selection.definition` | `string (path)` | `yes` | `user` | Path to an agent definition markdown file with YAML frontmatter. |
| `agents.selection.name` | `string` | `yes` | `user` | Built-in or discovered agent definition name. Also GROK_AGENT and `--agent-profile`. |
| `agents.selection.system_prompt_label` | `string` | `yes` | `user` | Global system-prompt identity; per-model override wins. |
| `agents.subagents.enabled` | `boolean` | `pin` | `user` | Subagent / task tool master switch, default true even when other `agents.subagents.*` keys are set. Also GROK_SUBAGENTS or `--no-subagents`. |
| `agents.subagents.limit_behavior` | `queue / fail` | `yes` | `user` | What to do when the concurrent subagent cap is hit. |
| `agents.subagents.max_concurrent` | `integer` | `yes` | `user` | Max concurrent subagents. |
| `agents.subagents.max_depth` | `integer` | `yes` | `user` | Max nested subagent depth (clamped ≥1). |
| `agents.subagents.models.<name>` | `string` | `yes` | `user` | Per-subagent model id override. |
| `agents.subagents.toggle.<name>` | `boolean` | `yes` | `user` | Enable or disable an individual subagent type. Omitted agents default on. |
| `agents.workflows.enabled` | `boolean` | `yes` | `user` | Enable workflows. |

### `announcements`

| Key | Type / Values | Requirements | Managed | Details |
| --- | --- | --- | --- | --- |
| `announcements` | `array of tables` | `—` | `user` | Remote announcement payloads consumed at load. Not a user-authored table. |

### `campaigns`

| Key | Type / Values | Requirements | Managed | Details |
| --- | --- | --- | --- | --- |
| `campaigns` | `array of tables` | `yes` | `user` | Named campaign patches applied below requirements. The deployment publishes these. |

### `cli`

| Key | Type / Values | Requirements | Managed | Details |
| --- | --- | --- | --- | --- |
| `cli.auto_update` | `boolean` | `pin` | `user` | Still loads; has no effect. |
| `cli.channel` | `stable / alpha` | `pin` | `user` | Still loads; has no effect. |
| `cli.grove_worktree` | `boolean` or `grove` / `grove-fuse` / `grove-nfs` / `nfs` / `copy` / `true` / `false` / `1` / `0` / `on` / `off` | `yes` | `user` | Session / `-w` Grove vs copy. Default copy. Distinct from creation-mode `cli.worktree_type`. Also `GROK_WORKTREE_TYPE`. Layer order: request → env → local → enable-all (`GROK_GROVE` / `[cli] grove`) → remote-true; then kill last: remote `grove_worktree = false` → copy (`remote_kill`). Missing remote settings are not a kill: local/env/request/enable-all still apply. Does not enable `grok clone`. |
| `cli.grove` | `boolean` or `grove` / `grove-fuse` / `grove-nfs` / `nfs` / `all` / `copy` / `true` / `false` / `1` / `0` / `on` / `off` | `yes` | `user` | Convenience that turns **both** `grok clone` and session / `-w` Grove on when the specific knobs are unset. Also `GROK_GROVE`. `false` / `copy` / `off` means enable-all is off (fall through); it does not force both surfaces off. `[cli] grove_worktree` and `GROK_WORKTREE_TYPE` still win for worktrees; `GROK_CLONE` still wins for clone. Remote `grove_worktree = false` still kills worktrees only. |
| `cli.installer` | `string` | `—` | `user` | Still loads; has no effect. |
| `cli.maximum_version` | `string` | `pin` | `user` | Still loads; has no effect. |
| `cli.minimum_version` | `string` | `pin` | `user` | Still loads; has no effect. |
| `cli.nfs_worktree` | same as `cli.grove_worktree` | `yes` | `user` | Read alias of `cli.grove_worktree`. |
| `cli.npm_registry` | `string` | `yes` | `user` | Still loads; has no effect. |
| `cli.required_maximum_version` | `string` | `pin` | `user` | Still loads; has no effect. |
| `cli.required_minimum_version` | `string` | `pin` | `user` | Still loads; has no effect. |
| `cli.session_picker_grouped` | `boolean` | `yes` | `user` | Group sessions by repo in the picker and CLI listings. |
| `cli.session_registry` | `boolean` | `yes` | `user` | Participate in the cross-process session registry. |
| `cli.show_tips` | `boolean` | `pin` | `user` | Startup tips. |
| `cli.use_leader` | `boolean` | `pin` | `user` | Use the leader process for config reload and MCP watches. |
| `cli.worktree_type` | `string` | `yes` | `user` | Creation-mode when set to `linked`, `standalone`, or `git`. The spellings `grove`, `grove-fuse`, `grove-nfs`, `nfs`, and `copy` also feed the session / `-w` Grove gate (same as `cli.grove_worktree`); they are not creation-mode values. |

### `endpoints`

| Key | Type / Values | Requirements | Managed | Details |
| --- | --- | --- | --- | --- |
| `endpoints.cli_chat_proxy_base_url` | `string` | `pin` | `user` | Session-service API base URL. |
| `endpoints.feedback_base_url` | `string` | `yes` | `user` | Where feedback submissions go. Also GROK_FEEDBACK_BASE_URL. |
| `endpoints.managed_config_url` | `string` | `yes` | `user` | Override managed config endpoint. Also GROK_MANAGED_CONFIG_URL. |
| `endpoints.models_base_url` | `string` | `pin` | `user` | Custom inference base URL. Also GROK_MODELS_BASE_URL. |
| `endpoints.models_list_url` | `string` | `pin` | `user` | Override model-list URL. Also GROK_MODELS_LIST_URL. Alias `models_endpoint`. |
| `endpoints.trace_upload_url` | `string` | `pin` | `user` | Proxy destination for traces. Also GROK_TRACE_UPLOAD_URL. Trace uploads go through the proxy only: a config that sets a removed direct-bucket key (`trace_upload_bucket`, `trace_upload_region`, `trace_upload_credentials`, `trace_upload_credentials_file`, `trace_upload_endpoint_url`, `gcs_service_account_key`) or env var (`GROK_TRACE_UPLOAD_BUCKET`, `GROK_TRACE_UPLOAD_REGION`, `GROK_TRACE_UPLOAD_CREDENTIALS_FILE`, `GROK_TRACE_UPLOAD_ENDPOINT_URL`, `GROK_TELEMETRY_GCS_BUCKET`) fails to load. |
| `endpoints.xai_api_base_url` | `string` | `pin` | `user` | Public xAI API base. Also GROK_XAI_API_BASE_URL. |

### `extensions`

| Key | Type / Values | Requirements | Managed | Details |
| --- | --- | --- | --- | --- |
| `extensions.compat.claude.agents` | `boolean` | `yes` | `user` | Scan CLAUDE.md. Also GROK_CLAUDE_AGENTS_ENABLED. |
| `extensions.compat.claude.hooks` | `boolean` | `yes` | `user` | Scan Claude hooks. Also GROK_CLAUDE_HOOKS_ENABLED. |
| `extensions.compat.claude.mcps` | `boolean` | `yes` | `user` | Scan Claude MCP config. Also GROK_CLAUDE_MCPS_ENABLED. |
| `extensions.compat.claude.rules` | `boolean` | `yes` | `user` | Scan Claude rules. Also GROK_CLAUDE_RULES_ENABLED. |
| `extensions.compat.claude.skills` | `boolean` | `yes` | `user` | Scan Claude skills. Also GROK_CLAUDE_SKILLS_ENABLED. |
| `extensions.compat.cursor.agents` | `boolean` | `yes` | `user` | Scan agent definitions from Cursor compat sources. Also GROK_CURSOR_AGENTS_ENABLED. |
| `extensions.compat.cursor.hooks` | `boolean` | `yes` | `user` | Scan Cursor hooks. Also GROK_CURSOR_HOOKS_ENABLED. |
| `extensions.compat.cursor.mcps` | `boolean` | `yes` | `user` | Scan Cursor mcp.json. Also GROK_CURSOR_MCPS_ENABLED. |
| `extensions.compat.cursor.rules` | `boolean` | `yes` | `user` | Scan `.cursor/rules/`. Also GROK_CURSOR_RULES_ENABLED. |
| `extensions.compat.cursor.skills` | `boolean` | `yes` | `user` | Scan Cursor skills directories. Also GROK_CURSOR_SKILLS_ENABLED. |
| `extensions.marketplace.require_sha` | `boolean` | `yes` | `user` | Tighten-only: remote plugin installs and updates must pin a full commit sha. Also `GROK_MARKETPLACE_REQUIRE_SHA`. Neither this key nor the env var can turn the gate back off. |
| `extensions.marketplace.sources` | `array of tables` | `yes` | `user` | `extensions.marketplace.sources` plugin marketplace repos. |
| `extensions.paths.extra_rule_dirs` | `string[]` | `yes` | `user` | More rule directories (absolute or `~/…`; each contains `*.md`), loaded after the home rules. |
| `extensions.paths.extra_skill_dirs` | `string[]` | `yes` | `user` | Records `/import-claude` skill directories that skill injection ignores in favor of `[skills] paths`. |
| `extensions.plugins.disabled` | `string[]` | `yes` | `user` | Plugin IDs to discover but not load. Project files may set this. |
| `extensions.plugins.enabled` | `string[]` | `yes` | `user` | Plugin IDs to enable; needed for project plugins that default off. |
| `extensions.plugins.paths` | `string[]` | `yes` | `user` | Additional plugin directories. Project files may set this when the folder is trusted. |
| `extensions.skills.disabled` | `string[]` | `yes` | `user` | Skill names to discover but not activate. |
| `extensions.skills.paths` | `string[]` | `yes` | `user` | Additional skill directories. |

### `features`

| Key | Type / Values | Requirements | Managed | Details |
| --- | --- | --- | --- | --- |
| `features.active_agent_messages` | `boolean` | `pin` | `user` | Enable or disable `active_agent_messages`. Default false. Also `GROK_ACTIVE_AGENT_MESSAGES`. |
| `features.ask_user_question` | `boolean` | `pin` | `user` | Enable or disable `ask_user_question`. Default true. Also `GROK_ASK_USER_QUESTION`. |
| `features.auto_wake` | `boolean` | `pin` | `user` | Enable or disable `auto_wake`. Default true. Also `GROK_AUTO_WAKE`. |
| `features.backend_tools` | `boolean` | `pin` | `user` | Enable or disable `backend_tools`. Default true. Also `GROK_BACKEND_SEARCH`. |
| `features.campaigns` | `boolean` | `yes` | `user` | Enable remote campaign patches. `GROK_CAMPAIGNS=0` still disables even when requirements set this true. |
| `features.cancel_rewind` | `boolean` | `pin` | `user` | Enable or disable `cancel_rewind`. Default true. Also `GROK_CANCEL_REWIND`. |
| `features.codebase_indexing` | `boolean / string[]` | `pin` | `user` | Codebase graph indexing; true indexes git repos, or pass include/exclude globs. |
| `features.compaction_detail` | `none / minimal / balanced / verbose` | `yes` | `user` | Verbatim detail level for `segments` compaction. Also GROK_COMPACTION_DETAIL. |
| `features.compaction_mode` | `summary / transcript / segments` | `yes` | `user` | Compaction strategy. Also GROK_COMPACTION_MODE. |
| `features.compaction_tool_choice` | `string` | `yes` | `user` | Tool-choice hint used during compaction. |
| `features.compaction_verbatim_input` | `boolean` | `pin` | `user` | Enable or disable `compaction_verbatim_input`. Default true. Also `GROK_COMPACTION_VERBATIM_INPUT`. |
| `features.dock` | `boolean` | `pin` | `user` | Enable or disable `dock`. Default false. Also `GROK_DOCK`. |
| `features.feedback_trace_card` | `boolean` | `pin` | `user` | Show a trace-upload consent question after `/feedback`. Default false. Also `GROK_FEEDBACK_TRACE_CARD`. |
| `features.feedback` | `boolean` | `pin` | `user` | Enable or disable `feedback`. Default true. Also `GROK_FEEDBACK_ENABLED`. |
| `features.image_edit_model_override` | `string` | `yes` | `user` | Imagine model id for image_edit. |
| `features.image_gen_model_override` | `string` | `yes` | `user` | Imagine model id for image_gen. Empty defers to the remotely configured default. |
| `features.image_gen` | `boolean` | `pin` | `user` | Enable image_gen / `/imagine`. |
| `features.lsp_tools` | `boolean` | `pin` | `user` | Enable or disable `lsp_tools`. Default false. Also `GROK_LSP_TOOLS`. |
| `features.managed_config` | `boolean` | `yes` | `user` | Fetch managed_config.toml and requirements.toml from the deployment. |
| `features.mcp_auto_restart` | `boolean` | `yes` | `user` | Auto-restart stdio MCP servers after transport failure. Also GROK_MCP_AUTO_RESTART. |
| `features.mcp_liveness_watchers` | `boolean` | `yes` | `user` | Poll MCP transports and push server_status updates. Emergency kill switch when false. |
| `features.mcp_push_server_status` | `boolean` | `yes` | `user` | Pager subscribes to MCP server_status push. Process env GROK_MCP_PUSH_SERVER_STATUS wins at launch. |
| `features.mcp_recursive_config_watch` | `boolean` | `yes` | `user` | Watch `<cwd>/` and `<cwd>/.grok/` for project MCP config edits. Name is a misnomer; watches are non-recursive. |
| `features.non_git_warning` | `boolean` | `yes` | `user` | Show a blocking warning when Grok starts outside a Git repository. |
| `features.remember_mode` | `boolean` | `—` | `—` | Remember the last permission mode across sessions. Read from user `config.toml` only. |
| `features.remote_fetch` | `boolean` | `pin` | `fleet` | Pin remote model-catalog and asset fetch. Managed wins over the user file when both set. |
| `features.session_recap` | `boolean` | `pin` | `user` | Enable or disable `session_recap`. Default true. Also `GROK_SESSION_RECAP`. |
| `features.session_search` | `boolean` | `pin` | `user` | Enable or disable `session_search`. Default true. Also `GROK_SESSION_SEARCH`. |
| `features.subagent_model_inheritance` | `boolean` | `pin` | `user` | Hide the subagent `model` argument when every model you can pick is an xAI model, so subagents inherit the parent's model. Default false. Also `GROK_SUBAGENT_MODEL_INHERITANCE`. Read when a session starts; changing it requires a restart. |
| `features.subagent_worktree_snapshot` | `boolean` | `pin` | `user` | Enable or disable `subagent_worktree_snapshot`. Default false. Also `GROK_SUBAGENT_WORKTREE_SNAPSHOT`. |
| `features.support_permission` | `boolean` | `yes` | `user` | Allow the agent to ask permission for tool executions. |
| `features.telemetry` | `boolean / session_metrics / off` | `pin` | `user` | Product telemetry mode. Enterprise default is off. |
| `features.terminal_theme` | `boolean` | `pin` | `user` | Reveal the terminal-native `terminal` color theme during its rollout. Default false. Also `GROK_TERMINAL_THEME`. |
| `features.title_refresh` | `boolean` | `pin` | `user` | Early-session auto-title refresh. Pin this in requirements to beat GROK_TITLE_REFRESH. |
| `features.turn_summary` | `boolean` | `pin` | `user` | Enable or disable `turn_summary`. Default true. Also `GROK_TURN_SUMMARY`. |
| `features.two_pass_compaction` | `boolean` | `pin` | `user` | Enable or disable `two_pass_compaction`. Default true. Also `GROK_TWO_PASS_COMPACTION`. Also a `/settings` → Agent row ("Two-pass compaction"); changing it requires a restart. |
| `features.video_gen` | `boolean` | `pin` | `user` | Enable video tools / `/imagine-video`. |
| `features.voice_mode` | `boolean` | `pin` | `user` | Enable or disable `voice_mode`. Default true. Also `GROK_VOICE_MODE`. |
| `features.web_fetch` | `boolean` | `pin` | `user` | Enable or disable `web_fetch`. Default false. Also `GROK_WEB_FETCH`. |
| `features.write_file` | `boolean` | `pin` | `user` | Enable or disable `write_file`. Default true. Also `GROK_WRITE_FILE`. |
| `features.zdr_access_enabled` | `boolean` | `pin` | `user` | Advertise ZDR-incompatible tools when the team is on Zero Data Retention. Also `GROK_ZDR_ACCESS_ENABLED`. |

### `hooks`

| Key | Type / Values | Requirements | Managed | Details |
| --- | --- | --- | --- | --- |
| `hooks.<event>[].hooks[].command` | `string` | `yes` | `user` | Command to run for this hook. `$VAR` is not expanded at load. |
| `hooks.<event>[].hooks[].type` | `command` | `yes` | `user` | Hook handler type. Command hooks are supported. |
| `hooks.<event>[].matcher` | `string` | `yes` | `user` | Tool-name matcher for this hook group. |
| `hooks.<event>` | `array of tables` | `yes` | `user` | Matcher groups for a lifecycle event such as PreToolUse or Stop. See Hooks. |

### `login`

| Key | Type / Values | Requirements | Managed | Details |
| --- | --- | --- | --- | --- |
| `login.auth_provider_command` | `string` | `yes` | `user` | External auth binary; stdout is the token. Also GROK_AUTH_PROVIDER_COMMAND. |
| `login.auth_provider_label` | `string` | `yes` | `user` | Login button label for an external auth provider. Also GROK_AUTH_PROVIDER_LABEL. |
| `login.auth_token_ttl` | `number` | `yes` | `user` | Token TTL in seconds for providers that return a bare token. Also GROK_AUTH_TOKEN_TTL. |
| `login.disable_api_key_auth` | `boolean` | `pin` | `user` | Refuse API-key auth so only the deployment IdP can log in. Also GROK_DISABLE_API_KEY_AUTH. |
| `login.force_login_team_uuid` | `string / string[]` | `pin` | `user` | Require login to this team UUID, or any of an array; empty array fails closed. Also GROK_FORCE_LOGIN_TEAM_ID. |
| `login.grok_ws_origin` | `string` | `yes` | `user` | Websocket origin for grok.invalid. Also GROK_WS_ORIGIN. |
| `login.grok_ws_url` | `string` | `yes` | `user` | Relay websocket URL. Also GROK_WS_URL. |
| `login.oauth2.client_id` | `string` | `yes` | `user` | OAuth2 client id. Also GROK_OAUTH2_CLIENT_ID. |
| `login.oauth2.issuer` | `string` | `yes` | `user` | OAuth2 issuer URL. Also GROK_OAUTH2_ISSUER. |
| `login.oauth2.principal_id` | `string` | `yes` | `user` | Required principal id when `principal_type` is set. Also GROK_OAUTH2_PRINCIPAL_ID. |
| `login.oauth2.principal_type` | `string` | `yes` | `user` | Token principal type, such as Team. Also GROK_OAUTH2_PRINCIPAL_TYPE. |
| `login.oauth2.referrer` | `string` | `yes` | `user` | Referrer for OAuth usage attribution. Also GROK_OAUTH2_REFERRER. |
| `login.oauth2.scopes` | `string[]` | `yes` | `user` | OAuth2 scopes. Also GROK_OAUTH2_SCOPES. |
| `login.oauth2` | `table` | `yes` | `user` | OAuth2 provider used when enterprise OIDC is unset. |
| `login.oidc.audience` | `string` | `yes` | `user` | Optional OIDC audience. Also GROK_OIDC_AUDIENCE. |
| `login.oidc.client_id` | `string` | `yes` | `user` | OIDC client id. Also GROK_OIDC_CLIENT_ID. |
| `login.oidc.issuer` | `string` | `yes` | `user` | OIDC issuer URL. Also GROK_OIDC_ISSUER. |
| `login.oidc.scopes` | `string[]` | `yes` | `user` | OIDC scopes. Also GROK_OIDC_SCOPES. |
| `login.oidc` | `table` | `yes` | `user` | Customer OIDC identity-provider settings. |
| `login.preferred_method` | `api_key / oidc` | `yes` | `user` | Pin automatic auth to one method with no fallthrough. |
| `login.token_header` | `string` | `yes` | `user` | Header name that carries the CLI auth token; default `xai-grok-cli`. |
| `login` | `table` | `yes` | `user` | grok.invalid websocket and OAuth/OIDC settings. `login` is an alias. |

### `mcp`

| Key | Type / Values | Requirements | Managed | Details |
| --- | --- | --- | --- | --- |
| `mcp.disabled_servers` | `string[]` | `yes` | `user` | MCP server names to skip without deleting their `mcp.servers` blocks. |
| `mcp.disabled_tools` | `map<string, string[]>` | `yes` | `user` | Per-server MCP tool deny lists keyed by server name. |
| `mcp.managed.enabled` | `boolean` | `pin` | `user` | Fetch managed MCP configs at startup. Also GROK_MANAGED_MCPS_ENABLED. |
| `mcp.managed.gateway_tools_enabled` | `boolean` | `yes` | `user` | Expose managed MCP gateway tools. Also GROK_MANAGED_MCP_GATEWAY_TOOLS_ENABLED. |
| `mcp.max_output_bytes` | `number` | `yes` | `user` | Cap MCP tool output size in bytes. Project files may set this. |
| `mcp.servers.<name>.args` | `string[]` | `yes` | `user` | `mcp.servers.<name>` `args` on a stdio or HTTP MCP server. |
| `mcp.servers.<name>.bearer_token_env_var` | `string` | `yes` | `user` | `mcp.servers.<name>` `bearer_token_env_var` on a stdio or HTTP MCP server. |
| `mcp.servers.<name>.command` | `string` | `yes` | `user` | `mcp.servers.<name>` `command` on a stdio or HTTP MCP server. |
| `mcp.servers.<name>.cwd` | `string` | `yes` | `user` | `mcp.servers.<name>` `cwd` on a stdio or HTTP MCP server. |
| `mcp.servers.<name>.enabled` | `boolean` | `yes` | `user` | `mcp.servers.<name>` `enabled` on a stdio or HTTP MCP server. |
| `mcp.servers.<name>.env` | `table` | `yes` | `user` | `mcp.servers.<name>` `env` on a stdio or HTTP MCP server. |
| `mcp.servers.<name>.expose_image_base64` | `boolean` | `yes` | `user` | `mcp.servers.<name>` `expose_image_base64` on a stdio or HTTP MCP server. |
| `mcp.servers.<name>.headers` | `table` | `yes` | `user` | `mcp.servers.<name>` `headers` on a stdio or HTTP MCP server. |
| `mcp.servers.<name>.oauth_client_id` | `string` | `yes` | `user` | `mcp.servers.<name>` `oauth_client_id` on a stdio or HTTP MCP server. |
| `mcp.servers.<name>.oauth_client_secret_env_var` | `string` | `yes` | `user` | `mcp.servers.<name>` `oauth_client_secret_env_var` on a stdio or HTTP MCP server. |
| `mcp.servers.<name>.oauth_scopes` | `string[]` | `yes` | `user` | `mcp.servers.<name>` `oauth_scopes` on a stdio or HTTP MCP server. |
| `mcp.servers.<name>.oauth` | `table` | `yes` | `user` | `mcp.servers.<name>` `oauth` on a stdio or HTTP MCP server. |
| `mcp.servers.<name>.setup` | `table` | `yes` | `user` | `mcp.servers.<name>` `setup` on a stdio or HTTP MCP server. |
| `mcp.servers.<name>.startup_timeout_sec` | `number` | `yes` | `user` | `mcp.servers.<name>` `startup_timeout_sec` on a stdio or HTTP MCP server. |
| `mcp.servers.<name>.tool_timeout_sec` | `number` | `yes` | `user` | `mcp.servers.<name>` `tool_timeout_sec` on a stdio or HTTP MCP server. |
| `mcp.servers.<name>.tool_timeouts` | `table` | `yes` | `user` | `mcp.servers.<name>` `tool_timeouts` on a stdio or HTTP MCP server. |
| `mcp.servers.<name>.type` | `string` | `yes` | `user` | `mcp.servers.<name>` `type` on a stdio or HTTP MCP server. |
| `mcp.servers.<name>.url` | `string` | `yes` | `user` | `mcp.servers.<name>` `url` on a stdio or HTTP MCP server. |

### `memory`

| Key | Type / Values | Requirements | Managed | Details |
| --- | --- | --- | --- | --- |
| `memory.enabled` | `boolean` | `pin` | `user` | Memory switch. Also `GROK_MEMORY`. `false` turns memory off unless `memory_v2.enabled = true`. With v2 off, `true` turns legacy memory on. |
| `memory.v2.archived_retention_days` | `number` | — | `user` | Retains archived memory-v2 observation files for this many days. Default: `30`. |
| `memory.v2.automatic_dream_enabled` | `boolean` | — | `user` | Enables event-driven memory-v2 Dream. Default: `true`. |
| `memory.v2.capture_enabled` | `boolean` | — | `user` | Enables memory-v2 extraction and observation capture. Default: `true`. |
| `memory.v2.capture_status_enabled` | `boolean` | — | `user` | Shows memory-v2 capture lifecycle messages in the UI for debugging. Successful captures are expandable and include generated content plus links to committed observation files. Telemetry and debug logs are always recorded. Default: `false`. |
| `memory.v2.enabled` | `boolean` | `pin` | `user` | Memory-v2 switch. True or absent selects v2. `false` selects the legacy pipeline, which then follows `memory.enabled`. Default: `true`. |
| `memory.v2.file_writes_enabled` | `boolean` | — | `user` | Enables all memory-v2 file mutation; `false` fails closed before scaffold creation. Default: `true`. |
| `memory.v2.job_retention_days` | `number` | — | `user` | Retains terminal memory-v2 capture-job metadata for this many days. Default: `14`. |
| `memory.v2.manual_dream_enabled` | `boolean` | — | `user` | Enables explicitly requested memory-v2 Dream. Default: `true`. |
| `memory.v2.rollout` | `"off"`, `"record_only"`, `"shadow"`, `"active"` | — | `user` | Advanced staged-rollout control for new v2 sessions. Default: `"active"`. Most users should leave this unset. |

### `models`

| Key | Type / Values | Requirements | Managed | Details |
| --- | --- | --- | --- | --- |
| `models.agent_type` | `string` | `yes` | `user` | Fallback agent_type for models without a per-model override. |
| `models.allowed_models` | `string[]` | `pin` | `user` | Glob allowlist for the model picker, default, and `-m`. Empty means no restriction. |
| `models.auth_providers.<name>` | `table` | `yes` | `user` | Named credential helper used by `[model.<id>] auth_provider`. |
| `models.default_reasoning_effort` | `string` | `yes` | `user` | Default reasoning effort for the default model when the model supports it. |
| `models.default` | `string` | `pin` | `user` | Model used for new sessions. Also `GROK_DEFAULT_MODEL`, `--model`, `-m`. |
| `models.definitions.<id>.agent_type` | `string` | `yes` | `user` | Agent definition type associated with this model. |
| `models.definitions.<id>.api_backend` | `chat_completions / responses / messages` | `yes` | `user` | Wire protocol for this model. |
| `models.definitions.<id>.api_base_url` | `string` | `yes` | `user` | Alternate API base used with XAI_API_KEY resolution. |
| `models.definitions.<id>.api_key` | `string` | `yes` | `user` | Inline API key. Prefer `env_key`. Not a secret to put in a shared repo. |
| `models.definitions.<id>.auth_provider` | `string` | `yes` | `user` | Name of a `models.auth_providers.<name>` helper that mints this model's bearer token. |
| `models.definitions.<id>.auto_compact_threshold_percent` | `integer` | `yes` | `user` | Per-model auto-compact threshold (0-100). |
| `models.definitions.<id>.base_url` | `string` | `yes` | `user` | Provider endpoint base URL. |
| `models.definitions.<id>.compaction_at_tokens` | `number / table` | `yes` | `user` | Token threshold that triggers compaction for this model. |
| `models.definitions.<id>.compactions_remaining` | `string / table` | `yes` | `user` | How compaction leftover context is sent. Alias `send_compactions_remaining`. |
| `models.definitions.<id>.context_window` | `number` | `yes` | `user` | Context window tokens; drives auto-compact timing. |
| `models.definitions.<id>.description` | `string` | `yes` | `user` | Optional description shown in the picker. |
| `models.definitions.<id>.env_http_headers` | `map<string,string>` | `yes` | `user` | HTTP headers populated from environment variables when set. |
| `models.definitions.<id>.env_key` | `string / string[]` | `yes` | `user` | Environment variable name(s) holding the provider API key. |
| `models.definitions.<id>.extra_headers` | `map<string,string>` | `yes` | `user` | Per-request headers for this model. |
| `models.definitions.<id>.hidden` | `boolean` | `yes` | `user` | Hide this model from the picker. Still usable via `-m`. |
| `models.definitions.<id>.inference_idle_timeout_secs` | `number` | `yes` | `user` | Idle timeout for streaming inference on this model. |
| `models.definitions.<id>.max_completion_tokens` | `number` | `yes` | `user` | Per-model max completion tokens. |
| `models.definitions.<id>.max_request_bytes` | `number` | `yes` | `user` | Provider request-body cap that inline images are evicted to stay under. Unset inherits the `models.providers.<id>` value, then the `api_backend` default: 30 MB for `messages`, 50 MiB otherwise. |
| `models.definitions.<id>.max_retries` | `number` | `yes` | `user` | Inference retries for this model. |
| `models.definitions.<id>.model_family` | `string` | `yes` | `user` | Family id used for compaction and capability grouping. |
| `models.definitions.<id>.model_provider` | `string` | `yes` | `user` | Named `models.providers.<name>` provider id for this model. |
| `models.definitions.<id>.model` | `string` | `yes` | `user` | Model id sent to the API. |
| `models.definitions.<id>.mtls_cert_dir` | `string` | `yes` | `user` | Directory containing the model endpoint's mTLS identity as `client.crt` and `client.key`, or `tls.crt` and `tls.key`; configuration is rejected unless the same model has one HTTPS `base_url` and no `api_base_url`, and requests do not follow redirects. |
| `models.definitions.<id>.name` | `string` | `yes` | `user` | Label shown in the model picker. |
| `models.definitions.<id>.query_params` | `map<string,string>` | `yes` | `user` | Extra query parameters on this model's requests. |
| `models.definitions.<id>.rate_limit_retry_threshold` | `number` | `yes` | `user` | Total-attempt ceiling for rate-limited requests, capped by the resolved `max_retries`; when configured, it disables the separate subagent 429 wait loop. |
| `models.definitions.<id>.reasoning_effort` | `string` | `yes` | `user` | Deprecated per-model effort; prefer `reasoning_efforts`. |
| `models.definitions.<id>.reasoning_efforts` | `array of tables` | `yes` | `user` | Allowed reasoning-effort values for this model. When omitted, the menu comes from the endpoint's `/v1/models` row (`reasoning_efforts`, or `capabilities.reasoning_effort` when that is absent). |
| `models.definitions.<id>.reasoning_summary` | `none / auto / concise / detailed` | `yes` | `user` | Responses API `reasoning.summary` for this model; default `concise`. `none` omits the field for endpoints that reject it (e.g. AWS Bedrock Mantle). |
| `models.definitions.<id>.show_model_fingerprint` | `boolean` | `yes` | `user` | Show the provider model fingerprint in the UI when present. |
| `models.definitions.<id>.stream_tool_calls` | `boolean` | `yes` | `user` | Per-model tool-call streaming request shape. |
| `models.definitions.<id>.subagent_rate_limit_max_attempts` | `number` | `yes` | `user` | Maximum subagent 429 wait-loop attempts when `rate_limit_retry_threshold` is unset; default 8, maximum 32, and `0` disables the wait loop. |
| `models.definitions.<id>.supported_in_api` | `boolean` | `yes` | `user` | Whether this catalog entry is offered as a public API model. |
| `models.definitions.<id>.supports_backend_search` | `boolean` | `yes` | `user` | Whether the endpoint supports Grok-hosted server-side search tools. |
| `models.definitions.<id>.supports_reasoning_effort` | `boolean` | `yes` | `user` | Deprecated; prefer `reasoning_efforts`. An explicit `false` keeps the model out of any menu it would otherwise inherit from the endpoint or from a same-model catalog entry. |
| `models.definitions.<id>.system_prompt_label` | `string` | `yes` | `user` | Per-model system-prompt identity label. |
| `models.definitions.<id>.temperature` | `number` | `yes` | `user` | Per-model sampling temperature. |
| `models.definitions.<id>.top_p` | `number` | `yes` | `user` | Per-model top_p. |
| `models.definitions.<id>.use_concise` | `boolean` | `yes` | `user` | Use the concise tool-description pack for this model. |
| `models.definitions.<id>` | `table` | `yes` | `user` | Per-model override or BYOK definition. Prefer `env_key` over inline `api_key`. |
| `models.disabled_models` | `string[]` | `yes` | `user` | Remove these model IDs from the catalog. Wins over `hidden_models`. |
| `models.extra_headers` | `map<string,string>` | `yes` | `user` | Request headers applied to every model; per-model keys win. |
| `models.hidden_models` | `string[]` | `yes` | `user` | Hide these model IDs from the picker; `-m` can still select them. |
| `models.image_description` | `string` | `yes` | `user` | Vision model used to transcribe user-supplied images. |
| `models.inference_idle_timeout_secs` | `number` | `yes` | `user` | Global idle timeout for streaming inference when a model leaves it unset. |
| `models.max_completion_tokens` | `number` | `yes` | `user` | Global max completion tokens default when a model leaves it unset. |
| `models.max_retries` | `number` | `yes` | `user` | Global inference retry default when a model leaves it unset. |
| `models.prompt_suggestion` | `string` | `yes` | `user` | Model pin for next-prompt ghost text. Unset falls through remote, then the session model. |
| `models.providers.<name>` | `table` | `yes` | `user` | Named custom model provider definition. |
| `models.rate_limit_retry_threshold` | `number` | `yes` | `user` | Global total-attempt ceiling for rate-limited requests when a model leaves it unset, capped by the resolved `max_retries`; when configured, it disables the separate subagent 429 wait loop. |
| `models.session_summary` | `string` | `yes` | `user` | Model used for session titles and summaries. |
| `models.stream_tool_calls` | `boolean` | `yes` | `user` | Global tool-call streaming request shape; some BYOK endpoints need false. |
| `models.subagent_rate_limit_max_attempts` | `number` | `yes` | `user` | Global default for subagent 429 wait-loop attempts when `rate_limit_retry_threshold` is unset; default 8, maximum 32, and `0` disables the wait loop. |
| `models.temperature` | `number` | `yes` | `user` | Global sampling temperature default when a model leaves it unset. |
| `models.top_p` | `number` | `yes` | `user` | Global top_p default when a model leaves it unset. |
| `models.web_search` | `string` | `pin` | `user` | Model used by the client `web_search` tool. Also `GROK_WEB_SEARCH_MODEL`. |

### `permission`

| Key | Type / Values | Requirements | Managed | Details |
| --- | --- | --- | --- | --- |
| `permission.allow` | `string[]` | `yes` | `user` | Compact allow rules such as `Bash(git *)`. Deny beats ask beats allow. Project files may set this. |
| `permission.ask` | `string[]` | `yes` | `user` | Compact ask rules. Project files may set this. |
| `permission.deny` | `string[]` | `yes` | `user` | Compact deny rules. Project files may set this. |
| `permission.rules` | `array of tables` | `yes` | `user` | Verbose action/tool/pattern object rules. Project files may set this. |

### `privacy`

| Key | Type / Values | Requirements | Managed | Details |
| --- | --- | --- | --- | --- |
| `privacy.privacy_banner_acked` | `string` | `—` | `—` | RFC 3339 UTC timestamp when the local privacy banner was dismissed. The pager reads user `config.toml` only. |

### `sandbox`

| Key | Type / Values | Requirements | Managed | Details |
| --- | --- | --- | --- | --- |
| `sandbox.auto_allow_bash` | `boolean` | `pin` | `user` | Skip bash permission prompts when a sandbox profile is active. Also GROK_SANDBOX_AUTO_ALLOW_BASH. |
| `sandbox.profile` | `off / workspace / read-only / strict / string` | `pin` | `user` | Filesystem sandbox profile. Also `--sandbox` and GROK_SANDBOX. |

### `session`

| Key | Type / Values | Requirements | Managed | Details |
| --- | --- | --- | --- | --- |
| `session.auto_compact_threshold_percent` | `integer` | `yes` | `user` | Auto-compact when context usage reaches this percent (0–100). |
| `session.harness.disable_workspace_teleport` | `boolean` | `pin` | `user` | Kill switch for per-turn workspace snapshots. |
| `session.harness.wait_for_uploads` | `boolean` | `yes` | `user` | Wait for turn-end trace uploads before returning the prompt response. Off by default; one-shot headless runs instead drain pending turn-end uploads at exit within a mandatory minimum budget (about 150s: the parse window plus one upload attempt) that `upload_flush_timeout_secs`, when larger, extends. |
| `session.load_envrc` | `boolean` | `yes` | `user` | Inject `.envrc` variables into bash. |
| `session.relay.enabled` | `boolean` | `yes` | `user` | Enable session relay sync. |
| `session.storage.cleanup_ttl_days` | `integer` | `yes` | `user` | Days a session may stay idle before its folder is deleted; media and terminal logs older than this are pruned from live sessions; unset or `0` disables cleanup. |

### `telemetry`

| Key | Type / Values | Requirements | Managed | Details |
| --- | --- | --- | --- | --- |
| `telemetry.diagnostics.crash_handler` | `boolean` | `yes` | `user` | Write a panic report under `$GROK_HOME/crash/`. Also GROK_CRASH_HANDLER. |
| `telemetry.feedback.user.command` | `string` | `yes` | `user` | Shell command that prints name and email JSON for feedback submissions. |
| `telemetry.feedback.user.email` | `string[]` | `yes` | `user` | Sources for the feedback author email (`git_email` or a literal). |
| `telemetry.feedback.user.name` | `string[]` | `yes` | `user` | Sources for the feedback author name (`os_user` or a literal). |
| `telemetry.otel_certificate` | `string` | `pin` | `user` | PEM path of extra CA certs for the collector. Also OTEL_EXPORTER_OTLP_CERTIFICATE. CA pin does **not** strip endpoints. |
| `telemetry.otel_client_certificate` | `string` | `pin` | `user` | PEM path of the mTLS client certificate. Also OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE. Pin strips credential copies, developer endpoints, and unlisted file siblings. |
| `telemetry.otel_client_key` | `string` | `pin` | `user` | PEM path of the mTLS client key. Tokens never live in this file. Also OTEL_EXPORTER_OTLP_CLIENT_KEY. |
| `telemetry.otel_enabled` | `boolean` | `pin` | `user` | External OTEL master switch. Also GROK_EXTERNAL_OTEL. |
| `telemetry.otel_endpoint` | `string` | `pin` | `user` | External OTLP base endpoint. Also OTEL_EXPORTER_OTLP_ENDPOINT. Pin strips developer env and unlisted user/managed file siblings except listed. |
| `telemetry.otel_log_assistant_responses` | `boolean` | `pin` | `user` | Content gate for grok_code.assistant_response text. Unset follows otel_log_user_prompts unless a sibling gate is pinned in requirements. Env-only OTEL_LOG_USER_PROMPTS=1 must set this to 0 (or pin it false) for a prompts-only stream. Also OTEL_LOG_ASSISTANT_RESPONSES. |
| `telemetry.otel_log_tool_content` | `boolean` | `pin` | `user` | Body gate for tool_input, tool_output, full_command, and error_message. Independent of details; default off. CONTENT-only loses verbatim MCP names and paths. Also OTEL_LOG_TOOL_CONTENT. |
| `telemetry.otel_log_tool_details` | `boolean` | `pin` | `user` | Metadata gate for tool-arg preview, paths, and verbatim names. Recommended on for SIEM join. Also OTEL_LOG_TOOL_DETAILS. Does not include full bodies. |
| `telemetry.otel_log_user_prompts` | `boolean` | `pin` | `user` | Content gate for prompt text on grok_code.user_prompt. Also OTEL_LOG_USER_PROMPTS. Pinning any content gate without listing a sibling defaults the omitted sibling off. |
| `telemetry.otel_logs_certificate` | `string` | `pin` | `user` | Logs-signal CA PEM path. Also OTEL_EXPORTER_OTLP_LOGS_CERTIFICATE. |
| `telemetry.otel_logs_client_certificate` | `string` | `pin` | `user` | Logs-signal mTLS client cert PEM path. Also OTEL_EXPORTER_OTLP_LOGS_CLIENT_CERTIFICATE. |
| `telemetry.otel_logs_client_key` | `string` | `pin` | `user` | Logs-signal mTLS client key PEM path. Also OTEL_EXPORTER_OTLP_LOGS_CLIENT_KEY. |
| `telemetry.otel_logs_endpoint` | `string` | `pin` | `user` | Logs-signal OTLP endpoint (verbatim). Also OTEL_EXPORTER_OTLP_LOGS_ENDPOINT. |
| `telemetry.otel_logs_exporter` | `otlp / console / none` | `pin` | `user` | External OTEL logs exporter. Also OTEL_LOGS_EXPORTER. |
| `telemetry.otel_logs_protocol` | `http/protobuf / grpc` | `pin` | `user` | Logs-signal OTLP protocol. Also OTEL_EXPORTER_OTLP_LOGS_PROTOCOL. |
| `telemetry.otel_metric_export_interval` | `number` | `pin` | `user` | Metric export interval in milliseconds. Also OTEL_METRIC_EXPORT_INTERVAL. |
| `telemetry.otel_metrics_certificate` | `string` | `pin` | `user` | Metrics-signal CA PEM path. Also OTEL_EXPORTER_OTLP_METRICS_CERTIFICATE. |
| `telemetry.otel_metrics_client_certificate` | `string` | `pin` | `user` | Metrics-signal mTLS client cert PEM path. Also OTEL_EXPORTER_OTLP_METRICS_CLIENT_CERTIFICATE. |
| `telemetry.otel_metrics_client_key` | `string` | `pin` | `user` | Metrics-signal mTLS client key PEM path. Also OTEL_EXPORTER_OTLP_METRICS_CLIENT_KEY. |
| `telemetry.otel_metrics_endpoint` | `string` | `pin` | `user` | Metrics-signal OTLP endpoint (verbatim). Also OTEL_EXPORTER_OTLP_METRICS_ENDPOINT. |
| `telemetry.otel_metrics_exporter` | `otlp / console / none` | `pin` | `user` | External OTEL metrics exporter. Also OTEL_METRICS_EXPORTER. |
| `telemetry.otel_metrics_include_session_id` | `boolean` | `pin` | `user` | Attach session.id to metrics. Also OTEL_METRICS_INCLUDE_SESSION_ID. |
| `telemetry.otel_metrics_protocol` | `http/protobuf / grpc` | `pin` | `user` | Metrics-signal OTLP protocol. Also OTEL_EXPORTER_OTLP_METRICS_PROTOCOL. |
| `telemetry.otel_protocol` | `http/protobuf / grpc` | `pin` | `user` | External OTLP transport. Also OTEL_EXPORTER_OTLP_PROTOCOL. Pin strips per-signal protocol env and unlisted file siblings except listed. |
| `telemetry.otel_timeout` | `number` | `pin` | `user` | Export timeout in milliseconds. Also OTEL_EXPORTER_OTLP_TIMEOUT. |
| `telemetry.trace_upload` | `boolean` | `pin` | `user` | Upload session traces. Requirements pin beats user config. |

### `tools`

| Key | Type / Values | Requirements | Managed | Details |
| --- | --- | --- | --- | --- |
| `tools.ask_user_question.timeout_secs` | `number` | `yes` | `user` | Timeout for the ask_user_question tool. |
| `tools.bash.auto_background_on_timeout` | `boolean` | `yes` | `user` | Background the command when the foreground timeout fires. |
| `tools.bash.login_shell_capture` | `boolean` | `yes` | `user` | Capture the user's login shell environment for bash. Overlay-allowlisted. |
| `tools.bash.max_timeout_secs` | `number` | `yes` | `user` | Cap on model-requested foreground timeouts. |
| `tools.bash.output_byte_limit` | `number` | `yes` | `user` | Max captured bash output in bytes. |
| `tools.bash.timeout_secs` | `number` | `yes` | `user` | Foreground bash command timeout in seconds. |
| `tools.disable_web_search` | `boolean` | `yes` | `user` | Drop the web_search tool for this process. Also `--disable-web-search`. |
| `tools.disable_zdr_incompatible_tools` | `boolean` | `yes` | `user` | Restrict tools that need xAI-hosted output under ZDR. Also GROK_DISABLE_ZDR_INCOMPATIBLE_TOOLS. |
| `tools.file_toolset` | `standard / hashline` | `yes` | `user` | File edit tool scheme. |
| `tools.media_gen.max_parallel_image_gen_calls` | `integer` | `yes` | `user` | Cap parallel image_gen/image_edit calls in one model step. Also GROK_MAX_PARALLEL_IMAGE_GEN_CALLS. |
| `tools.media_gen.max_parallel_video_gen_calls` | `integer` | `yes` | `user` | Cap parallel video_gen calls in one model step. Also GROK_MAX_PARALLEL_VIDEO_GEN_CALLS. |
| `tools.path_not_found_hints` | `boolean` | `yes` | `user` | Enrich path-not-found errors with CWD reminders and similar-name suggestions. |
| `tools.respect_gitignore` | `boolean` | `pin` | `user` | When true, search and read tools skip gitignored files. Also GROK_RESPECT_GITIGNORE. |
| `tools.shell_environment_policy.exclude` | `string[]` | `yes` | `user` | Env names to drop from bash. Overlay-allowlisted. |
| `tools.shell_environment_policy.ignore_default_excludes` | `boolean` | `yes` | `user` | Skip the built-in env denylist. Overlay-allowlisted. |
| `tools.shell_environment_policy.include_only` | `string[]` | `yes` | `user` | If set, bash inherits only these env names. Overlay-allowlisted. |
| `tools.shell_environment_policy.inherit` | `string` | `yes` | `user` | Which parent env names bash inherits. Overlay-allowlisted; cannot inject values. |
| `tools.shell_environment_policy.set` | `map<string,string>` | `yes` | `user` | Inject env values into bash. Not overlay-allowlisted. |
| `tools.web_fetch.allowed_domains` | `string[]` | `yes` | `user` | Domain allowlist override for web_fetch. |
| `tools.web_fetch.proxy_endpoint` | `string` | `yes` | `user` | Egress proxy URL for web_fetch. Also GROK_WEB_FETCH_PROXY. |
| `tools.web_search.allowed_domains` | `string[]` | `yes` | `user` | Domain allowlist for client web_search. Overlay-allowlisted. |
| `tools.web_search.excluded_domains` | `string[]` | `yes` | `user` | Domain denylist for client web_search. Overlay-allowlisted. |
| `tools.zdr_video_output_s3` | `table` | `yes` | `user` | Team S3 bucket for ZDR video output. See ZDR Video Storage. |

### `ui`

| Key | Type / Values | Requirements | Managed | Details |
| --- | --- | --- | --- | --- |
| `ui.approval_mode` | `string` | `yes` | `user` | Deprecated; use `ui.permission_mode`. |
| `ui.auto_dark_theme` | `string` | `yes` | `user` | Theme when `theme = auto` and the OS is dark. |
| `ui.auto_light_theme` | `string` | `yes` | `user` | Theme when `theme = auto` and the OS is light. |
| `ui.cancel_subagents_on_turn_cancel` | `ask / always_stop / always_continue` | `yes` | `user` | What to do with running subagents when cancelling a parent turn. |
| `ui.collapsed_edit_blocks` | `boolean` | `yes` | `user` | Show edits as one-line +N/-M summaries. Also GROK_COLLAPSED_EDIT_BLOCKS. |
| `ui.combine_queued_prompts` | `boolean` | `yes` | `user` | Merge consecutive plain follow-ups into one turn. |
| `ui.compact_mode` | `boolean` | `yes` | `user` | Denser message padding. Also `/compact-mode`. |
| `ui.confirm_before_rewind` | `boolean` | `yes` | `user` | Ask before rewinding conversation history. |
| `ui.contextual_hints.image_input` | `boolean` | `yes` | `user` | Clipboard image paste tip when the model accepts images. |
| `ui.contextual_hints.plan_mode` | `boolean` | `yes` | `user` | Suggest plan mode (Shift+Tab) for planning-style prompts. |
| `ui.contextual_hints.send_now` | `boolean` | `yes` | `user` | After queuing a mid-turn follow-up, Enter on an empty prompt sends now. |
| `ui.contextual_hints.small_screen` | `boolean` | `yes` | `user` | Suggest `/compact-mode` on short terminals. |
| `ui.contextual_hints.ssh_wrap` | `boolean` | `yes` | `user` | Recommend `grok wrap` when SSH lacks a clipboard sink. |
| `ui.contextual_hints.undo` | `boolean` | `yes` | `user` | Ctrl+Z restores a wiped prompt draft tip. |
| `ui.contextual_hints.word_select` | `boolean` | `yes` | `user` | After double-click with fold/nav selection, point at Word select in settings. |
| `ui.cursor_blink` | `boolean` | `yes` | `user` | Force blinking (true) or steady (false) block cursor. Unset inherits the terminal. |
| `ui.dashboard.enabled` | `boolean` | `yes` | `user` | Show the agent dashboard. |
| `ui.dashboard.grouping` | `state / directory` | `yes` | `user` | How dashboard rows group. |
| `ui.dashboard_preview` | `boolean` | `yes` | `user` | The dashboard preview and reply panel appear by default (Appearance in `/settings`). |
| `ui.default_selected_permission` | `string` | `yes` | `user` | Preselected approval row on the first prompt of a session. Also GROK_DEFAULT_SELECTED_PERMISSION. |
| `ui.display_refresh.auto_cadence_enabled` | `boolean` | `yes` | `user` | Match stream/scroll cadence to display refresh rate. Also GROK_DISPLAY_REFRESH_AUTO_CADENCE. |
| `ui.follow_up_behavior` | `queue / steer` | `yes` | `user` | Mid-turn follow-up routing. |
| `ui.group_tool_verbs` | `boolean` | `yes` | `user` | Fold consecutive read/search/list tool rows. Also GROK_GROUP_TOOL_VERBS. |
| `ui.hints.fork_worktree_mode` | `ask / always / never` | `yes` | `user` | Whether `/fork` offers a worktree. |
| `ui.hints.new_session_worktree_mode` | `ask / always / never` | `yes` | `user` | Whether `/new` offers a worktree. |
| `ui.hunk_tracker_mode` | `agent_only / all_dirty / off` | `yes` | `user` | File-change hunk tracking. Also GROK_HUNK_TRACKER and `--hunk-tracker-mode`. |
| `ui.invert_scroll` | `boolean` | `yes` | `user` | Reverse vertical scroll direction. Also GROK_INVERT_SCROLL. |
| `ui.keep_text_selection` | `flash / hold / word_select` | `yes` | `user` | In-app selection: brief flash, hold, or double-click word select. |
| `ui.max_thoughts_width` | `number` | `yes` | `user` | Column width for the thoughts panel (40–500). |
| `ui.mouse_reporting_toggle` | `boolean` | `yes` | `user` | Ctrl+R in scrollback toggles terminal mouse capture. Also GROK_MOUSE_REPORTING_TOGGLE. |
| `ui.page_flip_on_send` | `boolean` | `yes` | `user` | Snap the sent prompt to the top of the viewport. |
| `ui.permission_mode` | `default / ask / auto / always-approve` | `yes` | `user` | Default tool-permission behavior. Enterprise locks use requirements.toml. |
| `ui.prompt_suggestions` | `boolean` | `yes` | `user` | Next-prompt ghost text after each turn. Also GROK_PROMPT_SUGGESTIONS; a remote kill-switch can disable it fleet-wide. |
| `ui.remember_tool_approvals` | `boolean` | `yes` | `user` | Show per-tool Always allow options. Also GROK_REMEMBER_TOOL_APPROVALS. |
| `ui.render_mermaid` | `auto / on / off` | `yes` | `user` | How mermaid fences render: clickable open row or raw source. |
| `ui.screen_mode` | `fullscreen / minimal` | `yes` | `user` | Still loads; has no effect. |
| `ui.scroll_lines` | `integer` | `yes` | `user` | Lines per scroll tick (1–10). Also GROK_SCROLL_LINES. |
| `ui.scroll_mode` | `auto / wheel / trackpad` | `yes` | `user` | Scroll input classification. Also GROK_SCROLL_MODE. |
| `ui.scroll_speed` | `integer` | `yes` | `user` | Mouse/trackpad scroll speed multiplier (1–100). Also GROK_SCROLL_SPEED. |
| `ui.show_thinking_blocks` | `boolean` | `yes` | `user` | Show thinking/reasoning blocks while streaming. Also GROK_SHOW_THINKING_BLOCKS. |
| `ui.show_timeline` | `boolean` | `yes` | `user` | Per-turn tick rail instead of the scrollbar. |
| `ui.show_timestamps` | `boolean` | `yes` | `user` | Clock time next to messages. Also `/timestamps`. |
| `ui.status_line.command` | `string` | `yes` | `user` | Script for a `command` status line. Campaigns strip this path; a requirements layer still merges it. |
| `ui.status_line.type` | `disabled / command` | `yes` | `user` | Optional status-line row above the shortcuts bar. Off by default. See the status-line user guide. |
| `ui.theme` | `string` | `yes` | `user` | Color theme name, or `auto`/`system` to follow the OS. Also `/theme` and GROK_THEME. |
| `ui.ui_theme` | `string` | `yes` | `user` | Legacy alias for `ui.theme`. |
| `ui.voice.api_base` | `string` | `yes` | `user` | HTTPS API root for speech-to-text. Unset inherits `[endpoints].xai_api_base_url`. |
| `ui.voice.language` | `string` | `yes` | `user` | Preferred STT language catalog code or `auto`. |
| `ui.voice.sample_rate` | `number` | `yes` | `user` | STT capture rate in Hz. |
| `ui.voice_capture_mode` | `hold / toggle` | `yes` | `user` | Hold-to-talk or press-to-toggle voice capture. |
| `ui.voice_keybind_enabled` | `boolean` | `yes` | `user` | Enable Ctrl+Space / F8 for voice dictation. `/voice` still works when false. |
| `ui.voice_stt_language` | `string` | `yes` | `user` | Speech-to-text language code or `auto`. Overrides `[voice].language` for the session. |
| `ui.yolo` | `boolean` | `pin` | `user` | Always-approve tool calls. Requirements can pin false and block `--yolo`. |

### `version_overrides`

| Key | Type / Values | Requirements | Managed | Details |
| --- | --- | --- | --- | --- |
| `version_overrides` | `array of tables` | `yes` | `user` | Per-CLI-version config patches applied before merge. See `version_overrides`. |

### `worktrees`

| Key | Type / Values | Requirements | Managed | Details |
| --- | --- | --- | --- | --- |
| `worktrees.auto_gc` | `table` | `yes` | `user` | Automatic worktree garbage collection policy. |

## managed config

Managed config accepts every key in the tables above. It sets fleet defaults, so a developer's own `config.json` overrides it. Use it for values you want people to be able to adjust, and `requirements.toml` for values they cannot.

One exception to that rule:

| Key | Behaviour |
| --- | --- |
| `features.remote_fetch` | The managed value wins over the developer's. |

Grok Build reads `/etc/grok/managed_config.json` first, then `$GROK_HOME/managed_config.toml`, which the console keeps in sync. Values in the second replace values in the first.

The **Managed** column on the tables above is the per-key answer: `fleet` means the fleet value stands, `user` means the user's file wins, `—` means this file is ignored.

## requirements

Requirements are admin-enforced. Locations: `$GROK_HOME/requirements.toml` (signed cache) then `/etc/grok/requirements.json`, then macOS MDM `ai.x.grok`. The **Requirements** column on the `config.json` tables lists every key this file accepts (`pin` or `yes`). Omitted keys stay unconstrained.

These keys exist only in requirements:

| Key | Type / Values | Default | Details |
| --- | --- | --- | --- |
| `fail_closed` | `boolean` | `false` | Refuse to start when signed requirements or version_overrides cannot be applied; default false. |
| `features.image_edit` | `boolean` | — | Pin image_edit availability. Requirements only; a user-file entry is unrecognized and unset leaves the remotely configured default. |
| `ui.disable_bypass_permissions_mode` | `boolean` | — | Lock always-approve off. The lock is enforced only from a requirements layer; true in user or managed files is ignored. |

Policy pins such as `policy.allow_managed_hooks_only` (see [Hooks](10-hooks.md#allow-only-managed-hooks)) and the MCP and marketplace lists under `policy` (see [Plugins](09-plugins.md#restrict-which-mcp-servers-can-run)) are accepted in requirements. It managed config alike and only ever tighten.

`hooks.<Event>` arrays are accepted in every config file. Hooks from the signed requirements cache and the root-owned `/etc/grok` files are enforced. See [Hooks](10-hooks.md#enforced-hooks).

## What happens when a setting is refused

| Situation | What Grok Build does |
| --- | --- |
| A developer sets a key you pinned | The pinned value applies. `grok inspect` lists the requirements file that contributed. |
| A developer sets a key you shipped in managed config | Their value applies, except `features.remote_fetch`. Pin the key instead if it must hold. |
| Requirements are missing or their signature does not verify | The pins do not apply, and Grok Build starts without them. Set `"fail_closed": true` to refuse to start instead. |
| A pinned key names a value this version does not recognise | The key is ignored and the rest of the file still applies. |

## Check what is in effect

Run `grok inspect` on the developer's machine. It lists every config file that contributed, including requirements and managed layers, so a policy that is not applying is visible in one command.
