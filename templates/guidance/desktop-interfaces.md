# Author interfaces for Cargo-AI Desktop

Use this guide when creating a package page that runs inside Cargo-AI Desktop. It describes the implemented package-author bridge, `window.cargoAICompanion`; keep that identifier exactly. You do not need Desktop's source repository. This is an optional host interface over Cargo AI's existing action contract, not another runtime or a general browser SDK.

Start with the complete [Preview note example](examples/desktop-interface/README.md). It declares an interface and runs a deterministic local tool with no model call. Read [client-actions.md](client-actions.md) for the underlying CLI contracts, native roles and structured results. The older `examples/client-action-controls.js` is an illustrative injected callback, **not** the Desktop API.

## Install and refresh this context

In the project, use `cargo ai add guidance --style codex` to install the canonical bundle and a thin `AGENTS.md` entrypoint. Existing user instructions are preserved; follow the printed merge snippet when necessary. The entrypoint leads to `cargo-ai.md`, this guide and its local example files. Claude uses the same bundle with `--style claude`.

For an existing managed bundle, run `cargo ai guidance status`, then explicitly choose `cargo ai guidance update`. Updating uses the guidance embedded in the **installed CLI**, offline. An older CLI cannot supply newer guidance. Obtain a CLI candidate containing this guide before updating; a matching version string alone does not identify a development candidate. Modified, unmanaged or incomplete bundles need the reported preservation/recovery handling, not forced deletion or fabricated manifests. Updating the guidance does not rewrite example copies, packages, saved data or user-owned instruction files.

## Ownership and version checks

| Package owns | Desktop supplies | Core enforces |
|---|---|---|
| Portable definitions/tools, business schemas, action descriptions, interface resources, role/call-site declarations | Verified resource serving and bridge transport | Catalog/source identity, native resolution and supported execution |
| Drafts, saved records, revisions/conflicts, history, durable idempotency, media adoption and business success | Native project access review, private role defaults and approved options, exact per-action policies | Actual connection/selection/policy checks, scope/limits and revocation |
| Explicit buttons, progress presentation, safe result rendering and error recovery | Correlated handles, bounded observation, cancellation and authorized artifact delivery | Structured result and artifact boundaries |

Do not reproduce native model setup, profile/credential storage, permission dialogs or authorization in the page. Business inputs never confer authority. Trusted package tools are executable code; declarations and resource confinement are not an OS sandbox.

These are distinct revisions:

- Action catalogs: **3** for native role declarations (including tool-only contexts); **2** remains supported for legacy fixed behavior.
- Native action requests: **3** or **4**, constructed privately by Desktop. Request 4 uses runtime 8 / selection policy 2 for per-generation choices. The page does not assemble these requests.
- JavaScript: `cargoAICompanion.actions.version === 2`, with additive feature strings from `actions.capabilities()`.

Inspect `cargo ai capabilities --output-format json` when authoring and validate the actual project. In the page, check for the bridge and probe capabilities **after native project access is approved**. Even `capabilities()` requires authorization. Do not infer support from a model name, catalog revision or app version. Missing bridge means the page is outside a supported Desktop document; explain how to open the project instead of emulating native execution.

## Declare actions, resources and roles

Place `cargo-ai-actions.json` at the explicit project root. Each action needs a stable `id`, helpful `label`, short effect `description`, `target`, closed `input_schema` and mappings to declared inputs/runtime variables. An action is a user-visible operation. Its label/description should disclose effects and supporting model work. Several actions may target one coordinator with different input schemas or constants. A business variable named `operation` is ordinary package dispatch data; it is unrelated to a role requirement's `operation`, such as `text_generation`.

Interfaces name their permitted actions and resources. The presentation entrypoint is the **resource ID**, not a pathname:

```json
{
  "interfaces": [{
    "id": "notes", "label": "Notes", "actions": ["preview"],
    "resources": ["page", "controls"],
    "presentation": {"entrypoint": "page"}
  }],
  "resources": [
    {"id": "page", "path": "ui/index.html", "mime_type": "text/html"},
    {"id": "controls", "path": "ui/controls.js", "mime_type": "text/javascript"}
  ]
}
```

