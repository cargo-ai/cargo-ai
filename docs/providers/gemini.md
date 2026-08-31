# Google Gemini

[Provider guide](./README.md) · [Documentation home](../README.md) · [Public README](../../README.md)

Use `--server gemini` for Gemini through Google's native Interactions API. This
adapter requires `auth = "api_key"`; `none` and `openai_account` are not valid
Gemini auth modes.

## Set Up The Profile

Create or select a Gemini API key in Google AI Studio, then choose a model that
the associated Google AI project can access. Google's current
[API-key guide](https://ai.google.dev/gemini-api/docs/api-key) and
[Interactions quickstart](https://ai.google.dev/gemini-api/docs/get-started)
cover the key flow and endpoint. Verified: 2026-08-30.

```bash
cargo ai profile add gemini \
  --server gemini \
  --model YOUR_GEMINI_MODEL \
  --auth api_key

printf '%s' "$GEMINI_API_KEY" | cargo ai profile set gemini --stdin
cargo ai run --config ./agent.json --profile gemini
```

PowerShell credential step:

```powershell
$env:GEMINI_API_KEY | cargo ai profile set gemini --stdin
```

Replace `YOUR_GEMINI_MODEL` with an exact model ID available to your Google AI
project. Cargo AI does not maintain a Gemini model allowlist.

## Endpoint And Storage

The default endpoint is
`https://generativelanguage.googleapis.com/v1beta/interactions`. Cargo AI sends
`store = false` for every request. It includes `generation_config.max_output_tokens`
only when the profile or run explicitly sets `--max-output-tokens`; Cargo AI
does not add a Gemini output cap otherwise.

Use `--url` only for a complete endpoint implementing the native Interactions
contract. A `generateContent` endpoint or OpenAI-compatible facade does not
match this provider adapter.

## Capabilities And Boundaries

Cargo AI supports:

- text and client-fetched URL text
- local image input
- JSON-schema-directed text output and normalized usage
- interpreted and hatched execution

Direct file input and Gemini `generate_image` are unsupported and fail
explicitly. A Gemini parent can select an OpenAI or Ollama step-level profile
for image generation.

Cargo AI checks Gemini's known schema restrictions before sending a request. It
reports an unsupported keyword instead of removing or weakening it, and it
validates returned JSON locally before actions run. Provider or model rejection
does not trigger a model or provider fallback.

The integration is representative transport compatibility, not certification
of every Gemini model/schema combination.

## Credential Safety

Use the stdin command above for real keys. Do not place a key in agent JSON or
pass it with `--token`, where it can enter shell history or process arguments.

## Related Documentation

- [All providers](./README.md)
- [Agent definitions](../agent-definitions.md)
- [Actions and child agents](../actions-and-child-agents.md)
- [Troubleshooting](../troubleshooting.md)
- [Cargo AI Home](../cargo-ai-home.md)
- [Documentation home](../README.md)
- [Public README](../../README.md)
