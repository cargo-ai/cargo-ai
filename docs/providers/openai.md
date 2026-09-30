# OpenAI

[Provider guide](./README.md) · [Documentation home](../README.md) · [Public README](../../README.md)

Use `--server openai` for either an OpenAI account session shared with Codex or
a direct OpenAI API key. These are separate authentication and billing paths;
choose one profile mode deliberately.

## Option 1: OpenAI Account Session

This is the recommended first-run path when your ChatGPT workspace includes
Codex access. Install the [Codex CLI](https://learn.chatgpt.com/docs/codex/cli),
then create and authenticate the profile:

```bash
cargo ai profile add openai-account \
  --server openai \
  --model gpt-5.6-terra \
  --auth openai_account

cargo ai auth login openai --profile openai-account --set-default
```

`cargo ai auth login openai` runs the Codex login flow, validates the resulting
session, clears any Cargo AI-local logout state, marks the named profile as
`openai_account`, and optionally makes it the default. Cargo AI treats Codex
auth storage as the source of truth and does not copy the account session token
into its own credential store.

Account-session inference uses
`https://chatgpt.com/backend-api/codex/responses` and sends `store = false`.
If the cached Codex session is missing, expired, or locally disabled, Cargo AI
fails with login guidance instead of falling back to an API key.

In a build containing account model discovery, list choices without knowing a
valid current model:

```bash
cargo ai models list --profile openai-account
# An existing Codex login also supports a draft without creating a profile:
cargo ai models list --server openai --auth openai_account
cargo ai profile set openai-account --model MODEL_ID
```

Use an exact ID returned by listing. Discovery performs one bounded live read
of picker-visible models and does not infer invocation access or feature support.
It reads the existing file-backed commercial Codex session without starting
login, refreshing tokens or writing a cache. Missing/expired sessions, local
logout, unknown account context and unsupported routing fail with safe guidance;
manual model entry remains available. Keyring-only/ephemeral sessions, FedRAMP
or alternate endpoints and custom account discovery URLs are unsupported.

Account profiles follow the current Codex account/workspace. Separate Cargo AI
homes keep their own settings/local logout but share Codex identity. Refresh
the catalog after switching accounts; a relevant session change during listing
fails safely. Native account text/image requests carry the selected context only
to the trusted Codex HTTPS endpoint and refuse redirects. Rebuild generated
agents to obtain this context handling. See the [machine contract](../machine-interface.md#models)
for catalog and caller boundaries.

Official OpenAI documentation describes ChatGPT sign-in as subscription access
and lists `gpt-5.6-terra` as a current model balancing capability and cost.
Workspace model availability can still differ. See
[OpenAI authentication](https://learn.chatgpt.com/docs/auth) and the
[GPT-5.6 Terra model page](https://developers.openai.com/api/docs/models/gpt-5.6-terra).
Verified: 2026-08-30.

## Option 2: Direct OpenAI API Key

Create an API key in your OpenAI API organization, then store it through
standard input:

```bash
cargo ai profile add openai-api \
  --server openai \
  --model gpt-5.6-terra \
  --auth api_key

printf '%s' "$OPENAI_API_KEY" | cargo ai profile set openai-api --stdin
cargo ai run --config ./agent.json --profile openai-api
```

PowerShell credential step:

```powershell
$env:OPENAI_API_KEY | cargo ai profile set openai-api --stdin
```

Direct API-key inference defaults to
`https://api.openai.com/v1/chat/completions`. Use `--url` only for a complete
compatible Chat Completions endpoint. API-key requests use the API
organization's usage and data controls rather than the ChatGPT workspace's
subscription controls. See the official
[OpenAI API authentication reference](https://platform.openai.com/docs/api-reference/authentication).
Verified: 2026-08-30.

## Capabilities And Boundaries

Cargo AI's OpenAI adapter supports text, client-fetched URL text, local image
input, direct file input, strict JSON-schema-directed output, normalized usage,
and interpreted or hatched execution.

OpenAI also supports Cargo AI `generate_image` actions:

- API-key profiles use the OpenAI image generation endpoint and use image edits
  when `reference_images` are present.
- Account profiles use the Codex Responses image tool and pass reference images
  as Responses image inputs.
- A step-level profile and explicit image model can keep image generation
  separate from the main inference model.

Audio actions require a compatible model and API-key profile. OpenAI API-key profiles provide `generate_audio` for `.wav` or `.mp3` speech using a selected model and provider voice, and `transcribe_audio` for local `.wav` or `.mp3` files up to 10 MiB. A transcription result is captured as text for later steps in the same action. These two audio steps are unavailable through the OpenAI account transport; use an API-key step profile when an account-backed parent needs them.

Model access and feature support remain model-specific. The recommended model
above is a representative onboarding choice, not certification of every OpenAI
model or guarantee of access in every workspace.

## Strict Failure Behavior

Cargo AI requests strict structured output and validates the returned JSON
locally before actions run. It does not weaken the authored schema, change the
model, switch between account and API-key auth, or fall back to another
provider after a rejection or invalid response.

Keep API keys out of agent JSON and command arguments. Use the stdin flow above;
account-session credentials remain owned by Codex.

## Related Documentation

- [All providers](./README.md)
- [Build and run your first agent](../getting-started.md)
- [Actions and child agents](../actions-and-child-agents.md)
- [Troubleshooting](../troubleshooting.md)
- [Cargo AI Home](../cargo-ai-home.md)
- [Documentation home](../README.md)
- [Public README](../../README.md)
