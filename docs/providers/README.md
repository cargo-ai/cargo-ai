# Model Providers

[Documentation home](../README.md) · [Public README](../../README.md)

Cargo AI uses connection profiles to keep provider, model, endpoint, and
authentication choices outside agent definitions. The person running an agent
selects the profile; the authoring assistant does not select the runtime
provider.

## Choose A Provider

| Provider guide | `--server` | Default inference transport | Authentication | Cargo AI input support |
| --- | --- | --- | --- | --- |
| [OpenAI](./openai.md) | `openai` | API key: `https://api.openai.com/v1/chat/completions`; account session: `https://chatgpt.com/backend-api/codex/responses` | `api_key` or OpenAI-only `openai_account` | text, client-fetched URL text, images, files |
| [Anthropic](./anthropic.md) | `anthropic` | `https://api.anthropic.com/v1/messages` | required `api_key` | text, client-fetched URL text, images; no direct files |
| [Google Gemini](./gemini.md) | `gemini` | `https://generativelanguage.googleapis.com/v1beta/interactions` | required `api_key` | text, client-fetched URL text, images; no direct files |
| [xAI](./xai.md) | `xai` | `https://api.x.ai/v1/responses` | required `api_key` | text and client-fetched URL text only |
| [TypeSafe Jev](./typesafe.md) | `typesafe` | `https://api.typesafe.ai/v1/systemone` | required `api_key` | text and client-fetched URL text only |
| [Mistral API](./mistral.md) | `mistral` | `https://api.mistral.ai/v1/chat/completions` | required `api_key` | text and client-fetched URL text only |
| [Ollama](./ollama.md) | `ollama` | `http://localhost:11434/v1/chat/completions` | none for the normal local server; optional `api_key` for compatible deployments | text, client-fetched URL text, images, and forwarded files, subject to model/endpoint support |

All seven provider identities support interpreted and hatched agents and
normalized usage when the provider reports token counters. TypeSafe translates
its supported flat enum/rubric schema to Choice/Score questions; general
providers use JSON-schema-directed output. Compatible wire formats do not collapse provider identity:
diagnostics and usage continue to report `mistral`, `ollama`, or `xai` as
selected.

Media actions use a compatible step profile and the provider's native route; text wire compatibility does not imply media support.

| Provider | `generate_image` | `generate_audio` speech | `transcribe_audio` |
| --- | --- | --- | --- |
| OpenAI API key | image generation and reference edits | WAV or MP3 | WAV or MP3 source |
| OpenAI account | Responses image tool | unsupported | unsupported |
| Gemini API key | PNG output; up to four PNG/JPEG references | WAV | WAV or MP3 source |
| Mistral API key | PNG via image tool; no references | WAV or MP3; existing saved voice ID required | WAV or MP3 source |
| xAI API key | JPEG output; up to five PNG/JPEG references | WAV or MP3; fixed service with no step model | WAV or MP3 source |
| Ollama | experimental PNG output without references | unsupported | unsupported |
| Anthropic, TypeSafe | unsupported | unsupported | unsupported |

The OpenAI, Gemini, Mistral, and xAI audio routes and the Gemini, Mistral, and xAI image routes are implemented adapters awaiting live provider verification. Their entries describe intended formats and limits, not yet established compatible support. Existing OpenAI and Ollama image support is unchanged.

New image adapters cap each reference at 10 MiB, all references at 20 MiB, and returned image bytes at 20 MiB. Transcription source files are limited to 10 MiB. Unsupported formats, providers, account transports, and models fail explicitly. A parent may select a different saved step profile for a media action. See [Actions and child agents](../actions-and-child-agents.md) for audio path and capture behavior.

## Create A Profile

Every profile needs a name, exact provider identity, and model identifier:

```bash
cargo ai profile add PROFILE_NAME \
  --server PROVIDER_NAME \
  --model MODEL_ID
```

Provider pages give complete commands, including the required auth mode and any
useful output-token setting. `--url` replaces the default with a complete
endpoint URL; the selected adapter still expects its documented protocol. For
example, an Anthropic custom URL must implement Messages, not an
OpenAI-compatible facade.

