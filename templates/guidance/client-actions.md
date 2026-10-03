# Portable Client Actions

Use this topic when a project needs buttons, native controls or page-free calls that invoke declared Cargo AI agents. Check `cargo ai capabilities --output-format json` on the client's selected binary first. These interfaces require a build advertising the action commands and action-run selector; source documentation does not establish installed or released availability.

A normal browser has no Cargo AI execution bridge. If the host lacks the action callback, or its selected binary lacks catalog/request 2 and the required capabilities, keep execution controls disabled and explain that execution is unavailable. The document may still show read-only content. The illustrative adapter disables its buttons when `host.submit` is absent and leaves them disabled after uncertain completion pending native reconciliation; a client exposes that callback only after negotiating the real CLI capabilities and grants. Do not fall back to arbitrary shell commands, legacy interface execution or guessed flags.

## Ownership and authoring

The project-root `cargo-ai-actions.json` catalog declares stable actions and their JSON entrypoints. A client chooses how to present them. An action selects a whole agent or coordinator; it does not bypass dependencies by selecting an internal agent `actions[]` step. Keep orchestration in agent definitions and trusted tools.

One interface can offer several actions: Generate this panel and Generate selected panels can bind the same coordinator with different validated values; Review can bind another agent. A native interface needs no HTML resource. Optional web presentation lists local resources explicitly. Catalog, machine-envelope, agent-definition and package-format versions are independent.

Copy the examples as one group into a new project: rename `examples/client-actions.json` to project-root `cargo-ai-actions.json`, and keep the two `client-action-*.json` agent definitions beside it. These agents print harmless markers to demonstrate dispatch; replace their steps with the project's actual work. Unix echo receives the mapped JSON as an argv value; the Windows echo example prints only a fixed marker to keep business values out of shell parsing. `examples/client-action-controls.js` demonstrates imperative controls. Its injected `host.submit` callback is illustrative client code, not a Cargo AI browser API or an installed host SDK. `examples/client-action-request.json` shows a non-web request; a host must replace its illustrative binding with current discovery and construct its own authorized execution policy. The sample `fixture` profile/model is not shipped configuration; discover and grant a configured selection. Even an action-only run retains existing root profile/settings resolution, while these marker agents skip model inference.

## Discover, validate and run

Use one explicit target: `--project PATH` for a source project or `--package ALIAS` for an installed package. These commands do not infer a catalog from the current directory.

```sh
cargo ai actions list --project ./project --output-format json
cargo ai actions validate --project ./project --interface panels --action generate-selected --request-stdin --output-format json < request.json
cargo ai run --project ./project --interface panels --action generate-selected --action-request-stdin --output-format ndjson < request.json
```

The interface/action flags must match the request. Finite responses use the existing schema-v1 machine envelope; action runs reuse `run` JSON/NDJSON, cancellation and result handling. Submit request bytes through bounded stdin, and use an argv array when a client launches Cargo AI. Do not reconstruct shell commands from page values.

Discovery also returns `capabilities` and `limits`; compare the required capabilities and honor the advertised bounds before offering controls. These describe compiled contract support, not execution consent or provider access. The strict authored catalog and the typed discovery response have different fields.

Discovery is passive: it does not build, run agents/tools, contact providers or initialize a Cargo AI home. Validation does not authorize execution. The client obtains explicit execution consent and supplies the authorized target, current binding and typed execution policy. Cargo AI validates again immediately before dispatch. A headless caller submits the same request without a document; there is no schedule flag or Cargo AI scheduling engine. The schema-v2 request contains `interface`, `action`, `inputs`, `expected_binding`, `execution_policy` and optional trusted `attachment_grants` and `artifact_access`. Validation returns binding/mapping descriptors without echoing business values.

## Inputs and identity

Declare bounded business inputs and map them explicitly to named agent inputs or scalar `runtime_vars`. Arrays and objects need explicit JSON encoding into a declared string destination. Unknown business fields, undeclared destinations, duplicate mappings and constant/input collisions fail. Keep execution authority outside business values; declared business fields with similar names remain ordinary data. Inputs and runtime variables are ordinary data, not secret stores.

