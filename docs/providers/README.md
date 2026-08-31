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
| [Mistral API](./mistral.md) | `mistral` | `https://api.mistral.ai/v1/chat/completions` | required `api_key` | text and client-fetched URL text only |
| [Ollama](./ollama.md) | `ollama` | `http://localhost:11434/v1/chat/completions` | none for the normal local server; optional `api_key` for compatible deployments | text, client-fetched URL text, images, and forwarded files, subject to model/endpoint support |

All six provider identities support interpreted and hatched agents,
JSON-schema-directed output, and normalized usage when the provider reports
token counters. Compatible wire formats do not collapse provider identity:
diagnostics and usage continue to report `mistral`, `ollama`, or `xai` as
selected.

OpenAI and Ollama also support Cargo AI `generate_image` actions. Anthropic,
Gemini, Mistral, and xAI do not; select an OpenAI or Ollama step-level profile
when an otherwise different parent provider needs image generation.

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

Create the profile first, then pipe the provider key through standard input:

```bash
printf '%s' "$PROVIDER_API_KEY" | cargo ai profile set PROFILE_NAME --stdin
```

PowerShell:

```powershell
$env:PROVIDER_API_KEY | cargo ai profile set PROFILE_NAME --stdin
```

Standard input keeps the secret out of the command arguments. Do not put real
keys in agent JSON, examples, shell history, or source control. Cargo AI stores
profile credentials through its credential backend; see
[Cargo AI Home](../cargo-ai-home.md) for the local-state boundary.

## Strict Output And Failure Behavior

Cargo AI sends the complete authored return schema to the selected provider and
validates the returned JSON locally before actions run. Provider-specific schema
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
