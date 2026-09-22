# Optional usage backup and restore

Automatic usage history is local by default. Cloud backup is off until you enable
it explicitly for the signed-in Cargo AI account. Collection, upload, local deletion
and cloud deletion are separate controls. This feature backs up logical metadata
records; it does not continuously merge device histories or copy a live SQLite file.

## Enable future backup

Use the existing account sign-in and credential store, then run:

```sh
cargo ai usage backup status --json
cargo ai usage backup enable --json
```

The first command reads local state without network access or credential lookup.
Add `--remote` to explicitly inspect the signed-in account. Enabling returns an
opaque `account_binding`, a consent `generation`, and a local sequence boundary.
Only later records become eligible automatically. Repeating enable for the same
active account preserves the original boundary and its offline backlog.

The CLI and newly generated standalone agents attempt one upload pass when their
root run finishes, including failure paths that return through normal cleanup.
They need no background daemon or CLI subprocess. A pass has a two-second network
budget and sends at most one batch of 100 records, bounded below 256 KiB. SQLite
operations use bounded lock waits. Pending selections stay in the local database;
failed automatic passes back off from 30 seconds to one hour. A later run resumes
them. Process termination can prevent the final pass; it does not remove committed
history. Already-built older generated agents need rebuilding for these features.

```sh
cargo ai usage backup sync --json
cargo ai usage backup disable --json
```

Explicit sync bypasses automatic backoff, sends at most ten batches in 30 seconds,
and stops at the first failure. Disabling upload preserves local facts and previously
selected records. Re-enabling starts a new future interval; records collected during
the pause require explicit historical selection. Already transmitted requests may
finish. Turning collection off with `usage settings --tracking off` does not erase
history or revoke a separately enabled upload selection.

## Select older history

Preview before selecting older records:

```sh
cargo ai usage backup include-history --json
```

The preview returns the exact account binding, generation, `through` snapshot and
number of additional records. Substitute those returned values below:

```sh
cargo ai usage backup include-history --through SNAPSHOT --account-binding ACCOUNT_BINDING --generation GENERATION --confirm --json
cargo ai usage backup sync --json
```

Confirmation is tied to the preview's account and generation. It never silently
retargets old selections after a different sign-in. Each queue remains bound to
its original account and generation; status shows separate queues and unscanned
eligible records. Historical selection uses short transactions and a 30-second
budget. If it stops partway, completed selections remain; repeating the same
confirmed snapshot selects only the remaining records.

## Restore a snapshot

```sh
cargo ai usage backup restore --json
cargo ai usage backup restore --account-binding ACCOUNT_BINDING --generation GENERATION --confirm --json
```

Use the preview's binding/generation. Each confirmed command imports one page of
up to 100 records. If `next_cursor` is non-null, repeat the confirmed command with
`--cursor CURSOR` until `snapshot_complete` is true. The cursor fixes the snapshot
and its account/generation; later uploads do not shift the page boundaries.

Restoring keeps original event IDs, so identical repeats do not double-count usage.
A conflicting event rejects the page and preserves original local facts. Existing
local labels remain intact; newly restored project, agent, profile, tool and action
labels may be opaque IDs. Restored records are excluded from automatic upload and
historical selection. Back up new execution facts from each home explicitly rather
than using restore as a continuous synchronization loop.

## Delete cloud history separately

```sh
cargo ai usage backup delete --json
cargo ai usage backup delete --account-binding ACCOUNT_BINDING --generation GENERATION --confirm --json
```

Use the binding/generation returned by the preview. Deletion removes that account's
cloud usage, disables its upload generation and leaves local history intact. Old
offline queues cannot repopulate the deleted generation. To upload old local facts
again, explicitly enable a fresh generation and preview/confirm historical selection.
Local `usage delete` instead removes local records and their queued selections;
it does not delete existing cloud copies. Standard service recovery-copy retention
is separate from deleting active cloud history.

## Metadata and failure contract

Backup includes stable opaque identities, supported provider/model identifiers,
available token facts and provenance, timestamps/durations, bounded outcomes and
operational counts. It excludes prompts, responses, tool inputs/outputs, arbitrary
provider metadata, local paths, user-authored labels, credentials and raw errors.
Unsafe custom identifiers are omitted from the cloud projection; local facts remain.
Token details can overlap headline counters and are not additional billable totals.
No dollar-cost calculation or pricing catalog is included.

All backup command results are JSON with `schema_version: 1`; success has a `backup`
object, and failure has `error.kind` and a redacted `error.message`, with nonzero
exit status. Inspect command success and coverage, not just the presence of a queue.
Unknown usage remains unknown. A completed backup contains only captured records:
it cannot recover facts lost before local persistence or certify unobserved calls.
Cloud failure never changes an agent's result or deletes its local history.

Account credentials stay in the established credential backend. There is no
automatic sign-in; an existing refreshable session may be renewed using the existing
account status flow; backup keeps the refreshed token in memory and leaves stored credentials unchanged. Switching accounts, expired authorization or a revoked generation
stops the affected upload. HTTP redirects are refused, request/response sizes are
bounded, and error output omits service bodies and credentials.

Backup consent changes are serialized with uploads and other backup operations. If a command reports that another backup operation is active, it has not completed the requested change; retry after that operation finishes. Tracking changes affect collection only and preserve backup consent.