Cargo AI does not maintain a provider model catalog or silently substitute a
model. Choose a current model that the selected provider account, project,
workspace, or local Ollama installation can access.

## Store API Keys Safely

Create the profile first. A native client should start `cargo-ai` directly with arguments `profile`, `set`, `PROFILE_NAME`, `--stdin`, write the key from its secure entry field to the child's stdin pipe, and close the pipe to signal EOF. Wait for completion. Do not put real keys in shell commands/history, process arguments, agent JSON, logs, examples, or source control. Terminal stdin is rejected; this mode does not prompt.

The raw input limit is **16,384 bytes**, including trailing newline bytes. Cargo AI reads through EOF, failing as soon as the limit is exceeded, removes trailing CR/LF, and trims surrounding spaces/tabs. It rejects empty content, embedded CR/LF, NUL, and invalid UTF-8 before writing credentials. No newline, LF, and CRLF are accepted.

`--token`, `--stdin`, `--env`, and `--clear-token` remain mutually exclusive. The non-stdin interfaces remain available for compatibility; prefer the direct stdin pipe for native secret entry. Successful completion follows credential and metadata persistence; a storage/configuration failure exits nonzero and does not claim completion. A failed metadata write can leave a credential already stored, so resolve the local storage problem before repeating setup.

Use the same `CARGO_AI_HOME` and supported credential-storage configuration for profile creation, token storage, and later runs. Cargo AI uses its existing credential backend; stdin does not select a different store. See [Cargo AI Home](../cargo-ai-home.md) for the local-state boundary.

Inspect and select saved profiles without resupplying provider credentials:

```bash
cargo ai profile list
cargo ai profile show PROFILE_NAME
cargo ai profile set PROFILE_NAME --default
cargo ai run ./my_agent.json --profile PROFILE_NAME
```

Authoring-host access, runtime-provider access, and optional Cargo AI hosting are separate. Access to the assistant that authors an agent does not automatically provide provider credentials for the resulting CLI. A Cargo AI hosted account is not required for local execution and does not supply runtime-provider credentials.

## Profile Temperature

Temperature is an optional profile setting for text and image-analysis requests:

```bash
cargo ai profile set PROFILE_NAME --temperature 0
cargo ai profile show PROFILE_NAME
cargo ai profile set PROFILE_NAME --clear-temperature
```

You can also pass `--temperature` to `profile add`. An unset temperature sends no
sampling override and uses the provider/model default. This also applies to
existing profiles without this setting: earlier versions forced zero for some
models and one for others. Explicit zero is different from unset.

Explicit temperature is supported by the OpenAI API-key Chat Completions path
and OpenAI-compatible transports (Mistral and Ollama). Values must be finite and
nonnegative; the selected provider/model determines its supported range. Cargo AI
reports unsupported settings rather than silently dropping them or changing the
model. Other text transports, including OpenAI account sessions, currently require
temperature to remain unset. This setting does not configure image generation.

For models that do not accept sampling overrides, leave temperature unset.
Temperature does not select reasoning effort or guarantee identical output.

## Strict Output And Failure Behavior

Cargo AI translates the authored return contract for the selected provider and
validates returned output locally before actions run. TypeSafe supports flat
Choice/Score fields. On general providers, optional rubric metadata becomes
ordered descriptive instructions and numeric bounds remain in the schema;
definitions without rubric retain their existing request representation. Provider-specific schema
limits are surfaced as errors. Cargo AI does not remove constraints, change
models, switch providers, retry through another transport, or run downstream
actions after malformed or schema-invalid output.

The provider qualification lanes exercise representative integrations and
these failure boundaries. They are not a model allowlist or certification of
every model/schema combination. Model selection and access remain
operator-controlled.

## Related Documentation

- [Build and run your first agent](../getting-started.md)
- [Define inputs and structured output](../agent-definitions.md)
- [Use actions and step-level profiles](../actions-and-child-agents.md)
- [Troubleshoot provider failures](../troubleshooting.md)
- [Testing and Product Qualification](../testing-and-release-qualification.md)
- [Documentation home](../README.md)
- [Public README](../../README.md)
