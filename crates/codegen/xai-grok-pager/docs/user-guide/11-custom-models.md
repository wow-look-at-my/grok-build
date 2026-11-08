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
grok -p "Hello" -m grok-4.6
```

### Slash Command

In the TUI, switch models during a session:

```
/model grok-4.6
```

Or use the alias:

```
/m grok-4.6
```

### Model Picker (Ctrl+M)

Press `Ctrl+M` from the scrollback pane to open the model picker. It lists all available models, both built-in and custom, and lets you switch with a single keystroke. With the prompt focused, `Ctrl+M` toggles multiline input instead -- use `/model` to switch without leaving the prompt.

Provider catalogs are combined rather than replaced. For example, models autodetected from a `models.providers.<id>` block appear alongside Grok and custom models with qualified IDs such as `ollama/llama3`. Refreshing or signing out of one provider does not erase models owned by another provider.

### Fleet allowlist (`requirements.toml`)

Enterprise hosts can pin the **selectable** set — not only the default — in signed `requirements.toml`. That list **replaces** any user `allowed_models` (it is not a union), so `/model`, `Ctrl+M`, and `-m` cannot offer models outside it.

```json
{
  "models": {
    "allowed_models": [
      "grok-4.5",
      "grok-4*"
    ],
    "default": "grok-4.5"
  }
}
```

A fleet pin matches the **model id** (not a user-chosen catalog key), so a local `models.definitions.<name>` entry cannot widen the set. User-config `allowed_models` still matches catalog key or model id. Omit the key to leave user config standing. An empty array is unrestricted. A present-but-unreadable pin fail-closes (nothing selectable). A default or `-m` value outside the pinned set is rejected once the model catalog is fetched — contact your administrator.

The list is not user-editable.

### Config Default

Set a persistent default in `~/.grok/config.json`:

```json
{
  "models": {
    "default": "grok-4.5"
  }
}
```

---

## Supported API Backends

Grok supports API backends. Set `api_backend` in your `models.definitions.*` config to choose which protocol the model uses:

| Value | API | Default |
|-------|-----|---------|
| `"chat_completions"` | OpenAI Chat Completions (`/v1/chat/completions`) | Yes |
| `"responses"` | OpenAI Responses (`/v1/responses`) | |
| `"messages"` | Anthropic Messages (`/v1/messages`) | |

When you omit `api_backend`, Grok uses `chat_completions`.

To send provider-specific authentication or version headers -- for example, Anthropic's `x-api-key` -- use the `extra_headers` field described below. Grok sends those headers verbatim with every request to the endpoint.

---

## Configuring Custom Models

Add custom model endpoints in `~/.grok/config.json` under `models.definitions.<name>` sections:

```json
{
  "models": {
    "definitions": {
      "my-model": {
        "api_backend": "chat_completions",
        "api_key": "sk-...",
        "base_url": "https://api.example.com/v1",
        "context_window": 128000,
        "description": "Model description",
        "env_http_headers": {
          "X-Tenant": "TENANT_TOKEN"
        },
        "env_key": "XAI_API_KEY",
        "extra_headers": {
          "x-api-key": "sk-..."
        },
        "max_completion_tokens": 8192,
        "model": "model-id",
        "name": "Display Name",
        "query_params": {
          "api-version": "2026-07-22"
        },
        "reasoning_summary": "concise",
        "temperature": 0.7,
        "top_p": 0.95
      }
    }
  }
}
```

### Credential Resolution

Grok resolves the API key in this order:

1. The `api_key` field in the model config
2. The environment variable(s) named by `env_key` — a single string or an array of names. The first set, non-empty value wins (for example `env_key = ["ANTHROPIC_AUTH_TOKEN", "LC_ANTHROPIC_AUTH_TOKEN"]` for SSH `LC_*` forwarding)
3. Your signed-in session token (from `grok login`), for a model with no `api_key`/`env_key` of its own
4. The `XAI_API_KEY` environment variable (global fallback; Grok also accepts `GROK_CODE_XAI_API_KEY` for backward compatibility)

### Context Window

The `context_window` value tells Grok when to trigger auto-compaction. When you override a known model, Grok inherits that model's context window. When you define a new model and omit `context_window`, Grok defaults to 200,000 tokens, so set it explicitly to match your provider.

### Request Size Limit

`max_request_bytes` is the largest request body your endpoint accepts. Grok evicts older inline images from the conversation to stay under it, so a session with many screenshots keeps working instead of being rejected. When you omit it, Grok picks the default for the `api_backend`: 30 MB for `messages`, and 50 MiB for `chat_completions` and `responses`. Set it only when your host enforces a different cap.

The cap is a property of the endpoint. It also works on a shared `models.providers.<id>` block, where every model pointing at that provider inherits it. A `max_request_bytes` on the model itself overrides the provider's value.

```json
{
  "models": {
    "definitions": {
      "claude-opus": {
        "max_request_bytes": 20000000,
        "model": "claude-opus",
        "model_provider": "messages-gateway"
      },
      "claude-sonnet": {
        "model": "claude-sonnet",
        "model_provider": "messages-gateway"
      }
    },
    "providers": {
      "messages-gateway": {
        "api_backend": "messages",
        "base_url": "https://gateway.example/v1",
        "max_request_bytes": 25000000
      }
    }
  }
}
```

### Global Default Headers

Prefetched from `/v1/models`, or custom -- set them once under the global `models` section instead of repeating them per model:. Do this to apply the same headers to *every* model in the catalog -- built-in.

```json
{
  "models": {
    "extra_headers": {
      "X-Request-Tags": "team=example,env=prod"
    }
  }
}
```

These act as a base for each model's inference requests. A per-model `models.definitions.<id>.extra_headers` entry overrides the global default **per key** (matched case-insensitively). A key set on the model wins, while any global-only keys are still inherited by that model. Like the per-model field, they ride on that model's inference calls -- not on separate services such as image generation or video generation. Which makes them handy for attribution tags (for example, cost tracking) without re-declaring them when a new model appears.

### Global Default Values

A few common per-model settings can also be set once under `models` as a default for *every* model. A per-model `models.definitions.<id>` value always wins. The global only fills in where a model (or the server's model list) left the field unset:

```json
{
  "models": {
    "inference_idle_timeout_secs": 600,
    "max_completion_tokens": 8192,
    "max_retries": 8,
    "rate_limit_retry_threshold": 4,
    "stream_tool_calls": true,
    "subagent_rate_limit_max_attempts": 8,
    "temperature": 0.7,
    "top_p": 0.95
  }
}
```

This is a small, fixed set of environment-wide knobs.

`rate_limit_retry_threshold` and `subagent_rate_limit_max_attempts` select different 429 retry paths for subagents. Configuring `rate_limit_retry_threshold` makes the sampler own those retries and disables the separate subagent wait loop, including its 150-second cumulative wait budget and wait telemetry. `subagent_rate_limit_max_attempts` applies only when the sampler threshold is unset.

> **Note on `stream_tool_calls`:** this one affects request *shape*, not just sampling. A few endpoints (some BYOK providers) expect it left unset; if a global `"stream_tool_calls": true` causes problems for such a model, opt that model out with `"stream_tool_calls": false` in its `models.definitions.<id>` block.

### Request Query Parameters

Some gateways route or version on the query string. `query_params` appends percent-encoded query parameters to every request Grok makes for a model. For example, a gateway that selects an API version this way:

```json
{
  "models": {
    "definitions": {
      "my-gateway": {
        "api_backend": "responses",
        "base_url": "https://gateway.example/v1",
        "env_key": "GATEWAY_API_KEY",
        "model": "my-model",
        "query_params": {
          "api-version": "2026-07-22"
        }
      }
    }
  }
}
```

A key that also appears in the `base_url` query string is overridden (last value wins) rather than duplicated. Query parameters are saved in the session, so do not put secrets in them: use `env_http_headers` for a secret.

### Environment-Variable Headers

`env_http_headers` maps a request header to the name of an environment variable that supplies its value. A per-request secret never has to be written into `config.json`:

```json
{
  "models": {
    "definitions": {
      "gateway": {
        "base_url": "https://gateway.example/v1",
        "env_http_headers": {
          "X-Tenant-Token": "GATEWAY_TENANT_TOKEN"
        },
        "model": "my-model"
      }
    }
  }
}
```

Grok reads each variable when it builds the client for a session and places the value in the request headers only, never on disk. A header is skipped when its variable is unset or blank, and a resolved value overrides an `extra_headers` entry of the same name. Use `extra_headers` for a static value and `env_http_headers` for one that comes from the environment.

Both fields also work on a shared `models.providers.<id>` block -- see [Provider Defaults](#provider-defaults).

---

## Provider Defaults

Most of a `models.definitions.<id>` block is not about the model at all. The endpoint, the API key, the wire format and the headers belong to the *provider*. Repeating them on every model is how one rotated key turns into a dozen edits.

Put them in a `models.providers.<id>` block once, and point each model at it with `"model_provider": "<id>"`:

```json
{
  "models": {
    "definitions": {
      "acme-deep": {
        "model": "acme-deep-1",
        "name": "Acme Deep",
        "temperature": 0.2
      },
      "acme-fast": {
        "model": "acme-fast-1",
        "name": "Acme Fast"
      }
    },
    "providers": {
      "acme": {
        "api_backend": "responses",
        "base_url": "https://gateway.acme.com/v1",
        "context_window": 128000,
        "env_key": "ACME_API_KEY",
        "extra_headers": {
          "anthropic-version": "2023-06-01"
        },
        "query_params": {
          "api-version": "2026-07-22"
        }
      }
    }
  }
}
```

Both models reach the gateway with the same URL, key, backend and headers. Neither restates one.

### How inheritance resolves

A model's own value always wins. The provider only fills in what the model left unset.

- An explicit `false` or `0` on the model is a value, not an omission.
- A model that sets one header of its own still gets every other header from the provider. Header names match case-insensitively, so a model's `x-tenant` shadows the provider's `X-Tenant` rather than riding beside it.
- **Credentials inherit as a set.** Half a credential from each side is never what you meant.
- A model naming a provider that does not exist warns and falls back to its own fields. It never sends your Grok sign-in token.
- **A URL is never invented.** A model you add takes its URL from its own `base_url`, its provider's `base_url`, or `endpoints.models_base_url`. With none of them, the model has no URL. Grok logs an error at startup and refuses every request to that model. The error tells you to set `base_url`.
- **A URL must also be allowed.** Grok sends nothing to a host missing from `endpoints.allowed_endpoints`. See [Allowed endpoints](05-configuration.md#allowed-endpoints).

### Credential helpers

A provider can mint tokens with a credential helper in place of a static key. Name an existing `models.auth_providers.<name>` block, or declare one inline:

```json
{
  "models": {
    "providers": {
      "corp": {
        "auth": {
          "command": "/usr/local/bin/corp-token",
          "token_ttl_secs": 3600
        },
        "base_url": "https://llm.corp.internal/v1"
      }
    }
  }
}
```

Every model behind `model_provider = "corp"` runs that helper. A model with its own `api_key` or `env_key` bypasses it.

A provider endpoint is not an xAI endpoint, so a model behind one never falls back to your `grok login` session token. A credential that does not resolve fails the request closed. The session bearer is never sent to a third party.

### Global defaults vs provider defaults

`models` (see [Global Default Values](#global-default-values)) covers *every* model in the catalog, including built-in and prefetched ones. `models.providers.<id>` covers only the models that name it. Precedence runs model → provider → `models` global → built-in default.

### Autodetected Provider Models

A `models.providers.<id>` block declares a base URL. Grok asks that base what models it serves, at `<base_url>/models`, and adds every model it names to the catalog. You write no `models.definitions.<id>` block per model:

```json
{
  "models": {
    "providers": {
      "gateway": {
        "base_url": "https://gateway.example/v1",
        "env_key": "GATEWAY_API_KEY"
      }
    }
  }
}
```

Each autodetected model is keyed `<provider id>/<model slug>`, for example `gateway/claude-sonnet-4-6`. The key is qualified so a slug that several providers serve stays one entry per provider, and so your own `models.definitions.<id>` block is never shadowed. Every autodetected model inherits the provider's connection and credential fields, exactly as a model that names `model_provider` does.

Discovery runs in the background at startup, so a slow provider never delays the session. A provider that cannot be reached contributes no models and never fails the others. Its models appear in the picker as soon as its listing answers.

These keys control it:

```json
{
  "models": {
    "providers": {
      "elsewhere": {
        "base_url": "https://elsewhere.example/inference",
        "models_list_url": "https://elsewhere.example/catalog.json"
      },
      "huge": {
        "base_url": "https://huge.example/v1",
        "models_autodetect": false
      }
    }
  }
}
```

`models_list_url` is asked for verbatim. The models it names still route to the provider's `base_url`.

### Favorite Models

A provider with a large catalog fills the picker. `favorite_models` is a list of glob patterns that marks the models you want in front of you:

```json
{
  "models": {
    "favorite_models": [
      "grok-4*"
    ],
    "providers": {
      "gateway": {
        "base_url": "https://gateway.example/v1",
        "favorite_models": [
          "*-sonnet-*",
          "*-opus-*"
        ]
      }
    }
  }
}
```

`/model` lists every model, newest first. Favorites come first, each with a star. Batch routes (`:batch`) are not listed, because they cannot answer an interactive turn.

The lists are joined: `models.favorite_models` matches any model, and a provider's own list matches only that provider's models. A pattern matches the catalog key or the routing slug, case-sensitive.

---

## Overriding Built-in Models

You can override specific fields of built-in models without redefining everything. Only specify the fields you want to change:

```json
{
  "models": {
    "definitions": {
      "grok-4.6": {
        "temperature": 0.5,
        "api_key": "sk-custom"
      }
    }
  }
}
```

The `grok-4.6` entry above overrides the temperature and the API key. A block with only `api_key` overrides only the key.

When you override a built-in model, Grok starts with the default configuration (including the correct `base_url`), then applies only the fields you specify. Unspecified fields inherit from the default.

### Priority Order

1. Your config (`models.definitions.*`) -- highest priority
2. Prefetched models from remote `/v1/models`
3. Hardcoded defaults -- lowest priority

---

## Provider Examples

### Anthropic (Claude)

Use Claude models directly via the Anthropic Messages API:

```json
{
  "models": {
    "definitions": {
      "claude-opus": {
        "api_backend": "messages",
        "base_url": "https://api.anthropic.com/v1",
        "context_window": 200000,
        "extra_headers": {
          "anthropic-version": "2023-06-01",
          "x-api-key": "sk-ant-..."
        },
        "model": "claude-opus-4-6",
        "name": "Claude Opus 4.6"
      }
    }
  }
}
```

The `messages` backend uses the Anthropic Messages protocol. Anthropic authenticates with an `x-api-key` header rather than `Authorization: Bearer`, so pass your key through `extra_headers`, which Grok sends verbatim.

For more than one Claude model, move the shared half to a provider block:

```json
{
  "models": {
    "definitions": {
      "claude-haiku": {
        "model": "claude-haiku-4-5",
        "model_provider": "anthropic",
        "name": "Claude Haiku 4.5"
      },
      "claude-opus": {
        "model": "claude-opus-4-6",
        "model_provider": "anthropic",
        "name": "Claude Opus 4.6"
      }
    },
    "providers": {
      "anthropic": {
        "api_backend": "messages",
        "base_url": "https://api.anthropic.com/v1",
        "context_window": 200000,
        "env_http_headers": {
          "x-api-key": "ANTHROPIC_API_KEY"
        },
        "extra_headers": {
          "anthropic-version": "2023-06-01"
        }
      }
    }
  }
}
```

### OpenAI (Chat Completions)

```json
{
  "models": {
    "definitions": {
      "gpt-4o": {
        "base_url": "https://api.openai.com/v1",
        "env_key": "OPENAI_API_KEY",
        "model": "gpt-4o",
        "name": "GPT-4o"
      }
    }
  }
}
```

`api_backend` defaults to `"chat_completions"`, so you don't need to set it explicitly for OpenAI.

### OpenAI (Responses API)

If your provider supports the newer Responses API:

```json
{
  "models": {
    "definitions": {
      "gpt-4o-responses": {
        "api_backend": "responses",
        "base_url": "https://api.openai.com/v1",
        "env_key": "OPENAI_API_KEY",
        "model": "gpt-4o",
        "name": "GPT-4o (Responses)"
      }
    }
  }
}
```

On the Responses API, Grok asks for a `concise` reasoning summary by default; that is what the reasoning text shown in the UI comes from. `reasoning_summary` changes the request: `detailed` or `auto` for a fuller summary, or `none` to omit the field for gateways that reject it.

### AWS Bedrock (Mantle)

Bedrock's OpenAI-compatible gateway rejects `reasoning.summary`, so set `reasoning_summary = "none"`. It authenticates with a Bedrock API key as a bearer token; the example below mints a short-lived one through a named auth provider:

```json
{
  "models": {
    "auth_providers": {
      "bedrock": {
        "command": "aws-bedrock-token",
        "token_ttl_secs": 3600
      }
    },
    "definitions": {
      "bedrock-grok-4.6": {
        "api_backend": "responses",
        "auth_provider": "bedrock",
        "base_url": "https://bedrock-mantle.us-west-2.api.aws/openai/v1",
        "context_window": 500000,
        "model": "xai.grok-4.6",
        "name": "Grok 4.6 (Bedrock)",
        "reasoning_summary": "none"
      }
    }
  }
}
```

### Ollama and LM Studio (local models)

Declare the provider and every model it serves shows up in `/model`. That Declare is with its real context window, its capabilities and a dot showing whether it is loaded in VRAM right now:

```json
{
  "models": {
    "providers": {
      "ollama": {}
    }
  }
}
```

That is the whole configuration. The provider id fills in the endpoint (`http://localhost:11434/v1`), the listing dialect and the pricing switch. LM Studio is the same:

