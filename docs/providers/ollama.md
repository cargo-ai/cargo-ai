# Ollama

[Provider guide](./README.md) · [Documentation home](../README.md) · [Public README](../../README.md)

Use `--server ollama` for a local Ollama server or another deployment that
implements the same OpenAI-compatible endpoints. The normal local server does
not require an API key.

## Set Up A Local Profile

[Install Ollama](https://ollama.com/download), pull a model, and create a
profile. The model below is a representative example, not a Cargo AI model
requirement.

```bash
ollama pull mistral

cargo ai profile add ollama \
  --server ollama \
  --model mistral \
  --default

cargo ai run --config ./agent.json --profile ollama
```

Cargo AI defaults to `http://localhost:11434/v1/chat/completions`. Ollama's
official [OpenAI compatibility guide](https://docs.ollama.com/api/openai-compatibility)
documents Chat Completions and the requirement to pull a model before use.
Verified: 2026-08-30.

Run `ollama list` if a model cannot be found. Cargo AI does not pull, substitute,
or select a local model automatically.

## Remote Or Authenticated Deployments

Set `--url` to the complete compatible Chat Completions endpoint. If the
deployment requires a key, use an API-key profile and store the secret through
standard input:

```bash
cargo ai profile add remote-ollama \
  --server ollama \
  --model YOUR_OLLAMA_MODEL \
  --auth api_key \
  --url https://ollama.example/v1/chat/completions

printf '%s' "$OLLAMA_API_KEY" | cargo ai profile set remote-ollama --stdin
```

PowerShell credential step:

```powershell
$env:OLLAMA_API_KEY | cargo ai profile set remote-ollama --stdin
```

Do not add `auth = "api_key"` to the normal unauthenticated local profile.

## Capabilities And Boundaries

Cargo AI's Ollama adapter supports text, client-fetched URL text, local image
input, forwarded direct file input, strict JSON-schema-directed output,
normalized usage when counters are present, and interpreted or hatched
execution. Actual image/file and schema support depends on the selected model
and compatible endpoint; rejections fail explicitly rather than falling back
to text-only behavior or another model.

Ollama also supports Cargo AI `generate_image` through its experimental
OpenAI-compatible `/v1/images/generations` endpoint:

- select an Ollama image model on the invocation or step-level profile
- use a `.png` output path
- `reference_images` are not supported on the current Ollama path
- Cargo AI expects the documented `b64_json` response

Ollama labels this image endpoint experimental, so it may change or disappear.
See the official [OpenAI compatibility guide](https://docs.ollama.com/api/openai-compatibility).
Verified: 2026-08-30.

Cargo AI sends the complete authored return schema and validates the returned
JSON locally before actions run. It does not weaken constraints, switch models,
change providers, or run downstream actions after invalid output.

The integration is representative compatibility, not certification of every
Ollama model, quantization, or compatible deployment.

## Related Documentation

- [All providers](./README.md)
- [Agent definitions](../agent-definitions.md)
- [Actions and child agents](../actions-and-child-agents.md)
- [Troubleshooting](../troubleshooting.md)
- [Cargo AI Home](../cargo-ai-home.md)
- [Documentation home](../README.md)
- [Public README](../../README.md)
