# Cargo AI Usage Ledger

Use this file when a user wants token usage, provider timing, run timing, or embedding-friendly accounting for Cargo AI runs.

## What It Is

Cargo AI automatically records local usage history and offers an explicit NDJSON usage ledger export. It records Cargo AI-owned runtime metadata so a caller can understand how much provider usage a run consumed and where time was spent.

It is not a general logging backend. Keep business logs, prompts, model outputs, decision traces, custom diagnostics, and writes to Datadog, Snowflake, S3, Postgres, or other systems inside explicit tools when the user asks for them.

## Optional Per-run NDJSON Export

For interpreted JSON definitions:

```bash
cargo ai run ./my_agent.json --usage-log ./usage.ndjson
```

For embedding products or shell wrappers:

```bash
CARGO_AI_USAGE_LOG=./usage.jsonl cargo ai run ./my_agent.json
```

For hatched agents:

```bash
./my_agent --usage-log ./usage.ndjson
CARGO_AI_USAGE_LOG=./usage.jsonl ./my_agent
```

If both `--usage-log <path>` and `CARGO_AI_USAGE_LOG=<path>` are present, the explicit CLI flag wins for that process.

## File Format

- `.ndjson` and `.jsonl` are both newline-delimited JSON: one complete JSON object per line.
- Cargo AI accepts any writable path, but prefer `.ndjson` or `.jsonl` for clarity.
- The file is append-friendly. A partial file is still useful if a run fails.
- Consumers should parse each line independently and ignore unknown fields for forward compatibility.

## Event Types

Common events:

- `usage_log_started`
- `agent_run_started`
- `provider_request_completed`
- `tool_run_started`
- `tool_run_completed`
- `agent_run_completed`
- `root_run_completed`

Provider requests use `provider_request_completed` for both success and failure. Successful provider requests include normalized `usage` when the provider reports counters. Failed provider requests include safe status/timing/error metadata when Cargo AI can measure it.

## Tree Reconstruction

Use these fields to rebuild the run tree:

- `root_run_id`: one Cargo AI-generated id for the whole root run, prefixed with `cai_run_`.
- `agent_run_id`: one Cargo AI-generated id for one execution of one agent, prefixed with `cai_agent_run_`.
- `parent_agent_run_id`: null for the root agent, otherwise the parent agent execution id.
- `depth`: display/filtering convenience for nested runs.
- `launched_by`: present when a child run was launched by an agent step or tool bridge.

Do not rely on `depth` alone. Sibling agents share a depth, repeated agents can run more than once, and recursive agents can have the same artifact/name at multiple depths.

## Agent Identity

Usage events include best-available agent metadata:

- `agent.source`: examples include `local_path`, `registry`, `inline_json`, `stdin_json`, and `hatched_agent`.
- `agent.artifact`: local path or generated binary path when known.
- `agent.name`: derived or authored display name when known.
- `agent.project_root`: project root for interpreted local runs when known.
- `agent.definition_sha256`: canonical definition hash for interpreted JSON definitions when Cargo AI has the source JSON.
- `agent.generated`: true for hatched/generated binaries.

For interpreted local JSON runs, expect `source: "local_path"`, an `artifact` path such as `./my_agent.json`, a derived `name`, and `definition_sha256`.

For hatched agents, expect `source: "hatched_agent"`, `generated: true`, the executable `artifact`, and the binary name. Newly hatched agents include the canonical embedded definition hash.

## Provider And Usage Fields

Provider events include:

- `provider.server`
- `provider.profile`
- `provider.auth_mode`
- `provider.model`
- `step.kind`, such as `agent_inference` or `generate_image`
- `duration_ms`
- `timing.provider_round_trip_ms` when measurable
- `status`
- `usage.input_tokens`
- `usage.output_tokens`
- `usage.total_tokens`

TypeSafe Jev uses `provider.server = "typesafe"`. Reported input/output counters populate the matching fields; total tokens are their checked sum only when both are present. No cache/reasoning details are inferred. Usage counters describe reported token use, not Score values, native confidence, calibrated accuracy or a price estimate. Missing counters remain absent; static compatibility checking makes no inference request and supplies no inference usage.