A discovered binding identifies the selected root, catalog bytes, all declared entrypoint/resource bytes and optional installed-package identity. The request identifies the interface/action separately. A change to a sibling action or resource also invalidates this catalog-wide binding. Use the returned identity without deriving it from filenames. A stale catalog, definition or package fails validation/dispatch. It does not freeze transitive children/tools, establish earlier user intent or prevent arbitrary same-user filesystem edits. If authored semantics require immutable content, the input must carry the necessary content identity; the client owns capture and retention. File input references require a scoped host grant rather than a page-supplied unrestricted path.

For web presentation, also bind the complete ordered resource inventory: resource IDs, MIME types and digests, including HTML and every local script/import served to it. The host serves verified bytes for that document generation. Undeclared imports and changed bytes fail. Re-enable a new document after resource changes; a page cannot inherit an old document's authorization merely because the agent/catalog stayed unchanged.

```sh
cargo ai actions resource --project ./project --interface panels --resource controls --request-stdin --output-format json < resource-request.json
```

The resource request is `{ "schema_version": 1, "interface": "panels", "resource": "controls", "expected_binding": <discovered binding> }`, with IDs matching the flags. Returned base64 bytes are verified resource data, not a private-store path API. The client owns origin, rendering and bridge confinement. Opening or reading a document is not execution consent.

## Settings and effects

Grant settings discovery separately. Reuse `profile list`, `profile show`, `models list` and `models thinking` in machine mode; return only permitted metadata to the page. Profile listing does not inspect credentials. Model discovery has its documented credential/network requirements and does not prove inference access. Discover exact profile/model/thinking values instead of inventing names or assuming a universal thinking order.

The client constructs permitted combinations of profile, request kind, model and thinking selection, with invocation ceilings. It must not accept a page's proposed execution policy as authority. Cargo AI applies normal authored/root/child precedence and checks effective supported declarative selections against the supplied policy before the corresponding credential, provider or child-launch effects. Authored constants, dynamic input names and model output cannot widen that policy. Explicit limit flags above the policy ceilings are rejected; omitted limits and authored defaults remain capped by those ceilings. Explicit provider-default/fixed-service choices need permission too. Media and children retain their own profile precedence and thinking fallback rules. Trusted tools and local subprocesses still run with their authorized ambient effects; this policy is not an OS sandbox.

## Progress, results and cancellation

Use the existing `run` operation/root/invocation IDs, action correlation, sequence numbers and terminal envelope. Acceptance and progress do not establish completion. Missing terminal evidence stays incomplete/unknown. An action-only coordinator may succeed with root result `not_produced`; do not parse child prose into a structured result. Private result content requires `--include-result-content` and is separate from usage metadata.

A client may expose local handles and subscriptions, but those are client details. Keep ownership of active process cancellation and apply the existing process-group/native-job cleanup. Cancellation does not undo completed effects. Correlation IDs are not idempotency keys: duplicate accepted invocations can execute twice. Guard duplicate controls in the client and reconcile uncertain effects before retries. On reload or reconnect, distinguish a newly opened document from an authorized active invocation.

Generated children advertise their execution subset through passive runtime capabilities. Rebuild older executables when required input/settings/policy enforcement is absent; absent support fails before child launch. Supervised process events and exit status do not prove internal provider telemetry or structured child results. `run ALIAS::ENTRY` executes packaged JSON, including hatchable entries; it does not prove the compiled binary ran. No standalone administration or NDJSON parity is implied.

## Packaging and client integration

For the example group, select both definitions and the catalog/adapter explicitly:

```toml
[build.default]
agent_definitions = ["client-action-coordinator.json", "client-action-review.json"]
assets = ["cargo-ai-actions.json", "client-action-controls.js"]
```

