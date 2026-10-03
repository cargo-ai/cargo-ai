# Portable Client Actions

Use this topic when a project needs buttons, native controls or page-free calls that invoke declared Cargo AI agents. Check `cargo ai capabilities --output-format json` on the client's selected binary first. These interfaces require a build advertising the action commands and action-run selector; source documentation does not establish installed or released availability.

A normal browser has no Cargo AI execution bridge. If the host lacks the action callback, or its selected binary lacks the required schema/capabilities, keep execution controls disabled and explain that execution is unavailable. The document may still show read-only content. The illustrative adapter disables its buttons when `host.submit` is absent; a client exposes that callback only after negotiating the real CLI capabilities and grants. Do not fall back to arbitrary shell commands, legacy interface execution or guessed flags.

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

Discovery is passive: it does not build, run agents/tools, contact providers or initialize a Cargo AI home. Validation does not authorize execution. The client obtains explicit execution consent and supplies the authorized target, current binding and typed execution policy. Cargo AI validates again immediately before dispatch. A headless caller submits the same request without a document; there is no schedule flag or Cargo AI scheduling engine. The schema-v1 request contains `interface`, `action`, `inputs`, `expected_binding`, `execution_policy` and optional trusted `attachment_grants`. Validation returns binding/mapping descriptors without echoing business values.

## Inputs and identity

Declare bounded business inputs and map them explicitly to named agent inputs or scalar `runtime_vars`. Arrays and objects need explicit JSON encoding into a declared string destination. Unknown business fields, undeclared destinations, duplicate mappings and constant/input collisions fail. Keep execution selectors, profiles and limits outside business values. Inputs and runtime variables are ordinary data, not secret stores.

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

The browser sample expects buttons with `data-action="generate-one"`, `data-action="generate-selected"` or `data-action="review"`, a `data-panel-id` on the single-panel button, and a `[data-current-action]` element. The client supplies the current selection function and authorized host callback. Its host builds an argv array equivalent to the command above and writes the schema-v1 request to stdin. If it chooses a root profile/model/thinking override, use permitted existing `run` flags separately from business inputs; the policy must grant the resulting effective selection.


List `cargo-ai-actions.json` and optional presentation resources explicitly under the selected build profile's `assets`; list referenced agents under `agent_definitions` or `hatched_agents`. Build/package rejects references to excluded agents rather than copying them implicitly. A package without a sidecar remains an ordinary package. A malformed/unsupported present sidecar fails before installation replaces active state. Existing package permissions, `package/` ownership, disposable `runtime/`, preserved `data/` and supported leases remain in force. See `package-workflow.md`.

Companion and other clients should consume the neutral catalog and machine request directly. Their separate integration needs capability negotiation, explicit consent, scoped attachment/settings grants, verified presentation resources, process supervision, incremental event decoding, result opt-in and cancellation. A Companion adoption replaces its affected legacy `companion.json`/`getInputs()` interface contract; Cargo AI does not ship a legacy bridge adapter. Client authors decide overlap/queue behavior, schedule persistence/timing, saved versus fresh inputs, attachment retention and handle/reload recovery. Tests of Cargo AI routing establish the upstream contract; actual controls and scheduled execution still need consumer integration tests. No universal host, scheduler, storage schema or second lifecycle is required.

To install/update this offline topic, use `cargo ai add guidance --style codex` (or `claude`), then `cargo ai guidance status` and explicit `cargo ai guidance update`. Updates preserve user-owned entry instructions and refuse locally modified managed assets; keep custom project guidance outside the managed bundle.
