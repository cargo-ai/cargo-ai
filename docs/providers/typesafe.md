# TypeSafe Jev

[Provider guide](./README.md) · [Documentation home](../README.md) · [Public README](../../README.md)

Use `--server typesafe` with an API-key profile for hosted Jev inference. Cargo AI remains a local Rust client; selected text leaves the machine for TypeSafe. No SDK, companion runtime or local model weights are required.

Jev support requires Cargo AI **0.4.2 or newer**. See the [0.4.2 release notes](../../releases/0.4.2.md) and [release status](../testing-and-release-qualification.md#release-status) for publication availability; the published 0.4.1 crate does not include this provider.

## Set Up The Profile

Obtain TypeSafe account/API access and confirm that your account can use the selected model. The example pins `jev-1.13.0`; Cargo AI does not substitute a model or resolve an alias to a permanent version for you.

```bash
cargo ai profile add jev --server typesafe --model jev-1.13.0 --auth api_key
printf '%s' "$TYPESAFE_API_KEY" | cargo ai profile set jev --stdin
cargo ai profile show jev
```

The environment variable above must already be supplied securely; never type a real key into a shell command or agent JSON. A native client can instead pipe its secure input directly to `profile set jev --stdin`. Terminal stdin is rejected. PowerShell users can pipe `$env:TYPESAFE_API_KEY` to the same command. See [credential storage](./README.md#store-api-keys-safely). Creating this profile preserves an existing default unless explicitly requested. If no default exists, the new profile becomes the default.

The default endpoint is `https://api.typesafe.ai/v1/systemone`. A custom `--url` must implement that protocol. Leave both temperature and max-output-tokens unset; these settings are unsupported, including inherited profile values:

```bash
cargo ai profile set jev --clear-temperature
cargo ai profile set jev --clear-max-output-tokens
```

## Author And Check Choice/Score

Copy the complete [Jev example](../../templates/guidance/examples/jev-choice-score.json) to `./triage.json`. It returns a department label and a 0–100 urgency score. At urgency >= 75, it prints a local `JEV_HIGH_URGENCY` marker with platform-specific echo steps. It sends no email and writes no files.

**The new rubric revision `2026-09-19.r1` supports local and hatched execution. Hosted account storage of this revision is deferred.** Keep ordinary scaffolds on `2026-09-09.r1` when rubric is unnecessary. Existing strict and legacy definitions retain their contracts. Use an updated CLI and re-hatch to run new rubric definitions; old binaries do not gain support automatically.

```bash
cargo ai hatch triage --config ./triage.json --check --profile jev
cargo ai run --config ./triage.json --profile jev
cargo ai hatch triage --config ./triage.json --profile jev
```

Run the resulting standalone executable with `--profile jev` using the existing profile configuration. The hatch target (`--profile` or `-P`) is a compatibility check, not an embedded runtime lock. Without this target, hatch retains generic checks even when a default profile exists. An explicitly missing profile fails.

The assessment reads provider/model/auth-mode/settings metadata without loading secrets, discovering models or making inference calls. It covers the declared schema, input kinds and known settings before compilation. Other ordinary hatch work can still fetch account definitions or audit tools. Static success cannot certify credentials, live model availability, context-token fit, answer quality, fetched content, runtime overrides or dynamic children. Runtime checks the actual invocation and each child's effective provider.

## Supported Contract

| Field/input | Support |
| --- | --- |
| Flat root object | All declared outputs required; unknown output fields rejected |
| String enum with nonblank description | Choice: 1–255 unique nonblank exact labels; category distinctions belong in the description |
| Top-level number with description, inclusive range and explicit rubric | Score: 2–10 ordered nonblank descriptive levels in revision `2026-09-19.r1` |
| Ordinary bounded number, integer, boolean, free-form string, structured/nullable field | Unsupported; use a capable profile or intentionally redesign the authored task |
| Text and URL | Text supported; Cargo AI fetches URL text using its existing HTTP behavior |
| Image or file | Unsupported before Jev inference; no automatic extraction or conversion |

A rubric requires finite `minimum < maximum` and a finite span. Exclusive bounds and nested/integer rubrics fail definition validation on every provider. Bounds alone never imply Score semantics. Criteria need meaningful descriptions in low-to-high order; validation cannot establish their quality.

For N levels and native score s in [0,N−1], Cargo AI maps to authored [a,b] using `a + (s / (N - 1)) * (b - a)`. Three levels on [0,100] map 1.6 to 80. Exact endpoints and fractional results are retained. Invalid scores are rejected without clamping, and the mapped output passes local validation before actions. When switching to a compatible general provider, ordered rubric descriptions/range become instructions in the field description; unsupported rubric metadata is removed from its wire schema. Answers need not match between providers.

The adapter sends text parts in order and maps field descriptions and labels/rubrics to questions. Missing/extra answers, wrong primitive types, unknown labels, malformed responses or invalid scores fail before downstream actions. Native probabilities, confidence, distributions, Score legends and Noul are not exposed. An intentionally authored unknown option is an ordinary label, not calibrated abstention.

An empty action-only root skips inference and its Jev certification. Children and image-generation steps use their own compatible effective profiles. TypeSafe does not support `generate_image`; select a supported OpenAI or Ollama step profile. A failed child cannot undo earlier successful actions.

## Limits And Failures

TypeSafe documents 64k tokens per request and 32k for state plus the longest question (verified 2026-09-19). Cargo AI has no exact local Jev tokenizer: static checks do not certify token fit or silently truncate input. Existing Cargo AI data limits still apply. Server context errors, 401/422/429/529 responses, timeouts and malformed output fail with sanitized diagnostics. Check the account/model, settings, described rubric and provider limits before an explicit retry. The direct HTTP adapter adds no automatic retries or fallback.

Usage is normalized when reported; absent counters are not estimated. Inspect [usage guidance](../../templates/guidance/usage-ledger.md) for fields and missing-value semantics. JSON validity establishes the output contract, not judgment correctness: test representative labeled inputs before relying on the decisions.

Official references: [API](https://docs.typesafe.ai/api), [Choice](https://docs.typesafe.ai/primitives/choice), [Score](https://docs.typesafe.ai/primitives/score), [models](https://docs.typesafe.ai/models), [model limits](https://docs.typesafe.ai/model-jaggedness/jev-1.13).

## Related Documentation

- [Agent definitions](../agent-definitions.md)
- [Actions and child agents](../actions-and-child-agents.md)
- [Troubleshooting](../troubleshooting.md)
- [Complete rubric contract](../../templates/guidance/agent-definition-contract.md#explicit-rubric-scores-and-typesafe-jev)
