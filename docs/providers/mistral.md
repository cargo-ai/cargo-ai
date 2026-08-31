# Mistral API

[Provider guide](./README.md) · [Documentation home](../README.md) · [Public README](../../README.md)

Use `--server mistral` for hosted Mistral models through Mistral's Chat
Completions API. This adapter requires `auth = "api_key"`; `none` and
`openai_account` are not valid Mistral auth modes.

## Set Up The Profile

Activate Mistral Studio, create an API key, and choose a model that the
workspace can access. Mistral's current setup guide says Free mode is enabled
by default without a credit card, subject to usage and rate limits, and
documents secure key creation. See
[Activate Studio and generate an API key](https://docs.mistral.ai/getting-started/quickstarts/studio/activate-and-generate-api-key).
Verified: 2026-08-30.

```bash
cargo ai profile add mistral-api \
  --server mistral \
  --model YOUR_MISTRAL_MODEL \
  --auth api_key

printf '%s' "$MISTRAL_API_KEY" | cargo ai profile set mistral-api --stdin
cargo ai run --config ./agent.json --profile mistral-api
```

PowerShell credential step:

```powershell
$env:MISTRAL_API_KEY | cargo ai profile set mistral-api --stdin
```

Replace `YOUR_MISTRAL_MODEL` with an exact model ID available to the Mistral
workspace. Cargo AI does not maintain or silently select from a model catalog.

## Endpoint And Request Behavior

The default endpoint is `https://api.mistral.ai/v1/chat/completions`, matching
Mistral's official Chat Completions reference. When configured,
`--max-output-tokens` maps to the Chat Completions `max_tokens` field.

Use `--url` only for a complete compatible Chat Completions endpoint. Other
native Mistral platform services do not match this adapter.

## Capabilities And Boundaries

The current compatibility slice supports:

- text and client-fetched URL text
- strict custom `json_schema` output and normalized usage
- interpreted and hatched execution

Image input, direct file input, native Mistral platform services, and Mistral
`generate_image` are unsupported and fail explicitly. A Mistral parent can
select an OpenAI or Ollama step-level profile for image generation.

Cargo AI preserves the provider identity even though the wire format is
OpenAI-compatible. It sends the complete authored return schema in strict mode
and validates the returned JSON locally before actions run. A schema rejection
does not cause Cargo AI to weaken the schema, change models, switch providers,
or run downstream actions.

The provider lane verifies representative Mistral wiring and the failure
boundary; it is not certification of every Mistral model/schema combination.

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