Anthropic, Gemini, Mistral, Ollama, OpenAI, TypeSafe, and xAI usage counters are normalized into the same `input_tokens`, `output_tokens`, and `total_tokens` shape when reported. Compatible wire formats do not collapse identity: Mistral events use `provider.server = "mistral"`, and xAI Responses events use `provider.server = "xai"`. If a provider does not return counters, Cargo AI leaves the counters absent (and may leave `usage` null or omitted) instead of estimating.

## Metadata Boundary

Usage logs must stay metadata-only. They should not include:

- prompts
- model output text
- generated image bytes or file contents
- tool arguments
- tool stdout or stderr
- tool return payloads
- profile tokens
- access tokens
- refresh tokens
- raw provider response bodies

When the user wants those details, create explicit project-local tools for that logging behavior and make the data handling visible in the tool contract.

## Quick Inspection

After a run, inspect the tree fields:

```bash
jq -c 'select(.agent) | {event_type, depth, agent_run_id, parent_agent_run_id, agent}' usage.ndjson
```

Inspect provider usage totals:

```bash
jq -c 'select(.event_type == "provider_request_completed") | {agent: .agent.name, model: .provider.model, status, usage, duration_ms}' usage.ndjson
```

## Project-owned Child Logs

When a project opts in with `[runtime] data_root = ".cargo-ai/data"`, an action's child `usage_log` path resolves under that fixed directory. Installed aliases continue to use their own `data/`. These controlled relative paths reject traversal and links. An explicit top-level `--usage-log` or `CARGO_AI_USAGE_LOG` destination keeps its caller-selected meaning; the project setting does not redirect it. Legacy projects without the setting retain their previous path behavior.

## Automatic History and CLI JSON

Current runtimes collect metadata automatically into the selected Cargo AI Home at `usage/usage.sqlite3`. This history is shared across projects, profiles, parent/child runs and newly built standalone agents. No account, daemon or per-run flag is needed. Older binaries keep their original opt-in behavior; rebuild or rehatch them for automatic history.

```bash
cargo ai usage settings --json
cargo ai usage settings --tracking off
CARGO_AI_USAGE_TRACKING=off cargo ai run ./my_agent.json
cargo ai usage runs --json
cargo ai usage show cai_run_<id> --json
cargo ai usage summary --json --provider typesafe
cargo ai usage summary --json --profile my-profile --model jev
cargo ai usage export --format ndjson
cargo ai usage delete --before 2026-09-01T00:00:00Z
cargo ai usage delete --before 2026-09-01T00:00:00Z --confirm
```

`settings --tracking off` preserves recorded facts and changes the home default. `CARGO_AI_USAGE_TRACKING=off` overrides it for a process and its children. Explicit legacy `--usage-log <path>` / `CARGO_AI_USAGE_LOG=<path>` still requests an NDJSON export even when automatic history is disabled. Collection, backup consent and local/cloud deletion are separate controls.

The JSON envelope has `schema_version: 1` and `coverage`. `runs` returns `runs[]` with `root_run_id`, `agent_run_ids`, `status` and `summary`; `show` returns `events[]`. Run statuses without a recorded completion are `interrupted_or_running`, never inferred successes. Both support `--limit 1..1000` (default 100), `--cursor`, and return a string `snapshot` and nullable `next_cursor`. Keep filters unchanged when following a cursor. New writes above that snapshot do not appear on subsequent pages. Concurrent explicit deletions may remove facts from a snapshot. Export uses the same selection and pagination; its next cursor is printed on stderr while stdout remains NDJSON.

Filters are `--after` (inclusive), `--before` (exclusive), `--profile`, `--provider`, `--model` (requested) and `--resolved-model`. Times use RFC3339 UTC. Summary returns `summary.tokens.{input_tokens,output_tokens,total_tokens}` as known sums and `unknown_request_counts` for missing counters. It includes request/root/agent/tool counts, incomplete root count, error/retry fractions with explicit numerator/denominator, and `latency.{provider,run_wall_clock,tool}.{count,p50_ms,p95_ms}`. Latencies use nearest-rank percentiles; empty samples are null. `groups[].key` is `[UTC day, profile, provider, requested model, resolved model]` and each group has its own summary. Sum individual request attempts once; root and child completion events do not add tokens. Run duration is elapsed wall-clock time.