```json
{
  "models": {
    "providers": {
      "lmstudio": {}
    }
  }
}
```

Anything you write yourself wins over those defaults:

```json
{
  "models": {
    "providers": {
      "ollama": {
        "base_url": "http://workstation.local:11434/v1",
        "context_window_source": "loaded",
        "favorite_models": [
          "qwen3-coder*"
        ]
      }
    }
  }
}
```

**Why this matters.** Asked through the OpenAI-compatible `/v1/models`, a local runtime reports an id and nothing else, so every model gets the client's default 256k window. Ollama actually loads a runner at whatever your VRAM allowed — frequently 4k — and then silently drops the oldest messages once the prompt overflows. The harness never compacts, and the conversation loses its head with no error. Reading the runtime's own listing is what makes the number true.

The green dot beside a model in `/model` means it is resident in VRAM. A dim dot means it is on disk and will have to load first. Models that are not from a local runtime have no dot at all. The dot keeps up with the runtime on its own: a model that loads on its first request, or that LM Studio's idle TTL unloads. Changes colour within a few seconds without a start.

#### Pinning the window and keeping the model warm

To set them, use the native backend:

```json
{
  "models": {
    "providers": {
      "ollama": {
        "api_backend": "ollama",
        "base_url": "http://localhost:11434",
        "extra_body": {
          "keep_alive": "30m",
          "options.num_ctx": 32768,
          "truncate": false
        }
      }
    }
  }
}
```

