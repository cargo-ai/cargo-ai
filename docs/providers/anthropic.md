# Anthropic

[Provider guide](./README.md) · [Documentation home](../README.md) · [Public README](../../README.md)

Use `--server anthropic` for Claude through Anthropic's native Messages API.
This adapter requires `auth = "api_key"`; `none` and `openai_account` are not
valid Anthropic auth modes.

## Set Up The Profile

Create a Claude Console API key and choose a model that the Console organization
can access. A paid Claude consumer subscription does not supply Console API
credits or an API key; the products are billed separately. Anthropic's official
[authentication guide](https://platform.claude.com/docs/en/manage-claude/authentication)
and [subscription/API separation explanation](https://support.claude.com/en/articles/9876003-i-have-a-paid-claude-subscription-pro-max-team-or-enterprise-plans-why-do-i-have-to-pay-separately-to-use-the-claude-api-and-console)
confirm this setup. Verified: 2026-08-30.

```bash
cargo ai profile add anthropic \
  --server anthropic \
  --model YOUR_CLAUDE_MODEL \
  --auth api_key \
  --max-output-tokens 4096

printf '%s' "$ANTHROPIC_API_KEY" | cargo ai profile set anthropic --stdin
cargo ai run --config ./agent.json --profile anthropic
```

PowerShell credential step:

```powershell
$env:ANTHROPIC_API_KEY | cargo ai profile set anthropic --stdin
```

Replace `YOUR_CLAUDE_MODEL` with an exact model ID available to your Console
organization. Cargo AI does not maintain a Claude model allowlist.

## Endpoint And Output Limit

The default endpoint is `https://api.anthropic.com/v1/messages`. Anthropic's
official API reference identifies the same Messages path. Verified: 2026-08-30.

If `max_output_tokens` is omitted, Cargo AI uses `4096`. The cap covers the
provider's entire generated output; for models that spend output tokens on
thinking, a very small cap can finish without a text block. Raise
`--max-output-tokens` if that occurs.

Use `--url` only for a complete endpoint implementing Anthropic's native
Messages request and response contract. An OpenAI-compatible facade is not a
drop-in replacement for this provider identity.

## Capabilities And Boundaries

Cargo AI supports:

- text and client-fetched URL text
- local image input
- JSON-schema-directed text output and normalized usage
- interpreted and hatched execution

Direct file input and Anthropic `generate_image` are unsupported and fail
explicitly. An Anthropic parent can select an OpenAI or Ollama step-level
profile for image generation.

Cargo AI checks Anthropic's known schema restrictions before sending a request.
It reports an unsupported keyword instead of removing or weakening it, and it
validates returned JSON locally before actions run. Provider or model rejection
does not trigger a model or provider fallback.

The integration is representative transport compatibility, not certification
of every Claude model/schema combination.

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