The browser sample expects buttons with `data-action="generate-one"`, `data-action="generate-selected"` or `data-action="review"`, a `data-panel-id` on the single-panel button, and a `[data-current-action]` element. The client supplies the current selection function and authorized host callback. Its host builds an argv array equivalent to the command above and writes the schema-v2 request to stdin. If it chooses a root profile/model/thinking override, use permitted existing `run` flags separately from business inputs; the policy must grant the resulting effective selection.


List `cargo-ai-actions.json` and optional presentation resources explicitly under the selected build profile's `assets`; list referenced agents under `agent_definitions` or `hatched_agents`. Build/package rejects references to excluded agents rather than copying them implicitly. A package without a sidecar remains an ordinary package. A malformed/unsupported present sidecar fails before installation replaces active state. Existing package permissions, `package/` ownership, disposable `runtime/`, preserved `data/` and supported leases remain in force. See `package-workflow.md`.

Companion and other clients should consume the neutral catalog and machine request directly. Their separate integration needs capability negotiation, explicit consent, scoped attachment/settings grants, verified presentation resources, process supervision, incremental event decoding, result opt-in and cancellation. A Companion adoption replaces its affected legacy `companion.json`/`getInputs()` interface contract; Cargo AI does not ship a legacy bridge adapter. Client authors decide overlap/queue behavior, schedule persistence/timing, saved versus fresh inputs, attachment retention and handle/reload recovery. Tests of Cargo AI routing establish the upstream contract; actual controls and scheduled execution still need consumer integration tests. No universal host, scheduler, storage schema or second lifecycle is required.

To install/update this offline topic, use `cargo ai add guidance --style codex` (or `claude`), then `cargo ai guidance status` and explicit `cargo ai guidance update`. Updates preserve user-owned entry instructions and refuse locally modified managed assets; keep custom project guidance outside the managed bundle.

## Business data and selected tool results

The current package–host interface uses catalog **2** and action request **2**. Catalog/request 1 and unknown revisions are rejected before execution; there is no old/new validator switch or translation adapter. Outer machine envelopes/events, static-resource requests and execution policy retain revision 1. Discovery advertises `supported_catalog_versions:[2]`, `supported_request_versions:[2]`, `business_inputs.v1`, `structured_results.v1` and `artifact_access.v1`; the business rules apply to every current action.

`inputs` is the bounded business namespace. Its schema and a selected tool result's schema share these types: object, array, string, integer, number, boolean, null and exactly `[T,"null"]`, with the non-null type first. Objects require `properties`, an explicit `required` array (possibly empty) and `additionalProperties:false`; arrays require `items`. Supported optional keywords are descriptions, enum, string/array length bounds and inclusive/exclusive numeric bounds. References, patterns, arbitrary unions, expressions and unknown keywords are rejected. This business schema is separate from the narrower model `agent_schema`.

Business keys are nonempty UTF-8, at most 128 bytes, with no control characters. `__proto__`, `prototype` and `constructor` are rejected at every nesting level in both directions. Declared names such as `settings`, `model`, `config`, `profile`, `thinking`, `token` and `execution_policy` are ordinary data. A native host must construct policy, binding, executable/target selection and grants outside `inputs`; never merge a business object into those authority fields or flatten its keys into CLI flags. Mapping destination identifiers retain their separate restrictions. A declared object/array, null or nullable value requires explicit `encoding:"json"` into a text input or string runtime variable; ordinary non-null scalar mappings keep their declared type.

Business schema/value depth is at most 8; each object has at most 64 members, each array at most 1,024 items, and each string at most 65,536 UTF-8 bytes. Business data is at most 65,536 serialized bytes. Duplicate JSON keys fail. Numeric integers retain the JSON implementation's signed/unsigned integer range; clients must not silently round them. String IDs/versions and decimal money are useful package conventions for JavaScript hosts.

The **entire** action request is limited to 65,536 UTF-8 bytes, including native policy/binding/grants. A 64 KiB result therefore is not a promise that it can be posted back whole. The host calculates the remaining business budget after fixing its authority fields and checks actual CLI and bridge encodings. Return a bounded editable record or send a declared edit projection with an expected version; preserve drafts on oversize rejection. No field-name transformation is needed. For example, a report load can return:

