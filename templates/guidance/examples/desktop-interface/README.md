# Preview note in Cargo-AI Desktop

This complete local interface invokes a managed Rust tool through the real actions bridge. It returns `{status:"previewed", text}` without inference, network access or tool filesystem effects. The CLI still writes its ordinary operation metadata. The page keeps its unsaved draft only in memory; closing/reloading discards it. This is an authoring example, not a durable notes app.

## Create a fresh project

Use a current Cargo-AI CLI supporting catalog 3/native roles and a compatible Cargo-AI Desktop. Rust/Cargo must be available for the normal managed tool build. Choose a new destination. Set `example` to this example's **absolute** directory inside your installed `.cargo-ai/guidance/examples/desktop-interface/` or source checkout; preserve that path before changing directories.

```sh
example="/absolute/path/to/desktop-interface"
check_dir="$(mktemp -d)"
cargo ai new ./desktop-note --vcs none
cd desktop-note
cp "$example/cargo-ai-actions.json" "$example/preview-note.json" .
cp -R "$example/ui" ./ui
cargo ai add tool desktop_preview
cp "$example/tool.rs" tools/desktop_preview/src/tool.rs
cargo ai tools build desktop_preview
cargo ai actions list --project . --output-format json > "$check_dir/discovery.json"
```

`add tool` supplies the maintained protocol adapter, manifest and child bridge; replace only its author-owned `src/tool.rs`. The fixed `role_registry.tool_content.desktop_preview` inventory names all six source/lock files created by this scaffold. It contains paths, not copied hashes. Build creates `Cargo.lock` before discovery. If your scaffold differs, reconcile its actual source inventory and CLI compatibility rather than dropping identity checks.

For a read-only CLI contract check, create a temporary host-owned request **outside the project and package payload** from the outline below. Save it as `$check_dir/request.json` and set `check_request` below. Replace the whole placeholder `expected_binding` object with the exact `data.binding` object from `$check_dir/discovery.json`. The empty allowed selections and model bindings are appropriate only because this exact context has no model call sites. These are not permissions for other actions. Never package native requests or approval/binding records.

```json
{
  "schema_version": 3,
  "interface": "notes",
  "action": "preview-note",
  "inputs": {
    "text": "Hello Desktop"
  },
  "expected_binding": {
    "replace": "with binding from actions list"
  },
  "execution_policy": {
    "version": 1,
    "allowed": [],
    "limits": {
      "max_runtime_secs": 30,
      "max_output_tokens": 256,
      "max_agent_depth": 0
    }
  },
  "attachment_grants": {},
  "role_execution": {
    "mode": "default",
    "binding_revision": {
      "version": 1,
      "revision": "local-preview",
      "bindings": []
    }
  }
}
```

```sh
check_request="$check_dir/request.json"
```

```sh
cargo ai actions resolve --project . --interface notes --action preview-note --request-stdin --output-format json < "$check_request" > "$check_dir/resolution.json"
```

Confirm `data.ready` is true and `data.required_selections` is empty. Add `role_execution.resolution_id` to the temporary request, using the exact `data.identity` from `$check_dir/resolution.json`, then validate the same candidate:

```sh
cargo ai actions validate --project . --interface notes --action preview-note --request-stdin --output-format json < "$check_request"
```

Discovery, resolution and validation execute no action and grant no authority. The temporary request demonstrates the native request shape; the web page never constructs or sends private native bindings/policies. Cargo-AI Desktop owns fresh resolution, exact approval and execution. See [Desktop interfaces](../../desktop-interfaces.md) for the page contract and [client actions](../../client-actions.md) for native role authoring, and [package workflow](../../package-workflow.md) for ordinary packaging of an authored project.

## Open and use

Open the new `desktop-note` **folder** as a project in Cargo-AI Desktop. Choose **Preview note**, review native setup for the tool-only operation and result-content access, then choose **Connect** in the page. No provider/model selection is needed. Enter a note and choose **Preview note**. The returned text is inserted with `textContent`; HTML-looking input remains text.

The page checks capabilities, exact action budget and epoch, mints a token just before submitting, then observes `wait` when advertised or polls `status`. It correlates handle/action/submission/epoch, waits for settlement and explicitly acknowledges the consumed outcome. Local draft edits stay enabled; an older response cannot replace a preview after an edit. Cancel requests stopping and preserves the draft/earlier preview; the small deterministic tool may finish before cancellation arrives.

Unavailable bridge/features, missing action/result access and business/terminal failures remain visible. A failed acknowledgment or uncertain submission stays pending and blocks another preview; inspect Desktop Activity and reconcile uncertain effects before reopening. The page does not infer retry safety, automatically reconnect/replay, substitute models, clear uncertainty or authorize itself. Draft text, handles, result content and native approval are separate lifetimes.

## Files in this example

- `cargo-ai-actions.json`: catalog 3, explicit interface/resources and exact tool-only native context.
- `preview-note.json`: action-only definition with empty model schema and one selected result producer.
- `tool.rs`: deterministic managed-tool author code; replace the generated scaffold file with this.
- `ui/index.html`, `ui/style.css`, `ui/preview.js`: declared presentation bytes and bridge-2 consumer.

Changing a declared resource or tool source changes package identity and requires the normal Desktop review. No provider fixture, model fallback, filesystem browser, web backend or new runtime is required. File/image intake and named model options are optional separate authoring patterns; this example intentionally declares neither.