This is a fragment; the complete example supplies the referenced action, agent and files. Declare every script, stylesheet, image and imported module used by the page. Use relative URLs and external scripts, for example `<script src="controls.js" defer></script>`. Desktop serves verified pinned bytes; the web view does not read project files directly. Inline CSS is permitted. Inline JavaScript, network/fetch, form submission, arbitrary frames, objects and alternate base URLs are blocked. Do not load a CDN, invent a loopback server or use arbitrary local paths. Ordinary interface pages do not gain execution permission by loading resources. Changed resources require rediscovery and affected native review.

For catalog 3, `role_registry.version:1` declares:

- `roles`: stable IDs, labels/purposes and operation, input modalities, structured-output and settings requirements, with optional nonpersonal recommendations.
- `call_sites`: concrete definition locators (`root` or `actions.0.run.0`, for example), their kind and role or fixed selection; include reachable children and tool-mediated calls.
- `contexts`: exact `{action, interface, mode}` keys, applicable call-site IDs, resource IDs, data-scope IDs and limits.
- `tool_content`: each participating tool's actual source-file inventory. Keep it complete as tools change; generated executables must support the selected native runtime.

For example, a structured text reviewer declares `requirements:{operation:"text_generation",input_modalities:["text"],structured_output:true,settings:{}}`. A `kind:"root"` site binds that role to an actual nonempty model schema. See the complete role declaration in [native role authoring](client-actions.md#native-role-authoring-and-host-flow). An image-review role includes the `image` input modality; image generation is `image_generation`, not text review. Do not declare a root inference site for an empty model schema.

Reuse one role ID across actions/interfaces when the same user choice serves all those uses. List each action's real dependencies in its own context; optional functionality needs an explicit declared mode/context. An unused role in one action does not make it optional for another. Tool-only contexts have `call_sites:[]` and need no invented model. Structural child sites without a role/fixed selection do not themselves imply inference; their descendants may. Desktop uses these dependencies to group operations and their models. Ambiguous multi-role work stays shared; no package priority field or special package-name convention is needed. Selecting a model does not select operations or grant their permissions.

Desktop discovers the package-wide registry, gathers private choices and resolves each action/interface/mode separately. A consolidated access review retains exact policies; it does not union permissions across actions. Native evidence of model/operation support is informational, distinct from verified account access and consent. Unknown/stale evidence does not justify a page-level permission bypass or an automatic provider/model substitution. An unsupported actual adapter operation still fails explicitly.

## Submit and observe one explicit intent

All methods below return Promises. Rejections are `Error` objects with a stable `.code`; use the code, not parsed message text. Treat handles, epochs, submission IDs and `documentID` as opaque.

| Call | Meaning |
|---|---|
| `actions.capabilities()` | Snapshot of features, epoch, busy/reconciliation state, per-action input budgets and bounds |
| `actions.newSubmissionID()` | Mint a token immediately before an explicit submission; minting does not run work |
| `actions.submit({action, inputs, submissionID, modelChoice?})` | Return a registered handle before validation/execution settles |
| `actions.status(handle)` | Observe state without running work |
| `actions.wait(handle, {timeoutMs:1000})` | Feature `action_wait.v1`; same snapshot after settlement or bounded timeout; one outstanding wait per document |
| `actions.cancel(handle)` | Request cancellation; returns `null`, not proof of rollback or completion |
| `actions.acknowledge(handle)` | Feature `terminal_acknowledgment.v1`; retire an already consumed, settled result |
| `actions.connect()` | Eligible retained handles for this interface/binding; no execution and no chronological ordering guarantee |
| `actions.artifact(reference)` | Read an exact registered artifact; no package tool/provider invocation |

Use only the closed submit fields above. Never send execution policies, credentials, profile references, binding revisions or filesystem grants from the page. `inputBytesByAction[actionID]` is the available business-input budget after native overhead; absent action/budget means it is unavailable in the current context. Both complete bridge and CLI requests are bounded at 65,536 UTF-8 bytes; JSON escaping still counts. Validate the small declared edit projection instead of sending a whole saved document indiscriminately.

Retain the frozen action/inputs/submission ID, epoch and returned handle while observing. `status`/`wait` returns:

```text
{handle, action, submissionID, epoch, state, canAcknowledge,
 progress, progressLimit, terminal, error}
```

Check all four correlation fields before using a response, then the selected business record/version and local edit revision. A late result must not overwrite newer edits. `validating` and `running` are active states; others include `succeeded`, `failed`, `partial`, `canceled`, `requires_interaction`, `not_started` and `unknown`. Ordinary progress is bounded `{event,sequence,operationID,data}` metadata; current event `data` is empty. There is no event-listener/subscription API. Use feature-gated wait or paced status polling; timeout is only another observation. The optional `native_live_progress.v1`/`liveProgress(handle)` route needs a separately supported progress declaration and schema; the feature alone is not an arbitrary file-reading channel. Ordinary progress needs no such extension.

The terminal projection is `{outcome,requestID,data,warnings,error}`. Check `data.client_action` against the selected action/interface. `data.result` may contain `availability`, `content_included`, `omission`, consented `content`, `artifact_references` and a delivery error. A succeeded process, registered handle or `availability:"available"` alone is not business success. Validate the package's result schema/status, inspect partial/conflict outcomes and keep warnings visible. Missing result consent must not cause a repeated save or generation merely to recover display bytes.

After consuming the result/error, wait until `canAcknowledge` is true, then acknowledge if supported. A terminal result can precede final process settlement. Acknowledgment is housekeeping, not business commit or undo; its failure must not turn committed work into a replay. Serial operations share one project execution context across views, with 128 ordinary handles. Do not queue around `host.busy` or exhaust handles by ignoring settled outcomes.

Cancellation can race with registration. Remember a cancel request made before `submit` returns, apply it to the returned handle, then keep observing. Earlier effects and media remain real. `unknown`, uncertain admission or lost observation requires Desktop Activity/reconciliation; do not mint a new token and replay. Exact-token retry is only for the same frozen intent in a valid context and returns its own existing registration. A changed choice/input is a new intent requiring a settled/reconciled context. Reconnect observes retained handles; it does not replay them or preserve live URLs across restart.

## Optional approved model choices

Desktop owns role defaults, optional private display names and approved alternatives. Offer an action-specific selector only when `approved_model_choices.v1` is present and `actions.modelChoices` exists:

```js
const choices = await window.cargoAICompanion.actions.modelChoices({action: "review"});
// {version:1, action, revision, roles:[{id,label,defaultOptionID,options:[
//   {id,label,model,settingsSummary,isDefault}
// ]}]}
```

Validate the action/version and projection; render all labels/model/settings as text. `defaultOptionID` may be null, and options may be empty. The projection contains no profile names, credentials, connections or global inventory. Use the exact role/option IDs and discovery revision only after an explicit selection:

```js
const modelChoice = {roleID: chosenRole.id, optionID: chosenOption.id, revision: choices.revision};
await actions.submit({action: "review", inputs, submissionID, modelChoice});
```

These snippets assume a declared `review` action and validated choices, inputs and token. Omission preserves the approved default behavior. Explicitly selecting the default ID pins the revision just like another option. One role changes for this invocation, including its declared descendants; other roles and the exact action policy remain enforced. The saved default is unchanged. Freeze `modelChoice` with the intent. `host.model_choice_stale` or `host.model_choice_unavailable` rejects before registration: keep the draft, rediscover on user action and require a new explicit choice. Never silently remove the choice or substitute another model. If the feature is absent, retain an existing default-only workflow only when that workflow was intended; do not invent a fallback for an explicit choice.

## Results, files and images

For structured results, declare a supported definition revision (at least `2026-10-03.r1`), `result:{source:"tool",schema:...,artifact_scopes:[...]}` and exactly one root tool step with `produces_result:true`. The tool returns a protocol result string containing `{data:<business value>,artifacts:[...]}`. Empty model properties permit a deterministic action-only agent. Desktop does not scrape stdout or promote child prose into a result.

Static UI resources and mutable data artifacts are different. Declare catalog `artifact_scopes` as `{id,path,mime_types}`, reference them in the interface and bound exports with the definition's `result.artifact_scopes`. Paths are subdirectories of the verified runtime data root. Desktop obtains separate native data consent and retains grants; the page only receives descriptors `{schema_version,id,reference,mime_type,size_bytes,content_sha256}`.

```js
const delivered = await actions.artifact(descriptor.reference);
if (delivered.mimeType === "text/plain" || delivered.mimeType === "application/json") {
  report.textContent = delivered.text;
} else if (delivered.mimeType === "image/png") {
  image.src = delivered.url;
} else if (delivered.mimeType === "audio/wav") {
  audio.controls = true;
  audio.src = delivered.url; // The user chooses Play.
}
```

Use declared elements and verified descriptors; never render returned text with `innerHTML`. URLs are opaque/document-scoped: do not construct, persist or treat them as paths. Current artifact rendering supports text, JSON, PNG and WAV, at most 4 MiB/file and 16,777,216 image pixels. One uncached read may be in flight; serialize reads. Finite registries/caches can evict old references. A cached delivery is verified captured bytes, not proof the file is still current; changed/deleted content can fail a fresh read. No generic PDF, HTML/SVG, video, JPEG or WebP **artifact renderer** is implied. Input formats below are a separate contract. A failed preview must not trigger regeneration or overwrite saved media.

For user-selected photos, use ordinary file-picker/drop `File` bytes and the existing `native_file_intake.v1` feature plus `cargoAICompanion.files.version === 1`. Check availability and the approved local adoption action/scope before reading/retaining a large file. The calls are:

```js
const transfer = await cargoAICompanion.files.begin({actionID, scopeID, sizeBytes, sha256});
const {nextOffset} = await cargoAICompanion.files.append({handle: transfer, offset, data});
const receipt = await cargoAICompanion.files.finish(transfer);
// receipt = {reference, sha256, sizeBytes, mimeType}
// After explicit successful business adoption and action acknowledgment:
await cargoAICompanion.files.release(transfer);
```

This is a sequence outline: compute the lowercase SHA-256 of original bytes before `begin`; send sequential base64 chunks of at most 32,768 **decoded** bytes and verify each returned offset until the complete size is acknowledged. Sources are nonempty and at most 20,971,520 bytes. PNG/JPEG/WebP and original HEIC/HEIF are accepted with bounded decoding: each side at most 10,000 pixels, 64 million pixels for PNG/JPEG/WebP or 24 million for HEIC/HEIF. The ordinary 64 KiB bridge-envelope bound remains. There is one active intake and at most 32 unretired receipts per document. HEIC/HEIF stays original; conversion/adoption belongs to the package and its declared dependencies. No fallback conversion is supplied by this guide.

The selected action must be approved for this interface, structurally ready, local with no model calls/required selections, and associated with the declared data/artifact scope. `begin` accepts a **scope ID**, never an arbitrary path. Intake stores bytes but makes no business edit. `finish` returns a durable receipt whose `sha256` hashes the **exact manifest bytes**, not the original photo. The original digest is in the manifest:

```json
{
  "schema_version": 1,
  "reference": "canonical-lowercase-uuid",
  "action_id": "adopt-photo",
  "interface_id": "photos",
  "file": {"name": "source", "sha256": "source-digest", "size_bytes": 123, "mime_type": "image/png"}
}
```

Within the verified runtime root, the declared scope contains `native-file-intakes/<reference>/manifest.json` and `source`. An explicitly submitted package adoption tool receives the receipt, expected business revision and bounded business choices. It must check the exact manifest hash, reference/action/interface, fixed source name, source size/hash and image constraints through confined no-follow access before atomic idempotent adoption. Recognize an already committed receipt before rejecting its original expected revision. A receipt is data, not permission; do not accept caller-chosen source paths. Retain prior saved data on failed/stale/canceled adoption.

Use `files.cancel(handle)` to abandon pending bytes; it can also retire completed retry state without deleting published content. An uncertain append is not blindly replayable: establish its acknowledged offset or cancel and start a new transfer. A same-handle `finish` retry returns the identical receipt until retirement. After confirmed adoption and acknowledgment, `release` retires host retry state without deleting adopted/private recovery files. Preserve an uncertain adoption for reconciliation; do not replay it or discard its recovery data. Revocation/document/context changes invalidate pending handles. Package tools retain their own decoder memory limits and dependency checks.

Clipboard image input is a separate optional `native_clipboard_image.v1` feature through `cargoAICompanion.clipboard.readImage()` (version 1). It requires native confirmation and supplies bounded normalized image data; it does not read arbitrary text/paths or submit an action. Do not implement a silent clipboard reader. Audio capture, exports, background jobs and isolated previews have additional declarations/contracts; the basic action bridge does not imply those workflows or permission to invent them.

## Saved data and unavailable states

Keep portable definitions, tools and UI assets in project/package content. Use the project's explicitly adopted runtime data root for mutable business records/media; `cargo ai new` adopts `.cargo-ai/data`. Existing projects require explicit adoption. Installed packages use their verified separate data root; avoid hardcoding an author-machine directory. Read [package-workflow.md](package-workflow.md) for existing packaging/data behavior. Private Desktop role mappings, named choices and remembered approvals live outside portable content and package-readable business data. Do not export them, put credentials in inputs, or persist handles/artifact URLs as saved content.

The package owns load/save operations and their semantics. Use string record IDs/revisions, expected-version conflict checks, bounded edit projections, atomic writes and durable operation identity where effects may be retried. Keep drafts until confirmed saved; distinguish a saved business revision from result delivery and acknowledgment. Resource reload is not a business-data freshness hook and can discard page edits. Reopening must load durable state through explicit declared operations, not assume a live handle or stale page cache is authoritative.

| Observation | Package response |
|---|---|
| Missing bridge/feature/action or unsupported contract | Explain the required Desktop/CLI capability; disable only the affected control, retain work. Do not synthesize a replacement API. |
| `host.authorization_required`, revoked/changed context | Direct the user to native project access/setup; keep saved choices and drafts. Never extend consent or ignore native rejection. |
| `host.busy`, session/size bound | Explain the current limit; finish/observe existing work or reduce the declared input. Do not queue or widen limits. |
| Unknown admission/completion, stale correlation | Preserve frozen intent and saved data; use Activity/reconciliation. No fresh-token automatic replay. |
| Missing tool/unsupported generated runtime/dependency | Explain the named missing build/dependency using its supported setup/rebuild instructions; do not hide it by changing providers or workflows. |
| Hidden/evicted/missing artifact or result | Keep the committed state and explain display availability; do not repeat mutation to get a preview. |
| Model choice stale/unavailable or provider rejects | Preserve the explicit choice and report the failure; rediscovery/new review is explicit. No silent default/substitution. |

Remembered approval has no time-based expiration. Native checks can pause work when connection continuity is uncertain or identity/settings/requirements/scope changes; only native verification decides whether review is needed. Routine unchanged credential renewal must not be turned into a package consent timer.

Validate the catalog and definitions with the existing CLI, then test the actual page JavaScript with deterministic host responses and real contract shapes. Include late/stale results, cancellation, missing consent, unavailable features, business conflict/error and cleanup failure. Use actual native UI testing for questions that those checks cannot establish, such as rendering/accessibility or native dialogs. Passing a contract test does not establish provider entitlement or live model quality.