```json
{"record":{"id":"report-7","settings":{"model":"editorial","config":{"language":"en","note":null}}},"version":"7"}
```

Its save action accepts `{"data":<record>,"expected_version":"7"}` under its own closed schema, maps `data` once with JSON encoding, and returns committed version `"8"`. The package owns validation/conflict statuses and optimistic transactions; runtime success alone does not prove a successful business save.

For a deterministic root result, select definition revision `2026-10-03.r1`, declare root `result`, and annotate exactly one concrete root `tool` step with `produces_result:true`:

```json
{
  "agent_definition_schema_version": "2026-10-03.r1",
  "agent_schema": {"type":"object","properties":{}},
  "result": {
    "source":"tool",
    "schema":{"type":"object","properties":{"count":{"type":"integer"}},"required":["count"],"additionalProperties":false},
    "artifact_scopes":["exports"]
  },
  "actions":[{"name":"create_report","logic":{"==":[1,1]},"run":[{"kind":"tool","name":"report_store","produces_result":true}]}]
}
```

This definition requires a project-authored `report_store` tool, declared action context and authorized `exports` scope; it is a declaration example, not a bundled executable tool. The tool uses its existing protocol. Its protocol `result` **string** contains:

```json
{"data":{"count":3},"artifacts":[{"id":"summary","scope":"exports","path":"summary.json","mime_type":"application/json"}]}
```

`data` is required, and omitted `artifacts` means `[]`. The complete decoded wrapper is at most 131,072 bytes, with at most 64 nominations and unique ASCII IDs of at most 128 bytes. Paths are portable scope-relative paths of at most 1,024 bytes. These checks apply even when private content is excluded. Protocol null means missing output; the string `{"data":null}` is valid only for a schema admitting null. Bare decoded null lacks the wrapper and is invalid.

An empty model schema skips inference. A nonempty model schema still runs its existing model pass, whose validated fields feed actions; the selected tool alone supplies the published root result. No producer fallback, child promotion, capture-name convention or stdout scraping exists. Missing/duplicate producers and unsupported annotations fail before effects. A skipped/unreached producer means `not_produced`. Ordinary agent execution without a selected producer retains its existing result semantics. The new revision includes the earlier rubric/thinking features.

Root result availability and invocation outcome remain separate. `--include-result-content` permits `data.result.content`, `artifact_references` and trusted-host-only `artifact_read_grants`; without it all three are omitted. Artifact issuance additionally requires native artifact permission. Result data captured before a later failure/cancellation remains known earlier state, never proof of final success. Malformed, missing, schema-invalid or excessive results produce safe typed failures. Earlier mutation may already have committed: do not automatically replay.

## Authorized runtime-data artifacts

Catalog 2 declares `artifact_scopes:[{"id":"exports","path":"reports","mime_types":["application/json","text/plain"]}]`; each interface names its allowed `artifact_scopes`. A definition's `result.artifact_scopes` is its maximum export set. Native action-request-2 permission is `artifact_access:{"version":1,"scopes":["exports"]}`. Definition/interface/native subsets are checked before effects; absence grants none. Catalog declarations and `actions validate` never supply consent. Plain `run` can use result-only definitions, but a result declaring artifact scopes requires the authorized action context.

Scopes are fixed subdirectories of the existing opted-in runtime-data root. Source/assembled projects must explicitly adopt runtime data in `.cargo-ai/project.toml`; installed aliases use their separate owned `data/`, retained read leases and installation incarnation. No cwd fallback, project-root override, immutable `package/` export or implicit consuming-project binding is allowed. Changed catalog/executable presentation invalidates binding; ordinary record/media changes do not. Installed update/rollback/uninstall or changed data context invalidates old grants, even when an alias later returns to identical version/content.

