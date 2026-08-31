# xAI

[Provider guide](./README.md) · [Documentation home](../README.md) · [Public README](../../README.md)

Use `--server xai` for Grok models through xAI's Responses API. `xai` is the
provider identity; Grok is the model family. This adapter requires
`auth = "api_key"`; `none` and `openai_account` are not valid xAI auth modes.

## Set Up The Profile

Create an xAI API key and choose a model that the xAI team can access. xAI's
official [API overview](https://docs.x.ai/overview) documents API-key access
and the Responses endpoint. Verified: 2026-08-30.

```bash
cargo ai profile add xai-api \
  --server xai \
  --model YOUR_GROK_MODEL \
  --auth api_key

printf '%s' "$XAI_API_KEY" | cargo ai profile set xai-api --stdin
cargo ai run --config ./agent.json --profile xai-api
```

PowerShell credential step:

```powershell
$env:XAI_API_KEY | cargo ai profile set xai-api --stdin
```

Replace `YOUR_GROK_MODEL` with an exact model ID available to the xAI team.
Cargo AI does not maintain or silently select from a Grok model catalog.

## Endpoint And Request Behavior

The default endpoint is `https://api.x.ai/v1/responses`. Cargo AI sends
`store = false`. When configured, `--max-output-tokens` maps to the Responses
`max_output_tokens` field.

Use `--url` only for a complete xAI Responses endpoint. Chat Completions is not
the transport for Cargo AI's `xai` identity.

## Capabilities And Boundaries

The current compatibility slice supports:

- text and client-fetched URL text
- strict JSON-schema-directed output and normalized usage
- interpreted and hatched execution

Image input, direct file input, provider-hosted xAI tools, and xAI
`generate_image` are unsupported and fail explicitly. An xAI parent can select
an OpenAI or Ollama step-level profile for image generation.

Cargo AI sends the complete authored return schema with strict mode and
validates the returned JSON locally before actions run. If a selected model
rejects the schema or returns malformed/schema-invalid JSON, Cargo AI does not
weaken the schema, change models, switch providers, or run downstream actions.

The provider lane verifies representative xAI wiring and the failure boundary;
it is not certification of every Grok model/schema combination.

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
