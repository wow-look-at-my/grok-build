# Custom Models

Grok connects to custom model endpoints for alternative providers, self-hosted models, and overriding built-in settings. This guide explains how to select models, configure endpoints, and integrate third-party providers.

---

## Default Models

By default, Grok uses models hosted by SpaceXAI, and new sessions start with `grok-4.5`. Default models require no configuration. Authenticate with `grok login` or an API key, then start a session.

List all available models:

```bash
grok models
```

---

## Selecting a Model

### CLI Flag

```bash
grok -p "Hello" -m grok-build
```

### Slash Command

In the TUI, switch models during a session:

```
/model grok-build
```

Or use the alias:

```
/m grok-build
```

### Model Picker (Ctrl+M)

Press `Ctrl+M` from the scrollback pane to open the model picker. It lists all available models, both built-in and custom, and lets you switch with a single keystroke. With the prompt focused, `Ctrl+M` toggles multiline input instead -- use `/model` to switch without leaving the prompt.

Provider catalogs are combined rather than replaced. For example, after
`/login codex`, Codex models appear alongside Grok and custom models with
qualified IDs such as `codex/gpt-5.4` and display names prefixed with
`Codex ·`. Refreshing or signing out of one provider does not erase models
owned by another provider.

### Config Default

Set a persistent default in `~/.grok/config.toml`:

```toml
[models]
default = "grok-4.5"
```

---

## Supported API Backends

Grok supports three API backends. Set `api_backend` in your `[model.*]` config to choose which protocol the model uses:

| Value | API | Default |
|-------|-----|---------|
| `"chat_completions"` | OpenAI Chat Completions (`/v1/chat/completions`) | Yes |
| `"responses"` | OpenAI Responses (`/v1/responses`) | |
| `"messages"` | Anthropic Messages (`/v1/messages`) | |

When you omit `api_backend`, Grok uses `chat_completions`.

To send provider-specific authentication or version headers -- for example, Anthropic's `x-api-key` -- use the `extra_headers` field described below. Grok sends those headers verbatim with every request to the endpoint.

---

## Configuring Custom Models

Add custom model endpoints in `~/.grok/config.toml` under `[model.<name>]` sections:

```toml
[model.my-model]
model = "model-id"                        # Model identifier sent to the API
base_url = "https://api.example.com/v1"   # OpenAI-compatible endpoint
name = "Display Name"                     # Shown in the model picker
description = "Model description"          # Optional description
api_key = "sk-..."                        # API key for this provider (optional)
env_key = "XAI_API_KEY"                   # Env var holding the API key (optional; string or array)
api_backend = "chat_completions"          # "chat_completions", "responses", or "messages"
temperature = 0.7                         # Sampling temperature
top_p = 0.95                              # Nucleus sampling parameter
max_completion_tokens = 8192              # Maximum tokens per response
context_window = 128000                   # Total context window in tokens
extra_headers = { "x-api-key" = "sk-..." } # Extra request headers, sent verbatim (optional)
query_params = { api-version = "2026-07-22" } # Query params appended to every request URL (optional)
env_http_headers = { "X-Tenant" = "TENANT_TOKEN" }    # Headers from env vars, resolved at client build (optional)
```

### Credential Resolution

Grok resolves the API key in this order:

1. The `api_key` field in the model config
2. The environment variable(s) named by `env_key` — a single string or an array of names. The first set, non-empty value wins (for example `env_key = ["ANTHROPIC_AUTH_TOKEN", "LC_ANTHROPIC_AUTH_TOKEN"]` for SSH `LC_*` forwarding)
3. Your signed-in session token (from `grok login`), for a model with no `api_key`/`env_key` of its own
4. The `XAI_API_KEY` environment variable (global fallback; Grok also accepts `GROK_CODE_XAI_API_KEY` for backward compatibility)

### Context Window

The `context_window` value tells Grok when to trigger auto-compaction. When you override a known model, Grok inherits that model's context window. When you define a new model and omit `context_window`, Grok defaults to 200,000 tokens, so set it explicitly to match your provider.

### Global Default Headers

To apply the same headers to *every* model in the catalog -- built-in, prefetched from `/v1/models`, or custom -- set them once under the global `[models]` section instead of repeating them per model:

