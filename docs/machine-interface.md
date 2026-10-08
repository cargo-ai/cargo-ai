# Machine interface

Application callers opt into the application envelope with `--output-format json --output-schema-version 1`. Existing human output and command-specific `--json` contracts retain their meanings. In particular, `run --json` still supplies an input definition; usage `--schema-version` still selects the usage payload revision independently of the application envelope.

```sh
cargo ai capabilities --output-format json
cargo ai profile list --output-format json --output-schema-version 1
cargo ai profile show example --output-format json
cargo ai models thinking --profile example --model MODEL_ID --output-format json
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

Only `run` currently advertises NDJSON. Decode incrementally across arbitrary read boundaries. With `--role-session`, stdout also contains reliable `cargo_ai_role_session` control acknowledgements: distinguish them by `protocol` before decoding ordinary machine events. Each event has schema/event type, operation/root/invocation IDs, monotonic zero-based sequence, UTC timestamp and typed data. `operation_started` precedes runtime lifecycle events; one `operation_completed` contains the terminal envelope. Stdout contains no human banners, ANSI rendering or raw external diagnostics. Child/build diagnostics identify origin and byte counts; private text is omitted. Writers flush each record and use pipe backpressure. Individual progress records are bounded to 64 KiB; finite/terminal responses to 8 MiB. At most 63 progress records are queued, leaving room for the terminal response; excess progress is omitted with a warning. Terminal delivery waits at most two seconds. Artifact metadata is bounded to 256 records and truncation is disclosed.

Known validated root results have availability metadata. `--include-result-content` explicitly includes private result content on stdout; content is bounded to 4 MiB; it adds no result persistence, usage metadata or backup content. Produced artifacts are reported only after the production write succeeds. Existing artifact files retain their established lifecycle. Older generated children are opaque: exit status does not create a result or establish child instrumentation.

Selected execution owns child process groups on Unix and native jobs on Windows, bounds captured child output to 1 MiB per channel, and cleans up descendants on cooperative cancellation/deadline. Builds have a 600-second bound. Forced termination, crashes and failures before machine negotiation may leave incomplete output; consumers must reconcile effects. Essential output delivery failure is an invocation failure, even if the underlying operation already applied.

## Declared actions

Action-capable builds advertise `actions list`, `actions validate`, `actions resource`, `actions artifact` and the `run` action selector. Use the explicit `--project PATH` or `--package ALIAS` target; action discovery does not infer a catalog from the working directory. See [client action authoring](client-actions.md) and [action payloads](machine-payloads.md#declared-client-actions).

```sh
cargo ai actions list --project ./project --output-format json
cargo ai actions validate --project ./project --interface panels --action generate-selected --request-stdin --output-format json < request.json
cargo ai actions resource --project ./project --interface panels --resource controls --request-stdin --output-format json < resource-request.json
cargo ai actions artifact --project ./project --interface panels --request-stdin --output-format json < artifact-request.json
cargo ai run --project ./project --interface panels --action generate-selected --action-request-stdin --output-format ndjson < request.json
```

The existing list/validate/resource/artifact operations are passive reads/validation. They do not compile or execute code, resolve provider credentials, contact providers or initialize incidental home state. Native `actions resolve` additionally reads the selected protected connection context without repairing it or contacting a provider. `validate` reports `execution_authorized:false`: it does not grant permission or certify later execution. Business input values are not echoed in its mapping descriptors. A file/image grant may read the authorized attachment to verify its supplied digest.

Catalog/action request 2 remain supported; native roles explicitly select catalog/action request 3. Catalog/request 1 and unknown revisions fail before execution. Outer machine/event, execution-policy and static-resource versions remain 1. The host constructs the bounded stdin request, expected catalog-wide binding, scoped attachment grants and typed execution policy. Interface/action IDs must match the flags. `run` action selection is mutually exclusive with existing definition sources; it retains root profile/settings resolution, shared package/runtime permissions and current JSON/NDJSON/private-result controls. Settings queries remain the existing profile/model commands with their own effects. Host policy constrains effective supported declarative root, media and child choices, including fallback, before their corresponding credential/provider/launch boundaries; it is not an OS sandbox or authority for arbitrary trusted tool code.

Selected tool results require definition revision `2026-10-03.r1` and advertised `structured_results.v1`; artifact reads require `artifact_access.v1`, native scope consent and a retained host-only grant. Read types/size/confinement and host renderer capabilities are distinct. See [business results and authorized reads](machine-payloads.md#shared-business-schema-and-selected-results). Runtime revision 4 retains generated validation/execution checking; revision 5 additionally advertises native roles, and revision 6 adds connection continuity without approval expiration; revision 7 adds informational capability evidence and exact native reasoning attempts. Generated runtimes still separate checking from CLI terminal delivery/artifact reads.

After resolution, terminal `data.client_action`, `client_action_resolved` progress and subsequent NDJSON event `client_action` metadata contain `{interface,action,binding}`. `operation_started` occurs before validation and has no action metadata. The operation/root/invocation IDs and terminal semantics remain the existing run lifecycle. No input values, client session handles, schedule records or idempotency promise are added to that correlation.

Catalog-wide identity includes declared target definitions and resources; changing a sibling action/resource invalidates it too. Hosts serve verified presentation bytes for one authorized document generation, and revalidate/re-enable after changes. A source binding does not freeze transitive dependencies or arbitrary same-user edits. Installed runs retain supported package leases. Missing generated-child policy/input capabilities require rebuilding or an unsupported result before launch; opaque child exit status does not establish inner telemetry or structured results.

## Thinking

Use the advertised `models thinking` JSON contract to query one explicit model and connection. `--model MODEL_ID` is required; saved-profile and draft server/auth selectors match `models list`. Configurable results expose exact suggested choices, descriptions and any authoritative default; unsupported and unknown results retain their separate meanings. Native runtime7 permits retained and custom named choices independent of these suggestions, with exact policy and adapter enforcement. The query performs no inference or profile mutation. Default runs need no thinking catalog read. Connection/authentication errors remain errors; a valid query with unknown support can succeed without a choice list. See the [typed payload](machine-payloads.md#thinking-discovery).

Profiles and invocations accept mutually exclusive `--thinking VALUE`, `--thinking-choice VALUE` and `--thinking-provider-default`; profile set also accepts conflicting `--clear-thinking`. The installed `run` capability descriptor declares tagged selection, terminal outcomes and action revision `2026-10-01.r1`. [Profile and action precedence](providers/README.md#thinking-selection) applies independently of model overrides.

Runtime `thinking_resolved` progress frames report request outcomes or passive child forwarding. Terminal JSON/NDJSON retains bounded observed records under `data.thinking` and structured fallback warnings, including when progress delivery is omitted or execution later fails. An opaque child's effective setting stays `child_unverified`. Read terminal records and coverage fields; a progress frame, saved choice or forwarded flag alone does not establish provider acceptance. [Runtime payloads](machine-payloads.md#runtime-thinking) describe the bounds and fallback codes.

CLI `capabilities` and newly generated `inspect --json` include passive `runtime_capabilities`. A declaration identifies invocation support; it does not authenticate an artifact, grant execution permission or attest to inference. Rebuild existing standalone applications to add these controls. Earlier binaries and hosted services can reject the opt-in revision; local support does not certify deployed acceptance.

## Models

```sh
cargo ai models list --profile example --output-format json
# Supply a draft key through a closed stdin pipe, never in argv:
cargo ai models list --server openai --auth api_key --stdin --output-format json
cargo ai models list --server openai --auth openai_account --output-format json
cargo ai models list --server ollama --auth none --output-format json
```

Saved API-key discovery requires explicit `secret_store = "file"`. Keychain and legacy/unset stores are refused before lookup; use a draft stdin key or manual model entry. No-auth discovery reads no credentials. TypeSafe and unsupported custom endpoint layouts require manual entry. Saved profiles need no valid current model. Draft discovery creates no profile or home and persists no credentials/cache.

OpenAI account discovery uses the existing file-backed Codex session for the default commercial Codex endpoint. It requires a valid, sufficiently fresh token and known selected account context; it never starts login or refreshes credentials. Account drafts omit `--stdin`; API keys are not interchangeable with account sessions. Keyring-only or ephemeral sessions, unsupported layouts, FedRAMP/alternate routing and custom account discovery URLs are unsupported. Local Cargo AI logout is honored for the selected home.

Account catalogs come from one direct network response, with `source:"live"` and `cache:null`; no bundled/app-server fallback is used. Only records explicitly marked picker-visible are returned. Exact slugs remain selectable IDs, including provider ordering and first-seen deduplication. Missing capability metadata stays unknown, and invocation access remains unverified. Completeness describes this visible selection, not every hidden/manual model or an immutable server snapshot. Empty visible catalogs succeed; invalid responses fail without a guessed or partially parsed account catalog.

Account catalogs add optional `compatibility: {"kind":"codex_backend","client_version":"0.159.2"}` provenance. The value identifies the adapter's outgoing query contract, not an installed Codex version or model entitlement. Older catalog payloads and other providers can omit it. Discovery does not execute Codex or derive compatibility from its installation; supported existing file-backed sessions work independently of CLI age/location. See the [compatibility policy](providers/openai.md#catalog-compatibility-policy) for scope and maintenance.

Profiles follow the current shared Codex session; separate `CARGO_AI_HOME` values isolate settings/local logout, not Codex identity. Switching the Codex account requires a fresh listing. A relevant session/configuration change during listing causes a safe changed-connection failure without retry. Catalog output contains no account ID or credential-derived fingerprint; callers must not treat a saved profile name or earlier catalog as a permanent account binding. Native account execution carries the same selected context for its invocation and refuses redirects. Existing generated agents require rebuilding to obtain that behavior.

Adapters list OpenAI, Anthropic, Gemini, Mistral, xAI and native Ollama catalogs. Fixture-qualified transport support is distinct from live-qualified access. Listing returns exact selectable IDs in provider order, optional provider metadata, live freshness, pages fetched and completeness; it never infers capabilities from model names or proves inference access. Gemini resource names are retained while its selectable ID removes the documented `models/` prefix. Complete empty lists are successful. Partial results are explicitly identified.

Discovery follows pagination within one connection/query for at most 20 requests, 1 MiB per response page and 30 seconds total. `--page-limit` may reduce the page budget. Reaching a page or time bound after a valid page returns partial coverage without a resumable cursor; a timeout before any valid page remains a failure. A new invocation is a fresh read with its explicitly supplied connection. Redirects and automatic retries are disabled; 401/403/429 never trigger inference or authentication. Remote authenticated endpoints require HTTPS; local Ollama supports loopback HTTP. Userinfo, query strings and fragments are refused. Error payloads contain static safe categories rather than raw provider bodies or credential-bearing URLs.

## Compatibility and integration

Use capabilities instead of help-text parsing. Profile reads and installed-package inspection supply typed fields instead of table/headline parsing. Installed lists retain the legacy default limit of 20, disclose available count/completeness, and offer the existing limit/all selectors where supported. Hosted lists preserve existing ordering/filter semantics and disclose their display limit. Usage v1/v2 payloads remain unchanged inside the explicitly selected envelope. Backup contracts describe remote acceptance and local queue/settings persistence separately.

This development interface becomes available only in a build containing it. Integration does not publish a release or upgrade users. Application wrappers consume these contracts; Cargo AI owns runtime, configuration, authentication and provider requests.


The same selection supports case-insensitive `--thinking on` / `off` and literal `--thinking-choice VALUE`. Discovery advertises qualified Boolean availability separately from named choices; [typed outcomes and runtime compatibility](machine-payloads.md#runtime-thinking) retain truthful default fallback. On/Off do not imply High/Low.

## Native roles and host control

Negotiate the selected binary's `run.native_roles` (including `definition_revision:"2026-10-06.r1"` for optional audio voice), role contract 1, catalog/request 3, session 1 and tool protocol 2 before enabling native role controls. Ordinary catalog/request 2 behavior remains available. Current generated native parents and descendants require runtime capability revision 7, native selection policy 1, connection continuity 1 and a verified exact source identity; rebuild artifacts lacking either before execution. A reported CLI capability does not upgrade installed artifacts or establish host adoption.

```sh
cargo ai profile refresh-context example --output-format json
cargo ai actions list --project ./project --output-format json
cargo ai actions resolve --project ./project --interface studio --action review --request-stdin --output-format json < private-resolve-request.json
```

`profile refresh-context` enrolls or explicitly repairs protected connection metadata. Refreshing a demonstrably unchanged connection preserves its UUID/generation. Account enrollment may fetch public verification keys; it never logs in, renews provider credentials or invokes inference. Routine callers use `profile validate-context` with the expected approved reference, adding `--renew` only to refresh verification evidence. `actions resolve` remains passive and returns advisory compatibility with `execution_authorized:false`. Elapsed time alone does not expire approval. Expired credentials or unavailable verification evidence pause execution; verified renewal preserves approval. Meaningful changes require affected review, and removal/recreation cannot revive old authority. Choices stay private outside project files and exports. See [connection continuity](machine-payloads.md#connection-continuity-and-remembered-approval).

Execute a reviewed request with `run --project ./project --interface studio --action review --action-request-stdin --role-session --output-format ndjson`. This invocation consumes LF-delimited `cargo_ai_role_session` version-1 frames: first `{type:"start",request:<action-request-3>}`, then revision-specific `revoke` or `cancel` controls. Keep stdin open while consent remains active. EOF, malformed frames or lost native control close admission and cancel the invocation; a one-shot redirected request is insufficient for this session. Each frame is at most 1,048,576 bytes including LF; its embedded request remains bounded to 65,536 bytes. Control acknowledgements bypass the progress budget and carry invocation/revision identity plus outstanding admitted work.

The session advertises `control_ack_deadline_ms:2000` and `max_control_frames:256`. The count excludes `start` and includes every successfully read later frame, including mismatched or malformed controls. Malformed controls terminate immediately; after the 256th handler/acknowledgement attempt, the invocation requests closure with `control_limit` and cancels without reading a 257th frame. The 1 MiB limit applies per frame: post-start controls have a maximum aggregate accepted wire bound of 256 MiB, plus the separately bounded start, rather than a 4 MiB session budget.

Each acknowledgement has a two-second deadline from just before nonblocking enqueue through writer completion receipt, with one pending queue slot. Full queue, write error or timeout closes admission, requests closure with `control_output_lost`, cancels and requests owned-child cleanup; it does not claim delivery. Drain stdout incrementally while sending controls. Revocation closes admission before attempting its acknowledgement; already admitted work may settle while delivery remains healthy. Acknowledgement delivery failure escalates to cancellation. Cancellation additionally requests owned-child cleanup and does not undo completed effects. Require matching acknowledgements and terminal evidence; broken delivery or missing terminal leaves completion uncertain. The terminal snapshot preserves the first closure reason; later output loss or control-limit cancellation may retain an earlier `revoked` reason. [The installed guidance](../templates/guidance/client-actions.md#native-role-authoring-and-host-flow) includes an executable host framing example.

Terminal `data.native_role_execution` reports the bounded session snapshot. Internal native-child bootstrap/admission frames are a private transport, not a host-supplied capability or page API. Pages supply declared business data; the native host owns mappings, policy, attachment/artifact grants, private profile contexts and active session control. Reusing bindings across actions never unions their consent or resources.

Generated capability framing and definition digests establish declared support/source match, not cryptographic build authenticity or an OS sandbox. Selected native/generated executables remain trusted code. Private child bootstrap uses the core-owned native channel after checks; supported package tool protocol 2 never receives it.

Finite `actions list` and `actions validate` metadata advertises both conditional payload revisions: legacy catalog 2 yields payload 2; role catalog 3 yields payload 3. Outer `schema_versions:[1]` describes the machine envelope, not the action payload. The legacy `payload_schema` field remains available; use the advertised payload variants when negotiating native roles.

## Native selection policy negotiation

Native role execution negotiates runtime revision **7** and `native_selection_policy:{version:1,capability_evidence:"informational",reasoning_choices:"exact_native_attempt",readiness:"structural_not_permission"}`. Capability status/provenance/freshness is advisory; missing, unknown, stale, unavailable or negative support does not itself deny an otherwise valid explicitly authorized request. `ready` describes complete bindings and declared constraints, never permission or remote entitlement. Context/credential verification, revocation, exact separate policies, limits and attachment grants still govern execution. Listing supplies selection suggestions, not proof of every operation.

Named thinking is already an open tagged string: `{mode:"choice",value:"Custom-Attempt"}`. Preserve case and exact value through binding, resolution and policy; provider default is `{mode:"provider_default"}` and omits reasoning. Native execution performs no thinking-catalog query or metadata-driven substitution. An adapter that can serialize the choice sends it exactly and attributes provider rejection; an adapter lacking that wire control fails explicitly before network dispatch. `applied`/`effective` describe request selection, not successful serialization or remote acceptance. OpenAI API-key image generation/edit, Ollama/xAI/Mistral image and TypeSafe text currently have no exact named reasoning field; use an explicitly selected provider default or an implemented transport, never automatic fallback. OpenAI account text/image and Gemini image retain their existing exact fields; this introduces no new image route or remote-support claim. Generic Boolean native controls are implemented only for the existing Ollama text wire mapping; other named-only adapters reject Boolean controls.

Legacy non-native runs retain their existing discovery/fallback behavior. Existing wire versions remain outer1, catalog/request3 (legacy2), role/binding/resolution1, policy1 and continuity1. The runtime7 declaration distinguishes these new semantics; revisions1–6 remain readable but native descendants require rebuilding with7. Re-resolve existing native requests because the runtime identity changed; no time-based consent expiry is introduced. Advisory evidence/status/freshness is excluded from the authority resolution digest; exact requirements, choices, action context, policy, source and verified connection still participate. Support expiry alone therefore changes the displayed information without changing authority. No frontend telemetry, usage upload or server-side verification record is added.
