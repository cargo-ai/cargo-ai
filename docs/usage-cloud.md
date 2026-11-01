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
cloud consent. Enabling cloud analytics selects future activity and does not send
it. Start or resume an explicit sync job:

```bash
cargo ai usage cloud enable --json
cargo ai usage cloud sync --json
cargo ai usage cloud sync --include-history 30d --json
```

`--include-history Nd` additionally selects available local history from the last
N days. Basic permits 1–30 days; larger windows return `plan_limit_denied` before
history selection or network access. Selection uses the job's start time and exact
committed record identities. New arrivals wait for the next explicit job. Changing
the requested window while a job remains active is rejected; plain `sync` resumes it.
After disabling and enabling again, an explicit sync can renew that same target
under the current consent. The passive runner cannot renew consent, and old
ambiguous requests remain recorded.

Each pass sends bounded compressed batches oldest first within a 15-second budget.
One sender lease per environment prevents overlapping sends. The running CLI's
existing `run` operation maintenance scope can resume an unfinished explicit job when available;
it does not create jobs or run a background daemon. Generated agents keep their
existing local-maintenance-only boundary and do not send cloud analytics. A paused quota job waits until
the recorded UTC reset before trying again. Successful acknowledgment progress and
unknown requests survive restart. Retries preserve exact event and batch identities.

Machine outcomes are `fully_synced`, `already_running`, `partial`, `quota_paused`,
`plan_limit_denied`, `failed` and `outcome_unknown`. The result and local status show
the latest job identity, outcome, completion flag, fixed boundary, total, pending,
uploaded, excluded and expired counts, including after completion. Server-expired
identities remain separately counted and are excluded from later jobs for that
account, including after a generation change. Independent local expiry can remove unsent records; those records are reported as
expired and never counted as synced. A durable upload acknowledgment preserves
local history and does not establish analytical query readiness.

The account-wide daily allowance is 1,000,000,000 decoded bytes and 1,000 upload
requests, shared across environments and reset at UTC midnight. Transmitted retries
consume allowance. Cloud payload retention is 30 days from first server arrival,
with up to one million newest unique events per account after daily cleanup.
Overshoot between cleanup runs is accepted. Explicit remote status reports
`fresh`, `overdue`, `missing` or `failed` analytical refresh; the daily freshness goal
is 24 hours. Uploads do not query analytical SQL.

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
leaves local history intact. Enable again and use `sync --include-history Nd` to replay
retained local records into the new generation. Active-store removal, pending
processing and provider recovery-copy expiry are separate; clear does not promise
immediate physical erasure of every recovery copy. Local `usage delete` instead
removes selected local records and their queued selections.

The separately consented [hosted backup](usage-backup.md) keeps its existing
commands and purpose. Its automatic hooks do not enable cloud analytics. See the
[usage ledger](../templates/guidance/usage-ledger.md) for local queries and coverage,
and the [machine payload contract](machine-payloads.md) for application callers.