```toml
[models]
extra_headers = { "X-Request-Tags" = "team=example,env=prod" }
```

These act as a base for each model's inference requests. A per-model `[model.<id>].extra_headers` entry overrides the global default **per key** (matched case-insensitively): a key set on the model wins, while any global-only keys are still inherited by that model. Like the per-model field, they ride on that model's inference calls -- not on separate services such as image generation or video generation -- which makes them handy for attribution tags (for example, cost tracking) without re-declaring them whenever a new model appears.

### Global Default Values

A few common per-model settings can also be set once under `[models]` as a default for *every* model. A per-model `[model.<id>]` value always wins; the global only fills in where a model (or the server's model list) left the field unset:

```toml
[models]
temperature                 = 0.7
top_p                       = 0.95
max_completion_tokens       = 8192
max_retries                 = 8
inference_idle_timeout_secs = 600
stream_tool_calls           = true
```

This is a small, fixed set of environment-wide knobs. Settings that identify a specific model (`model`, `base_url`, `api_key`, `context_window`, ...) cannot be defaulted this way, and a few settings with their own dedicated configuration -- auto-compaction (`[session]`), the system-prompt label (`[agent]`), and reasoning effort (`[models].default_reasoning_effort`) -- keep their existing homes.

> **Note on `stream_tool_calls`:** this one affects request *shape*, not just sampling. A few endpoints (some BYOK providers) expect it left unset; if a global `stream_tool_calls = true` causes problems for such a model, opt that model out with `stream_tool_calls = false` in its `[model.<id>]` block.

### Request Query Parameters

Some gateways route or version on the query string. `query_params` appends percent-encoded query parameters to every request Grok makes for a model. For example, a gateway that selects an API version this way:

```toml
[model.my-gateway]
model = "my-model"
base_url = "https://gateway.example/v1"
api_backend = "responses"
env_key = "GATEWAY_API_KEY"
query_params = { api-version = "2026-07-22" }
```

A key that also appears in the `base_url` query string is overridden (last value wins) rather than duplicated. Query parameters are saved in the session, so do not put secrets in them: use `env_http_headers` for a secret.

### Environment-Variable Headers

`env_http_headers` maps a request header to the name of an environment variable that supplies its value, so a per-request secret never has to be written into `config.toml`:

```toml
[model.gateway]
model = "my-model"
base_url = "https://gateway.example/v1"
env_http_headers = { "X-Tenant-Token" = "GATEWAY_TENANT_TOKEN" }
```

Grok reads each variable when it builds the client for a session and places the value in the request headers only, never on disk. A header is skipped when its variable is unset or blank, and a resolved value overrides an `extra_headers` entry of the same name. Use `extra_headers` for a static value and `env_http_headers` for one that comes from the environment.

Both fields also work on a shared `[model_providers.<id>]` block -- see [Provider Defaults](#provider-defaults).

---

## Provider Defaults

Most of a `[model.<id>]` block is not about the model at all. The endpoint, the API key, the wire format and the headers belong to the *provider*. Repeating them on every model is how one rotated key turns into a dozen edits.

Put them in a `[model_providers.<id>]` block once, and point each model at it with `model_provider = "<id>"`:

```toml
[model_providers.acme]
base_url    = "https://gateway.acme.com/v1"
api_backend = "responses"
env_key     = "ACME_API_KEY"
context_window = 128000
extra_headers = { "anthropic-version" = "2023-06-01" }
query_params  = { api-version = "2026-07-22" }

[model.acme-fast]
model = "acme-fast-1"
name  = "Acme Fast"

[model.acme-deep]
model = "acme-deep-1"
name  = "Acme Deep"
temperature = 0.2          # this model only
```

Both models reach the gateway with the same URL, key, backend and headers. Neither restates one.

### What a provider can set

Everything except what identifies a single model (`model`, `name`, `description`):

| Group | Fields |
|-------|--------|
| Endpoint | `base_url`, `api_base_url`, `query_params` |
| Credentials | `api_key`, `env_key`, `auth_provider`, `[model_providers.<id>.auth]` |
| Wire format | `api_backend`, `strict_message_schema`, `stream_tool_calls` |
| Headers | `extra_headers`, `env_http_headers` |
| Sampling | `temperature`, `top_p`, `max_completion_tokens`, `context_window` |
| Reasoning | `reasoning_effort`, `supports_reasoning_effort`, `reasoning_efforts` |
| Behavior | `max_retries`, `inference_idle_timeout_secs`, `min_output_tokens_per_sec`, `supports_backend_search`, `use_concise`, `agent_type`, `show_model_fingerprint`, `compactions_remaining`, `compaction_at_tokens` |
| Catalog | `hidden`, `supported_in_api`, `pricing` |

### How inheritance resolves

A model's own value always wins. The provider only fills in what the model left unset.

- **Scalar fields** (`base_url`, `api_backend`, `temperature`, ...) inherit when the model omits the field. An explicit `false` or `0` on the model is a value, not an omission.
- **Table fields** (`extra_headers`, `query_params`, `env_http_headers`) inherit **per key**. A model that sets one header of its own still gets every other header from the provider. Header names match case-insensitively, so a model's `x-tenant` shadows the provider's `X-Tenant` rather than riding beside it.
- **Credentials inherit as a set.** Set any of `api_key`, `env_key` or `auth_provider` on a model, and that model inherits none of the provider's. Half a credential from each side is never what you meant.
- A model naming a provider that does not exist warns and falls back to its own fields.

### Credential helpers

A provider can mint tokens with a credential helper in place of a static key. Name an existing `[auth_provider.<name>]` block, or declare one inline:

```toml
[model_providers.corp]
base_url = "https://llm.corp.internal/v1"

[model_providers.corp.auth]
command         = "/usr/local/bin/corp-token"
token_ttl_secs  = 3600
```

Every model behind `model_provider = "corp"` runs that helper. A model with its own `api_key` or `env_key` bypasses it.

A provider endpoint is not an xAI endpoint, so a model behind one never falls back to your `grok login` session token. A credential that does not resolve fails the request closed. The session bearer is never sent to a third party.

### Global defaults vs provider defaults

`[models]` (see [Global Default Values](#global-default-values)) covers *every* model in the catalog, including built-in and prefetched ones. `[model_providers.<id>]` covers only the models that name it. Precedence runs model → provider → `[models]` global → built-in default.

### Autodetected Provider Models

A `[model_providers.<id>]` block declares a base URL. Grok asks that base what models it serves, at `<base_url>/models`, and adds every model it names to the catalog. You write no `[model.<id>]` block per model:

```toml
[model_providers.gateway]
base_url = "https://gateway.example/v1"
env_key = "GATEWAY_API_KEY"
```

Each autodetected model is keyed `<provider id>/<model slug>`, for example `gateway/claude-sonnet-4-6`. The key is qualified so a slug that several providers serve stays one entry per provider, and so your own `[model.<id>]` block is never shadowed. Every autodetected model inherits the provider's connection and credential fields, exactly as a model that names `model_provider` does.

Discovery runs in the background at startup, so a slow provider never delays the session. A provider that cannot be reached contributes no models and never fails the others. Its models appear in the picker as soon as its listing answers.

These keys control it:

```toml
[model_providers.huge]
base_url = "https://huge.example/v1"
# This provider serves hundreds of models. Do not ask.
models_autodetect = false

[model_providers.elsewhere]
base_url = "https://elsewhere.example/inference"
# The listing is not at <base_url>/models.
models_list_url = "https://elsewhere.example/catalog.json"
```

`models_list_url` is asked for verbatim. The models it names still route to the provider's `base_url`.

### Favorite Models

A provider with a large catalog fills the picker. `favorite_models` is a list of glob patterns that marks the models you want in front of you:

```toml
[models]
favorite_models = ["grok-4*"]

[model_providers.gateway]
base_url = "https://gateway.example/v1"
favorite_models = ["*-sonnet-*", "*-opus-*"]
```

`/model` opens on the favorites plus the model the session is running. The moment you type, it searches the whole catalog, so a model nobody marked is still one search away and `-m` still reaches it.

The lists are joined: `[models].favorite_models` matches any model, and a provider's own list matches only that provider's models. A pattern matches the catalog key or the routing slug, case-sensitive. When nothing is marked, the picker lists every model as before.

---

## Overriding Built-in Models

You can override specific fields of built-in models without redefining everything. Only specify the fields you want to change:

```toml
# Override only the API key for a default model
[model.grok-build]
api_key = "my-api-key"

# Override temperature and add a custom API key
[model.grok-build]
temperature = 0.5
api_key = "sk-custom"
```

When you override a built-in model, Grok starts with the default configuration (including the correct `base_url`), then applies only the fields you specify. Unspecified fields inherit from the default.

### Priority Order

1. Your config (`[model.*]`) -- highest priority
2. Prefetched models from remote `/v1/models`
3. Hardcoded defaults -- lowest priority

---

## Provider Examples

### Anthropic (Claude)

Use Claude models directly via the Anthropic Messages API:

```toml
[model.claude-opus]
model = "claude-opus-4-6"
base_url = "https://api.anthropic.com/v1"
name = "Claude Opus 4.6"
api_backend = "messages"
context_window = 200000
extra_headers = { "x-api-key" = "sk-ant-...", "anthropic-version" = "2023-06-01" }
```

The `messages` backend uses the Anthropic Messages protocol. Anthropic authenticates with an `x-api-key` header rather than `Authorization: Bearer`, so pass your key through `extra_headers`, which Grok sends verbatim.

For more than one Claude model, move the shared half to a provider block:

```toml
[model_providers.anthropic]
base_url = "https://api.anthropic.com/v1"
api_backend = "messages"
context_window = 200000
env_http_headers = { "x-api-key" = "ANTHROPIC_API_KEY" }
extra_headers = { "anthropic-version" = "2023-06-01" }

[model.claude-opus]
model = "claude-opus-4-6"
name  = "Claude Opus 4.6"
model_provider = "anthropic"

[model.claude-haiku]
model = "claude-haiku-4-5"
name  = "Claude Haiku 4.5"
model_provider = "anthropic"
```

### OpenAI (Chat Completions)

```toml
[model.gpt-4o]
model = "gpt-4o"
base_url = "https://api.openai.com/v1"
name = "GPT-4o"
env_key = "OPENAI_API_KEY"
```

`api_backend` defaults to `"chat_completions"`, so you don't need to set it explicitly for OpenAI.

### OpenAI (Responses API)

If your provider supports the newer Responses API:

```toml
[model.gpt-4o-responses]
model = "gpt-4o"
base_url = "https://api.openai.com/v1"
name = "GPT-4o (Responses)"
api_backend = "responses"
env_key = "OPENAI_API_KEY"
```

### Ollama and LM Studio (local models)

Declare the provider and every model it serves shows up in `/model`, with its
real context window, its capabilities and a dot showing whether it is loaded in
VRAM right now:

```toml
[model_providers.ollama]
```

That is the whole configuration. The provider id fills in the endpoint
(`http://localhost:11434/v1`), the listing dialect and the pricing switch. LM
Studio is the same:

```toml
[model_providers.lmstudio]
```

Anything you write yourself wins over those defaults:

```toml
[model_providers.ollama]
base_url = "http://workstation.local:11434/v1"
context_window_source = "loaded"   # "loaded" (default) or "max"
favorite_models = ["qwen3-coder*"]
```

**Why this matters.** Asked through the OpenAI-compatible `/v1/models`, a local
runtime reports an id and nothing else, so every model gets the client's
default 256k window. Ollama actually loads a runner at whatever your VRAM
allowed — often 4k — and then silently drops the oldest messages once the
prompt overflows. The harness never compacts, and the conversation loses its
head with no error. Reading the runtime's own listing is what makes the
number true.

The green dot beside a model in `/model` means it is resident in VRAM. A dim
dot means it is on disk and would have to load first. Models that are not from
a local runtime have no dot at all. The dot keeps up with the runtime on its
own: a model that loads on its first request, or that LM Studio's idle TTL
unloads, changes colour within a few seconds without a restart.

#### Pinning the window and keeping the model warm

Ollama's OpenAI-compatible endpoint cannot carry `num_ctx`, `keep_alive` or
`truncate` at all — they are not fields on that request. To set them, use the
native backend:

```toml
[model_providers.ollama]
base_url = "http://localhost:11434"   # native paths live at the host root
api_backend = "ollama"                # POST /api/chat

[model_providers.ollama.extra_body]
"options.num_ctx" = 32768   # pin the window the runner loads at
keep_alive = "30m"          # stay resident between turns
truncate = false            # error instead of silently dropping history
```

LM Studio's compatible endpoint does accept extra body fields, so it needs no
backend change:

```toml
[model_providers.lmstudio.extra_body]
ttl = 1800   # unload after 30 idle minutes
```

`extra_body` also works on a single `[model.<id>]` block, and a model's own
value wins over its provider's key by key.

Make sure the runtime is running (`ollama serve`, or LM Studio's server tab)
and the model is pulled (`ollama pull qwen3-coder:30b`).

### Together AI

```toml
[model.together-mixtral]
model = "mistralai/Mixtral-8x7B-Instruct-v0.1"
base_url = "https://api.together.xyz/v1"
name = "Mixtral 8x7B"
env_key = "TOGETHER_API_KEY"
```

### Local OpenAI-Compatible Server

Any server that implements the OpenAI Chat Completions or Responses API:

```toml
[model.local-llama]
model = "llama-3.1-70b"
base_url = "http://localhost:8080/v1"
name = "Local Llama"
temperature = 0.8
```

---

## Custom Models Endpoint

Point Grok at a custom OpenAI-compatible `/v1/models` endpoint instead of the default. Use this when your models sit behind a corporate gateway or a self-hosted inference service.

### Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `GROK_MODELS_BASE_URL` | Yes | Base URL for inference. Grok fetches the model list from `{base_url}/models`. |
| `XAI_API_KEY` | Yes | API key sent as `Authorization: Bearer`. Grok also accepts `GROK_CODE_XAI_API_KEY`. |
| `GROK_MODELS_LIST_URL` | No | Override the model-list URL when it differs from `{base_url}/models`. |

### Setup

```bash
export GROK_MODELS_BASE_URL="https://api.acme.com/v1"
export XAI_API_KEY="xai-..."
grok
```

### Config File Alternative

```toml
[endpoints]
models_base_url = "https://api.acme.com/v1"

# Override only the API key for a specific model
[model.grok-build]
api_key = "my-api-key"
```

When you use `[endpoints]` with partial model overrides, Grok inherits the `base_url` from the endpoints config, so you do not need to specify it in each `[model.*]` section.

### Auth Behavior

When you set `models_base_url`, Grok uses API key auth (`Authorization: Bearer`) instead of session auth. You do not need `grok login` -- the API key is enough.

---

## Web Search Model

The `web_search` tool uses a separate model. Configure it with:

```toml
[models]
web_search = "grok-4.5"
```

Or via environment variable:

```bash
export GROK_WEB_SEARCH_MODEL="grok-4.5"
```

If you point web search at a custom model, you also need a `[model.*]` entry so Grok can reach it. Server-side ("backend") web search runs only when the model sets `supports_backend_search = true` (and the build enables backend search); it does not depend on `api_backend`:

```toml
[models]
web_search = "my-custom-model"

[model.my-custom-model]
model = "my-custom-model"
supports_backend_search = true
```

---

## Using Custom Models

```bash
# List available models (including custom)
grok models

# Use in the TUI via slash command
/model my-model

# Use in headless mode
grok -p "Hello" -m my-model

# Set as default in config.toml:
[models]
default = "my-model"
```

---

## Enterprise Deployment

A complete config for an enterprise deployment with custom models:

```toml
[cli]
auto_update = false

[auth]
auth_provider_command = "/usr/local/bin/my-company-auth-provider"
auth_provider_label = "Acme Corp"
auth_token_ttl = 3600

[models]
default = "company-grok"

[model.company-grok]
model = "grok-build"
base_url = "https://grok-proxy.acme.com/"
name = "Grok Build Latest (Proxy)"
context_window = 128000

[features]
telemetry = false
```

---

## Troubleshooting

### Model Not Found

```bash
# List available models
grok models

# Check config.toml for typos in [model.*] sections
```

### Connection Errors

Verify the endpoint is reachable:

```bash
curl -s https://api.example.com/v1/models \
  -H "Authorization: Bearer $XAI_API_KEY"
```

### Debug Logging

```bash
RUST_LOG=debug GROK_LOG_FILE=/tmp/grok.log grok
tail -f /tmp/grok.log
```

Look for log entries containing `model` or `sampling` to trace model selection and API calls.
