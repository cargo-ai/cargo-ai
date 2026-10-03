// Illustrative web adapter: the client implements and authorizes host.submit.
// Cargo AI supplies the CLI contract; this callback is not a shipped browser API.
export function bindPanelControls(root, host, selectedPanelIds) {
  let pending = false;
  let reconciliationRequired = false;
  const buttons = [...root.querySelectorAll("button[data-action]")];
  const status = root.querySelector("[data-current-action]");
  if (typeof host?.submit !== "function") {
    buttons.forEach(control => { control.disabled = true; });
    status.textContent = "Execution unavailable; open in a capable client.";
    return () => {};
  }

  async function submit(button) {
    if (pending || reconciliationRequired) return;
    const action = button.dataset.action;
    if (!["generate-one", "generate-selected", "review"].includes(action)) return;
    const panelIds = action === "generate-one"
      ? [button.dataset.panelId]
      : [...selectedPanelIds()];
    if (panelIds.length === 0 || panelIds.some(id => typeof id !== "string" || !id)) return;

    pending = true;
    buttons.forEach(control => { control.disabled = true; });
    status.textContent = "Running…";
    try {
      // The host supplies target, current expected_binding and execution_policy,
      // serializes the request, and invokes the real `cargo ai run` action selector.
      // It resolves only after reading the existing operation_completed envelope.
      const terminal = await host.submit({
        interface: "panels",
        action,
        inputs: { panel_ids: panelIds }
      });
      status.textContent = terminal.outcome === "succeeded"
        ? "Completed"
        : `Finished: ${terminal.outcome}`;
    } catch {
      // Loss of terminal evidence cannot establish whether effects occurred.
      reconciliationRequired = true;
      status.textContent = "Completion unverified; use native reconciliation before resuming.";
    } finally {
      pending = false;
      buttons.forEach(control => { control.disabled = reconciliationRequired; });
    }
  }

  const handlers = buttons.map(button => {
    const handler = () => { void submit(button); };
    button.addEventListener("click", handler);
    return [button, handler];
  });
  // Detaching the document is separate from cancelling its active invocation.
  return () => handlers.forEach(([button, handler]) => {
    button.removeEventListener("click", handler);
  });
}