LM Studio's compatible endpoint does accept extra body fields. It needs no backend change:

```json
{
  "models": {
    "providers": {
      "lmstudio": {
        "extra_body": {
          "ttl": 1800
        }
      }
    }
  }
}
```

`extra_body` also works on a single `models.definitions.<id>` block, and a model's own value wins over its provider's key by key.

Make sure the runtime runs (`ollama serve`, or LM Studio's server tab) and the model is pulled (`ollama pull qwen3-coder:30b`).

### Together AI

```json
{
  "models": {
    "definitions": {
      "together-mixtral": {
        "base_url": "https://api.together.xyz/v1",
        "env_key": "TOGETHER_API_KEY",
        "model": "mistralai/Mixtral-8x7B-Instruct-v0.1",
        "name": "Mixtral 8x7B"
      }
    }
  }
}
```

### Local OpenAI-Compatible Server

Any server that implements the OpenAI Chat Completions or Responses API:

```json
{
  "models": {
    "definitions": {
      "local-llama": {
        "base_url": "http://localhost:8080/v1",
        "model": "llama-3.1-70b",
        "name": "Local Llama",
        "temperature": 0.8
      }
    }
  }
}
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

```json
{
  "endpoints": {
    "models_base_url": "https://api.acme.com/v1"
  },
  "models": {
    "definitions": {
      "grok-4.6": {
        "api_key": "my-api-key"
      }
    }
  }
}
```

When you use `endpoints` with partial model overrides, Grok inherits the `base_url` from the endpoints config. You do not need to specify it in each `models.definitions.*` section.

### Auth Behavior

When you set `models_base_url`, Grok uses API key auth (`Authorization: Bearer`) instead of session auth. You do not need `grok login` -- the API key is enough.

---

## Web Search Model

The `web_search` tool uses a separate model. Configure it with:

```json
{
  "models": {
    "web_search": "grok-4.5"
  }
}
```

Or via environment variable:

```bash
export GROK_WEB_SEARCH_MODEL="grok-4.5"
```

If you point web search at a custom model, you also need a `models.definitions.*` entry so Grok can reach it. Server-side ("backend") web search runs only when the model sets `"supports_backend_search": true` (and the build enables backend search). It does not depend on `api_backend`:

```json
{
  "models": {
    "definitions": {
      "my-custom-model": {
        "model": "my-custom-model",
        "supports_backend_search": true
      }
    },
    "web_search": "my-custom-model"
  }
}
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

# Set as default in config.json, under models.default:
#   "models": { "default": "my-model" }
```

---

## Enterprise Deployment

A complete config for an enterprise deployment with custom models:

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

## Troubleshooting

### Model Not Found

```bash
# List available models
grok models

# Check config.json for typos in models.definitions.* sections
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
