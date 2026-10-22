/* A package owns its draft and business outcomes; the host owns permission. */
(() => {
  class DesktopPreview {
    constructor(host, changed = () => {}, pause = ms => new Promise(resolve => setTimeout(resolve, ms))) {
      this.host = host; this.changed = changed; this.pause = pause;
      this.draft = ''; this.editRevision = 0; this.previewText = '';
      this.busy = false; this.pending = null; this.caps = null;
      this.message = 'Connect after reviewing this interface’s access in Cargo-AI Desktop.';
    }
    edit(text) { this.draft = text; this.editRevision++; this.changed(this); }
    async connect() {
      if (this.pending) return this.report('Review the pending operation in Desktop Activity; do not repeat it. Reconcile uncertain effects before reopening this interface.');
      try {
        if (this.host?.version !== 2) throw Error('Open this project in Cargo-AI Desktop with actions bridge 2.');
        const caps = await this.host.capabilities();
        this.requireSupport(caps);
        if (caps.busy || caps.reconciliationRequired) throw Error('Review active or uncertain work in Desktop Activity before connecting.');
        if (!(caps.inputBytesByAction?.['preview-note'] > 0)) throw Error('Allow the Preview note operation in Desktop setup, then connect again.');
        const handles = await this.host.connect();
        if (handles.length) throw Error('Review retained operations in Desktop Activity before opening a fresh example session. Nothing was replayed.');
        this.caps = caps; this.report('Ready. Preview computes a result without saving your note.');
      } catch (error) { this.caps = null; this.report(this.explain(error)); }
    }
    requireSupport(caps) {
      if (!['business_inputs.v1', 'structured_results.v1', 'terminal_acknowledgment.v1'].every(feature => caps.features?.includes(feature)) || typeof this.host.acknowledge !== 'function') {
        throw Error('This Desktop host lacks the required input, result or acknowledgment support.');
      }
    }
    report(message) { this.message = message; this.changed(this); }
    explain(error) { return `${error.code ? error.code + ': ' : ''}${error.message}`; }
    async preview() {
      if (this.busy || this.pending || !this.caps) return;
      this.busy = true; this.report('Preparing preview…');
      try {
        const caps = await this.host.capabilities();
        this.requireSupport(caps);
        if (caps.epoch !== this.caps.epoch || caps.reconciliationRequired || caps.busy) throw Error('The session changed or has active work. Review Desktop Activity and reconnect.');
        if (caps.handles >= caps.handleLimit) throw Error('The host’s retained-operation limit is reached. Review Desktop bookkeeping.');
        if (!(caps.inputBytesByAction?.['preview-note'] > 0)) throw Error('Preview note is no longer approved. Review Desktop setup and reconnect.');
        const inputs = {text: this.draft}, editRevision = this.editRevision;
        if (!inputs.text.length || new TextEncoder().encode(inputs.text).length > 4096) throw Error('Enter a note of 1–4096 UTF-8 bytes. Your draft is retained.');
        if (new TextEncoder().encode(JSON.stringify(inputs)).length > caps.inputBytesByAction['preview-note']) throw Error('This draft exceeds the current action input budget. Shorten it and try deliberately.');
        const submissionID = await this.host.newSubmissionID();
        const intent = {request: {action: 'preview-note', inputs, submissionID}, epoch: caps.epoch,
          editRevision, handle: null, cancelRequested: false, settled: false};
        this.pending = intent;
        this.changed(this);
        intent.handle = await this.host.submit(intent.request);
        if (intent.cancelRequested) await this.host.cancel(intent.handle);
        await this.observe(intent, caps);
      } catch (error) {
        const rejected = ['host.authorization_required', 'host.stale_document', 'host.invalid_submission',
          'host.busy', 'host.session_limit', 'host.request_limit', 'host.permission_denied', 'host.unsupported'];
        if (this.pending && !this.pending.handle && rejected.includes(error.code)) this.pending = null;
        if (this.pending) this.report(this.explain(error) + ' Review this operation in Desktop Activity; do not repeat it. Your draft is retained.');
        else this.report(this.explain(error));
      } finally { this.busy = false; this.changed(this); }
    }
    async observe(intent, caps) {
      for (;;) {
        const waiting = caps.features?.includes('action_wait.v1') && typeof this.host.wait === 'function';
        const snapshot = await (waiting ? this.host.wait(intent.handle, {timeoutMs: 1000}) : this.host.status(intent.handle));
        for (const [key, expected] of Object.entries({handle: intent.handle, action: intent.request.action,
          submissionID: intent.request.submissionID, epoch: intent.epoch})) {
          if (snapshot[key] !== expected) throw Error('The response does not match the submitted operation.');
        }
        if (snapshot.state === 'unknown' || (!['validating', 'running'].includes(snapshot.state) && !snapshot.terminal)) {
          throw Error('Completion is uncertain. Inspect and reconcile the operation in Desktop.');
        }
        if (['validating', 'running'].includes(snapshot.state) || !snapshot.canAcknowledge) {
          this.report(intent.cancelRequested ? 'Stopping; earlier effects may remain…' : 'Waiting for the operation to settle…');
          if (!waiting) await this.pause(200);
          continue;
        }
        const association = snapshot.terminal.data?.client_action;
        if ((snapshot.state === 'succeeded' && !association) || (association && (association.interface !== 'notes' || association.action !== intent.request.action))) {
          throw Error('The business result belongs to another interface or action.');
        }
        intent.settled = true;
        const result = snapshot.terminal.data?.result;
        const value = result?.content;
        let message;
        if (snapshot.state !== 'succeeded' || snapshot.terminal.outcome !== 'succeeded') {
          message = snapshot.state === 'canceled' ? 'Stopped. Your draft and earlier preview are retained.' : 'The operation did not succeed. Inspect its error in Desktop Activity.';
        } else if (result?.availability !== 'available' || result.content_included !== true || !value) {
          message = 'Result content is unavailable. Review Desktop result access; do not repeat work just to display it.';
        } else if (value.status !== 'previewed' || typeof value.text !== 'string' || new TextEncoder().encode(value.text).length > 4096) {
          message = 'The business result is unsupported. Your draft and earlier preview are retained.';
        } else if (this.editRevision !== intent.editRevision) {
          message = 'Your draft changed while the preview ran. The older result was not applied.';
        } else { this.previewText = value.text; message = 'Preview ready. Nothing was saved.'; }
        if (snapshot.terminal.warnings?.length) message += ' The operation reported warnings; inspect Desktop Activity.';
        this.report(message);
        try { await this.host.acknowledge(intent.handle); }
        catch (error) { throw Error('The result was consumed, but cleanup was not confirmed. ' + this.explain(error)); }
        this.pending = null;
        return;
      }
    }
    async cancel() {
      const intent = this.pending;
      if (!intent || intent.settled) return;
      intent.cancelRequested = true;
      if (!intent.handle && !this.busy) return this.report('The submission was not confirmed, so its cancellation cannot be addressed here. Review Desktop Activity and reconcile uncertain effects; do not repeat it.');
      this.report('Requesting stop; completed effects are retained…');
      try { if (intent.handle) await this.host.cancel(intent.handle); }
      catch (error) { this.report(this.explain(error) + ' Review cancellation in Desktop Activity.'); }
    }
  }
  globalThis.DesktopPreview = DesktopPreview;
  if (typeof document !== 'undefined') {
    const draft = document.querySelector('#draft'), result = document.querySelector('#result');
    const connect = document.querySelector('#connect'), preview = document.querySelector('#preview');
    const cancel = document.querySelector('#cancel'), status = document.querySelector('#status');
    const client = new DesktopPreview(globalThis.cargoAICompanion?.actions, state => {
      status.textContent = state.message; result.textContent = state.previewText;
      preview.disabled = !state.caps || state.busy || Boolean(state.pending);
      connect.disabled = state.busy; cancel.disabled = !state.pending || state.pending.settled;
    });
    draft.addEventListener('input', () => client.edit(draft.value));
    connect.addEventListener('click', () => client.connect());
    preview.addEventListener('click', () => client.preview());
    cancel.addEventListener('click', () => client.cancel());
  }
})();
