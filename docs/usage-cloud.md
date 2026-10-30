# Rolling local history and optional cloud analytics

Local recording, local expiry and cloud sending are separate choices. Usage
collection continues to work without a signed-in account. Cloud analytics accepts
a limited metadata projection; it cannot restore all original local details.
Check `cargo ai usage cloud status --remote --json` for service availability before
enabling it. These commands require a service advertising upload protocol 2.

## Select a local window

Existing history keeps its current policy until you explicitly choose rolling
expiry. Inspect the proposed coverage first:

```bash
cargo ai usage retention status --json
cargo ai usage retention preview --json
cargo ai usage retention configure --days 7 --enabled on \
  --acknowledge-local-detail-loss --acknowledge-unselected-loss --json
```

Supported windows are 1, 7 and 30 days; seven days is the default policy choice.
Expiry retains the selected number of completed UTC hourly buckets plus the
current partial hour. Active maintenance catches up elapsed hours after reopening.
It permanently removes expired local records and their retry copies, including
records that were never uploaded. A cloud acknowledgment does not delete local
records that remain inside the selected window. Turning the policy off stops
automatic expiry; it cannot recover already expired records.

Passive status and history reads do not initialize, migrate, expire or upload
history. Maintenance uses bounded transactions. If it needs another pass, metric
capture and upload pause with visible coverage while application execution
continues. Invalid or excessively future-dated event times are reported as
ineligible for cloud upload; they still have a finite local retention anchor.

## Enable and explicitly sync

Local tracking and an explicitly enabled rolling policy are prerequisites for
cloud consent. Enabling cloud analytics selects future records. Select existing
local history separately when wanted:

```bash
cargo ai usage cloud enable --json
cargo ai usage cloud include-history --json
cargo ai usage cloud include-history --through SEQUENCE \
  --preview-hash SHA256 --confirm --json
cargo ai usage cloud sync --json
```

Use the exact sequence and hash returned by the history-selection preview.
Selection alone does not upload. Only explicit `usage cloud sync` sends new cloud
analytics; execution and hourly maintenance do not send it automatically. A sync
pass sends bounded sequential compressed batches, newest eligible events first,
and reports partial progress. Each explicit sync first refreshes server policy and
the Home's registration within the same 15-second budget. Local status reads its
cached policy and reports pending events outside the cloud window separately.
Repeat sync to continue retained gaps or interrupted
work. Successful batches remain acknowledged across restart. Unknown attempts
retain the same event identities and compressed request while locally retained;
retries can produce transport duplicates that downstream processing deduplicates.

An upload acknowledgment means the service durably buffered the submitted batch.
Rows may still fail processing, and the dimensional model may lag. The response
does not certify that every event is query-ready. Expired-unsent and unknown
coverage remain explicit.

Basic cloud policy retains the most recent seven days and periodically trims the
oldest canonical events toward one million unique events per account, shared
across environments. This is a cleanup target with bounded overshoot. Upload rate
and processing-freshness safeguards may pause admission. A 30-day local window
does not extend the cloud window or cause expired cloud events to upload again.

Basic permits three upload-enabled Homes. CLI and generated agents sharing a
Home count once. To replace an active environment, use the opaque Home identifier
reported by remote status:

```bash
cargo ai usage cloud enable --replace-environment HOME_UUID --json
```

Replacement stops new uploads from the previous Home. Its existing cloud history
continues to expire normally; more than three historical environment identifiers
can therefore appear in retained history.

## Disable or clear

`usage cloud disable --json` stops new sends and preserves history. Cloud clear is
a separate preview-and-confirm operation:

```bash
cargo ai usage cloud clear --json
cargo ai usage cloud clear --preview-id PREVIEW_UUID \
  --preview-hash SHA256 --confirm --json
```

Use the exact preview values. If the confirmation response is lost, repeat that
confirmation to reconcile its outcome. `usage cloud status --json` exposes the
saved `pending_clear_confirmation` values after restart if the terminal result was
lost. Clear fences the old cloud generation and
leaves local history intact. Enable again and explicitly select history to replay
retained local records into the new generation. Active-store removal, pending
processing and provider recovery-copy expiry are separate; clear does not promise
immediate physical erasure of every recovery copy. Local `usage delete` instead
removes selected local records and their queued selections.

The separately consented [hosted backup](usage-backup.md) keeps its existing
commands and purpose. Its automatic hooks do not enable cloud analytics. See the
[usage ledger](../templates/guidance/usage-ledger.md) for local queries and coverage,
and the [machine payload contract](machine-payloads.md) for application callers.