Time and attribution filters select provider usage facts. Request counts include matching completions and matching starts that still have no completion at the query snapshot; `unfinished_request_count` identifies those starts, whose missing counters each contribute to `unknown_request_counts`. Start/completion pairs count once. A start whose completion falls outside the interval does not add that completion's tokens or appear as an unfinished attempt.

Run/agent/tool status, incomplete counts and lifecycle durations use all recorded lifecycle context for the selected roots at the same snapshot. A run completed after the selected time interval therefore retains its known completion without adding out-of-window provider usage. The envelope states this distinction in `coverage.lifecycle_scope` (`selected_roots_at_snapshot`) and `coverage.usage_scope`. Root/agent/tool counts and durations describe those selected roots in full; provider counts, token sums and provider latency obey the filters. `show` and `export` return only matching events in the requested page.

Counters are exact unsigned 64-bit JSON integers; consumers using JavaScript must use a lossless JSON parser for values above `Number.MAX_SAFE_INTEGER`. A checked aggregate overflow returns an error instead of wrapping or rounding. `usage.total_tokens_source` and request `coverage` distinguish reported, valid input-plus-output derivation, and unavailable counters. Provider-native cache, reasoning and modality details can overlap headline counters; do not add all detail fields together. Requested and returned models are separate snapshots. Stable `event_id`, `operation_id` and `attempt_id` prevent replay from becoming new usage. Attempt coverage describes requests observable to Cargo AI, including failures; it does not claim visibility into internal provider retries. These facts can support independent frontend pricing; Cargo AI supplies no monetary estimate or statement of actual charges.

Provider completion is recorded before downstream output validation. A later validation or action failure does not erase the provider's counters. Safe provider request IDs/finish reasons are retained where supplied; raw errors and arbitrary provider detail fields are excluded. UTC end timestamps and monotonic durations describe request/run/tool boundaries; request start timestamps are captured immediately before bounded history persistence and provider dispatch; provider round-trip latency begins after persistence. The invocation deadline is rechecked before dispatch, and an expired deadline records `not_dispatched` with unknown counters and no latency. Run/tool starts are derived from completion wall-clock time minus monotonic elapsed duration (`timestamp_basis`). Buffered response paths do not claim first-token latency.

History is retained until explicit deletion. `usage delete` first previews a count; `--confirm` deletes only the selected local records and queued selections. It leaves cloud copies alone, and an already-transmitted upload may complete. Queries, export and management report structured errors on storage failure. Automatic collection failures preserve agent execution, print one bounded diagnostic, retain up to 100 pending metadata facts for later writes during that run, and mark incomplete coverage where possible. An unwritable home or killed process can lose unwritten facts; no recovery or complete capture is claimed for that gap.

SQLite is embedded in distributed CLI and standalone binaries: no SQLite executable/library/service or first-run engine download is required. Building from source requires the supported Rust and C toolchains. First tracked use initializes a versioned schema; compatible reopenings preserve existing data and unknown newer schemas are refused. WAL uses short transactions, FULL durability and bounded busy waits on local filesystem storage. Keep the live database outside network shares and cloud-synchronized folders. The database and its WAL/SHM/state files are mutable home data, not application package contents. Read-only empty-history queries and opt-out do not create a database.

## Optional account backup

Backup is disabled by default and is managed through `cargo ai usage backup`. `status --json` reads only local state; `status --remote --json` explicitly queries the signed-in account. `enable` selects future records, `disable` pauses upload without deleting history, and `sync` performs a bounded upload pass. The runtime and newly generated standalone agents can drain enabled queues on root-run completion without launching a CLI subprocess. Offline failures retain account-bound selections and use bounded backoff.

`include-history`, `restore` and cloud `delete` first return a preview. Their `--confirm` form requires the preview's `--account-binding` and `--generation`; historical selection also requires `--through`, and restore continuation accepts `--cursor`. Never infer consent for older history from signing in or enabling future backup. Account changes never redirect old queues. Cloud deletion revokes the upload generation; old queues cannot repopulate it. Restored facts keep event IDs and are excluded from automatic upload. Local labels can remain private; restored labels may be opaque IDs.

Backup sends only the closed usage metadata projection, never raw NDJSON, local paths, user-authored labels, prompts/responses, tool payloads, credentials or raw provider errors. Disabled backup performs no account credential lookup or cloud request. Cloud availability does not determine agent success. See the repository's `docs/usage-backup.md` for the complete command and failure contract.
