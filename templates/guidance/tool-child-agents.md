# Cargo AI Tool Child Agents

Use this file when a project-local tool needs to call one or more same-project child agents.

## When To Use The Helper

New scaffolded tools receive an `InvocationContext` argument plus a Cargo AI-owned helper in `src/agent_bridge.rs`.

Use that helper when the tool needs procedural control such as:
- splitting or iterating over input values
- calling one or more same-project child agents in sequence
- keeping iterator or fan-out logic in Rust instead of expanding the agent JSON or relying on the model to do deterministic string processing

## Current Helper Shape

Use it from `src/tool.rs`:

```rust
let request = ChildAgentRequest::new("./child_agent.json")
    .add_text_input("hello from the tool");
context.invoke_agent(request)?;
```

## First-Slice Rules

- same-project child-agent targets only; use same-level relative paths such as `./child_agent` or `./child_agent.json`
- tool execution itself does not consume an extra agent-depth hop
- a child-agent call from the tool consumes depth exactly as if the parent had called that child directly
- the helper carries the remaining runtime budget through to the child-agent invocation
- the helper carries automatic history correlation (root run id, parent agent run id and tool-launch metadata), plus an optional explicit usage-ledger path, into child-agent invocations
- manual direct `describe` or `invoke` calls outside a parent Cargo AI tool step will not include child-agent bridge context
- keep custom orchestration in `src/tool.rs`; do not rewrite `src/agent_bridge.rs`

## Authoring Guidance

- keep the tool responsible for deterministic orchestration and splitting
- keep the child agent responsible for model-driven work
- prefer the helper over hand-rolled subprocess flags, depth propagation, or runtime-budget forwarding
- keep custom business logging inside tool code when needed; Cargo AI's built-in usage log records only usage/timing metadata and never tool arguments, stdout/stderr, or child-agent payloads

## Artifact Location and Data Location

For opted-in project runs, tools write relative to `.cargo-ai/data/`, while sibling child JSON/executables stay beside the original run artifacts. The runtime supplies an optional `artifact_root` in `runtime_context.agent_bridge`. The bridge resolves the existing single-sibling `./child` contract against that base, checks the artifact is a regular file rather than a link/reparse point, and invokes it from that artifact directory so immutable relative inputs keep their meaning. Installed runs derive the base from the verified entrypoint. Missing artifacts fail; the helper does not search elsewhere or copy children into mutable data.

If `artifact_root` is absent, the bridge retains its previous cwd-based contract. New scaffolds include this support. Cargo AI upgrades do not rewrite existing user tool bridges. Review an older bridge before opting an existing project into runtime data; adopting the setting alone does not repair its child lookup. An explicit source update and rebuild may be needed, with user changes preserved. This field grants no additional permission and does not change depth, profile, runtime-budget, or usage forwarding.

Updated bridge context includes the root consuming workspace for usage attribution. Forward it through the supplied bridge with run correlation; do not recalculate it from a tool's runtime-data directory or use it as filesystem authorization. Child runtimes resolve their own package, environment and executable provenance. Existing tool bridges require an explicit source update/rebuild; user-owned files are not automatically overwritten. Read `usage-ledger.md` for the local attribution contract.

## Declared children in a native role session

Native role sessions use the scaffold's tool protocol 2 and `InvocationContext::invoke_declared`, rather than selecting a child artifact/profile inside the tool. Declare the exact `tool_child` call site in catalog 3's `role_registry`, with locator `tools.<tool-name>.<site>`, verified child JSON `target`, optional generated `artifact`, role or fixed selection, operation requirements and closed business `input_schema`. Include the complete exact tool source inventory in `role_registry.tool_content`. Rebuild older scaffolds/artifacts; missing native capability or generated source identity fails before child launch.

From the author-owned `src/tool.rs` invocation body:

```rust
let inputs = BTreeMap::from([("message".to_owned(), serde_json::json!("Review this draft"))]);
let child_result = context.invoke_declared("review-child", inputs)?;
Ok(Some(serde_json::to_string(&child_result)
    .map_err(|_| ToolError::new("Child business result could not be encoded."))?))
```

The declared call-site ID must match the native registry. The child's source definition declares the corresponding runtime variable `message`; business input keys map to same-name child runtime variables through existing `--run-var`, with arrays/objects JSON encoded into explicitly string variables. No model/profile/credential/selectors are injected into business data. The native parent validates input schema, package/source identity, current binding generation, exact scoped policy and invocation-wide admission, mediates launch and returns only business result/error. Procedural looping stays in the tool, but each declared child requires its own admission; an undeclared dynamic call or legacy `invoke_agent` call fails in a native session. Direct shell spawning is not a supported native role fallback.

Ordinary non-role tool calls retain the existing helper contract. Native role revocation/cancellation prevents new admissions and propagates owned-child cleanup; already admitted work may settle. Missing child terminal/control evidence is completion unknown, never an empty successful result. See `client-actions.md` for the host session, private binding storage and uncertainty rules.
