# Machine interface

Application callers opt into the application envelope with `--output-format json --output-schema-version 1`. Existing human output and command-specific `--json` contracts retain their meanings. In particular, `run --json` still supplies an input definition; usage `--schema-version` still selects the usage payload revision independently of the application envelope.

```sh
cargo ai capabilities --output-format json
cargo ai profile list --output-format json --output-schema-version 1
cargo ai profile show example --output-format json
cargo ai usage runs --json --schema-version 2 --output-format json
cargo ai run agent.json --output-format ndjson --max-runtime-in-sec 60
```

`capabilities` reads compiled support without opening configuration, creating a home, checking updates, migrating credentials, contacting accounts/providers, compiling or executing package code. Its presence describes installed capability; it does not prove credentials, permissions or provider access. Unknown command/format/revision combinations fail before execution. Commands absent from this map retain their existing interfaces.

Each finite response contains schema and payload identities, command/build/request identity, safe selected-home context, outcome, typed data, warnings, structured error and terminal completion. Exit zero means `succeeded`; partial, failed and requires-interaction outcomes are nonzero; cooperative cancellation uses 130. Unknown outcomes and missing terminal records never imply success. Correlation IDs are not idempotency keys. Mutation data separates applied, unapplied, unknown and not-attempted effects; reconcile authoritative state before retrying an uncertain mutation. Account acceptance does not establish email delivery, and publication acceptance does not establish local receipt persistence.

Machine mode bypasses incidental startup writers. Necessary credential preparation and explicit command persistence remain command-owned effects. It does not change the legacy credential backend: OS keychain entries are shared by profile name rather than isolated by home. Selected homes also do not isolate shared project writes or establish an OS sandbox.

## Interaction and secrets

Machine requests are noninteractive. `profile remove --yes` explicitly consents to removal. Adding an existing profile returns an interaction requirement; use the separate `profile set` command for explicit updates. API keys and confirmation codes use bounded nonterminal stdin or established credential mechanisms; machine profile/run requests reject argv tokens. Do not place secrets in command lines, logs or JSON diagnostics.

OpenAI browser login is an explicit format exception: the selected invocation returns `requires_interaction` before launching a helper. Complete the existing `cargo ai auth login openai` terminal/browser protocol, then use the existing `cargo ai auth status --json` verification contract. Its short-lived authentication material remains in that existing interaction flow. Agent pull's existing `--stdout` continues to return a raw definition in legacy mode; selected JSON returns the definition as typed envelope data.

## Runtime stream

Only `run` currently advertises NDJSON. Decode incrementally across arbitrary read boundaries. Each event has schema/event type, operation/root/invocation IDs, monotonic zero-based sequence, UTC timestamp and typed data. `operation_started` precedes runtime lifecycle events; one `operation_completed` contains the terminal envelope. Stdout contains no human banners, ANSI rendering or raw external diagnostics. Child/build diagnostics identify origin and byte counts; private text is omitted. Writers flush each record and use pipe backpressure. Individual progress records are bounded to 64 KiB; finite/terminal responses to 8 MiB. At most 63 progress records are queued, leaving room for the terminal response; excess progress is omitted with a warning. Terminal delivery waits at most two seconds. Artifact metadata is bounded to 256 records and truncation is disclosed.

Known validated root results have availability metadata. `--include-result-content` explicitly includes private result content on stdout; content is bounded to 4 MiB; it adds no result persistence, usage metadata or backup content. Produced artifacts are reported only after the production write succeeds. Existing artifact files retain their established lifecycle. Older generated children are opaque: exit status does not create a result or establish child instrumentation.

Selected execution owns child process groups on Unix and native jobs on Windows, bounds captured child output to 1 MiB per channel, and cleans up descendants on cooperative cancellation/deadline. Builds have a 600-second bound. Forced termination, crashes and failures before machine negotiation may leave incomplete output; consumers must reconcile effects. Essential output delivery failure is an invocation failure, even if the underlying operation already applied.

## Models

```sh
cargo ai models list --profile example --output-format json
# Supply a draft key through a closed stdin pipe, never in argv:
cargo ai models list --server openai --auth api_key --stdin --output-format json
cargo ai models list --server ollama --auth none --output-format json
```

Saved API-key discovery requires explicit `secret_store = "file"`. Keychain and legacy/unset stores are refused before lookup; use a draft stdin key or manual model entry. No-auth discovery reads no credentials. OpenAI account login, TypeSafe and unsupported custom endpoint layouts currently require manual entry. Saved profiles need no valid current model. Draft discovery creates no profile or home and persists no credentials/cache.

Adapters list OpenAI, Anthropic, Gemini, Mistral, xAI and native Ollama catalogs. Fixture-qualified transport support is distinct from live-qualified access. Listing returns exact selectable IDs in provider order, optional provider metadata, live freshness, pages fetched and completeness; it never infers capabilities from model names or proves inference access. Gemini resource names are retained while its selectable ID removes the documented `models/` prefix. Complete empty lists are successful. Partial results are explicitly identified.

Discovery follows pagination within one connection/query for at most 20 requests, 1 MiB per response page and 30 seconds total. `--page-limit` may reduce the page budget. Reaching a page or time bound after a valid page returns partial coverage without a resumable cursor; a timeout before any valid page remains a failure. A new invocation is a fresh read with its explicitly supplied connection. Redirects and automatic retries are disabled; 401/403/429 never trigger inference or authentication. Remote authenticated endpoints require HTTPS; local Ollama supports loopback HTTP. Userinfo, query strings and fragments are refused. Error payloads contain static safe categories rather than raw provider bodies or credential-bearing URLs.

## Compatibility and integration

Use capabilities instead of help-text parsing. Profile reads and installed-package inspection supply typed fields instead of table/headline parsing. Installed lists retain the legacy default limit of 20, disclose available count/completeness, and offer the existing limit/all selectors where supported. Hosted lists preserve existing ordering/filter semantics and disclose their display limit. Usage v1/v2 payloads remain unchanged inside the explicitly selected envelope. Backup contracts describe remote acceptance and local queue/settings persistence separately.

This development interface becomes available only in a build containing it. Integration does not publish a release or upgrade users. Application wrappers consume these contracts; Cargo AI owns runtime, configuration, authentication and provider requests.
