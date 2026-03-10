# Cargo AI Troubleshooting

Use this file when `cargo ai hatch <agent-name> --config <config.json> --check` reports an error.

## Common Problems

### Missing required top-level keys

Check that the JSON includes:
- `version`
- `inputs`
- `agent_schema`
- `actions`

### Wrong field for a step kind

Examples:
- `output_variable` on `agent` or `email_me`
- missing `program` for `exec`
- missing `agent` for `kind: "agent"`

### Bad variable references

Check for:
- collisions with top-level schema fields
- reusing the same captured variable name twice in one action
- using a captured variable before the step that creates it

### Path problems

Check for:
- absolute paths
- `../`
- child agents that are not written as `./child_name`

### Portability drift

If the user asked for portability across macOS, Windows, and Linux:
- remove shell-specific assumptions when possible
- minimize `exec` usage
- keep commands and paths as generic as possible

## Default Fix Loop

1. Read the reported field path.
2. Fix one problem at a time.
3. Re-run:
   - `cargo ai hatch <agent-name> --config <config.json> --check`
4. Build only after the check passes.

## When To Add Sidecar Notes

If the JSON is technically valid but hard to explain:
- add a same-name sidecar Markdown file
- add an ASCII diagram near the top
- record any platform-specific assumptions there