Each successful nomination yields a page-safe private descriptor `{schema_version:1,id,reference,mime_type,size_bytes,content_sha256}` and a separate trusted-host grant `{schema_version:1,reference,interface,binding,data_context_sha256,scope,relative_path,mime_type,size_bytes,content_sha256}`. Existing files are referenced, not falsely labeled newly produced. The reference is a domain-separated digest of canonical grant identity, **not** a signature, bearer permission or proof of historical issuance. A trusted same-user CLI caller can construct an internally consistent in-scope grant. Native consent, a bounded host registry, document/main-frame identity and exact lookup protect the untrusted page boundary; CLI confinement protects against out-of-scope access.

The host retains grants/locators privately and passes only registered descriptors to the page. To retrieve bytes it invokes:

```sh
cargo ai actions artifact --project ./project --interface reports --request-stdin --output-format json < artifact-request.json
```

Use `--package ALIAS` instead for installed data. Request shape is `{schema_version:1,interface,expected_binding,reference,read_grant}`; it includes exactly the retained grant and matching selectors, never a page-supplied path. Successful `cargo-ai.actions.artifact.v1` data contains `{schema_version:1,binding,interface,reference,mime_type,size_bytes,content_sha256,encoding:"base64",data}`. The explicit read requests private bytes without a result-content flag. It invokes no agent/tool/provider/network operation and initializes no home/data state.

Reads support only UTF-8 `text/plain`, `application/json`, `image/png` and `audio/wav`, at most **4,194,304 decoded bytes per file**. Requests are at most 65,536 bytes; references at most 128 bytes. JSON has bounded parsing; PNG/WAV have bounded format checks. Format validation does not establish decoder safety, codec playability or host rendering support. PDF, final video, longer narration, HTML/SVG and other types are unavailable. There is no streaming, range retry or implicit renderer. Additional formats/transport limits must be explicitly advertised; hosts report renderer support independently, leaving package business records unchanged.

The CLI confines reads through anchored handles, rejects links/reparse points, traversal, multiple hard links and nonregular files, and verifies exact size/digest and current data context. Changed/deleted/stale content fails; an old reference never follows new bytes. Hosts bound byte caches and revoke descriptor/grant access on navigation/reload/restart/binding or permission changes. Revocation cannot retract bytes already displayed. A deliberate authorized load may acquire current references; neither refresh nor `connect` replays a mutation.

Export sets are atomic: one failed nomination produces no descriptors/grants for that set, but already validated business data is retained with a failed/partial terminal and `artifact.export_failed`. Do not describe a committed save as rolled back. For optional thumbnails or attachments, return the useful records first and use separate declared preview actions. One missing/stale/unsupported/denied preview then leaves the records and other successful preview grants useful. The package describes unavailable previews honestly. Keep jointly required files in one export action. Permission denial is not permission to use filesystem, network or regeneration fallbacks.

## Host sessions and uncertainty

Hosts own submission tokens, serialized/busy policy and finite handle retention; the CLI advertises no idempotency. A same-token retry may recover only its exact action/inputs handle. Another intent while busy must not receive that unrelated handle as its result. Keep load/search/save controls visibly unavailable during a serialized run while preserving drafts, progress and Cancel. A new session must invalidate old tokens/authorization rather than replay them. Host negotiation must expose its actual limits and recovery facilities; these docs do not establish any particular host's implementation.

Missing terminal evidence leaves completion unknown. A native recovery path may explicitly authorize one declared inspection action with exact inputs/policy/binding; an action named load/read-only can still mutate or initialize state. Keep ordinary submissions locked, establish the previous process is quiescent, and retain the original unknown state while observing authoritative package state. A known recovery terminal does not prove the original run succeeded. Only separate deliberate native acknowledgment enables future submissions; a recovery's own unknown completion also needs reconciliation. A durable host-private uncertainty marker may preserve this guard across restart without storing business inputs/results/grants. Package-owned version/job state supplies authoritative evidence; no automatic mutation replay or upstream recovery database is introduced.

Runtime capability revision 4 advertises structured-result definition validation and execution checking. Generated executables report `terminal_delivery:false` and `artifact_read:false`; only the CLI reports those delivery capabilities. Rebuild generated artifacts to gain checking. Neither generated checking, CLI conformance tests nor these examples prove native-host adoption, media playback or live consumer generation.
