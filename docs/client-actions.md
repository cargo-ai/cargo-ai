# Portable client actions

A project can expose stable actions to a web interface, a native application or a headless caller. Cargo AI discovers and validates those actions and routes them through its existing `run` runtime. Project authors choose controls and agent behavior; clients own presentation, consent, process supervision and scheduling/storage policy.

Check `cargo ai capabilities --output-format json` on the selected binary before using action commands. The contract requires a build advertising them; source integration does not publish a release or upgrade an installed client. Normal browsers and older hosts without the execution bridge/required capability keep execution controls disabled, explain the limitation and may display read-only content. Do not guess a shell or legacy execution fallback.

The project-root `cargo-ai-actions.json` declares actions, explicit JSON entrypoints, bounded inputs/mappings, interfaces and optional presentation resources. One interface can invoke the same coordinator with different inputs and also expose actions bound to other agents. A native or headless client needs no HTML. See the self-contained [action-authoring guidance](../templates/guidance/client-actions.md) and [examples](../templates/guidance/examples/README.md).

```sh
cargo ai actions list --project ./project --output-format json
cargo ai actions validate --project ./project --interface panels --action generate-selected --request-stdin --output-format json < request.json
cargo ai run --project ./project --interface panels --action generate-selected --action-request-stdin --output-format ndjson < request.json
```

Use `--package ALIAS` instead of `--project PATH` for an installed package. Validation is advisory and reports no execution authorization. The host supplies explicit consent, the expected discovery binding and a typed execution policy; dispatch checks them again. The action's selected entrypoint and input mapping come from the catalog, never from a page-supplied executable or shell string. File inputs require scoped host grants.

Web presentation binds the declared HTML/script resource inventory as well as catalog/entrypoint identity. Clients serve verified bytes for the authorized document generation and reject undeclared imports or changed bytes. Reading a resource does not authorize execution. Source identities do not freeze transitive tools/children or arbitrary same-user edits. Installed package permissions and leases retain their existing meaning; action policy is declarative execution enforcement, not an OS sandbox.

Action calls reuse the [machine interface](machine-interface.md), [typed payloads](machine-payloads.md) and current cancellation/results. Correlation IDs are not idempotency keys. Acceptance, child exit status and missing terminal evidence do not become structured successful results. Rebuild generated children when their passive capabilities lack required input/settings/policy support.

Package the sidecar and web assets explicitly in the selected profile's `assets`, and include every referenced agent in `agent_definitions` or `hatched_agents`. See [packages](packages.md). Package metadata transports declarations/resources; it does not render interfaces or add a hosted scheduler.

Companion is a consumer of this contract. Its production controls, bridge, consent UX, schedule persistence, attachment retention and reload recovery need separate consumer work and tests. The [integration handoff](../templates/guidance/client-actions.md#packaging-and-client-integration) identifies the shared contract and client responsibilities, including replacement of its affected legacy page contract. Small Cargo AI examples demonstrate routing; they do not establish completed Companion adoption.

## Current contract and private results

The current interface requires catalog 2 and action request 2; catalog/request 1 and unknown revisions are unavailable, with no compatibility adapter. Outer machine envelopes/events, static resources and execution policy remain revision 1. Native hosts must implement and negotiate the replacement; no released host adoption is implied.

Business inputs and selected tool results share a bounded schema and UTF-8 key policy. Declared `settings`, `model`, `config` and similar names remain data inside `inputs`; native binding, policy and grants remain separate authority. Definitions using `2026-10-03.r1` can select exactly one concrete tool result, in either action-only or model-backed execution. Private content needs explicit opt-in. See the full [business data and selected result contract](../templates/guidance/client-actions.md#business-data-and-selected-tool-results), including nullable records, explicit mappings, request budgeting and failure semantics.

[Authorized runtime-data artifacts](../templates/guidance/client-actions.md#authorized-runtime-data-artifacts) provide bounded passive reads of text, JSON, PNG and WAV, up to 4 MiB per file. Catalog/interface scopes, definition declarations and native permission must agree. Hosts retain read grants privately and expose registered descriptors only; content identities are not bearer permissions. Source projects require runtime-data opt-in; installed aliases retain their separate data ownership and leases. Optional previews should use separate actions so a missing attachment cannot erase useful records or imply a committed save was rolled back.

Hosts implement [session and uncertainty handling](../templates/guidance/client-actions.md#host-sessions-and-uncertainty), including exact intent correlation, truthful busy/limit states and explicit native recovery without automatic replay. Upstream tests establish the CLI contract, not native playback or completed consumer adoption.
