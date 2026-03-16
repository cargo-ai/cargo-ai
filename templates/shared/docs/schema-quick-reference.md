# Cargo-AI Agent Schema Quick Reference

Use this as a concise reference while authoring agent configs.

## Required top-level fields
- `version`
  - format: `YYYY-MM-DD.rN` (example: `2026-03-03.r1`)
- `inputs`
- `agent_schema`
- `actions`

## `inputs` expectations
- `inputs` is an ordered array with at least one entry
- supported input `type` values:
  - `text`
  - `url`
  - `image`
- `text` entries require `text`
- `url` entries require `url`
- `image` entries require `path`
- runtime overrides use:
  - `--input-mode` with `replace`, `append`, or `prepend`
  - `--input-text`
  - `--input-url`
  - `--input-image`
- if runtime input flags are provided without `--input-mode`, they replace config-defined `inputs`
- `--input-mode append` keeps baked inputs first; `--input-mode prepend` keeps runtime inputs first

## `agent_schema` expectations
- `agent_schema.type` must be `"object"`
- `agent_schema.properties` must be an object

Supported property `type` values:
- `string`
- `number`
- `integer`
- `boolean`

Optional top-level property metadata and constraints:
- `description` on any supported field
- `enum` on `string` fields only
- `minimum`, `maximum`, `exclusiveMinimum`, and `exclusiveMaximum` on `number` and `integer` fields
- `enum` values are exact and case-sensitive
- lower bounds may use `minimum` or `exclusiveMinimum`, but not both
- upper bounds may use `maximum` or `exclusiveMaximum`, but not both

## Current unsupported schema shapes
These should fail fast during `hatch --check`:
- top-level arrays
- nested objects
- union types

## `actions` expectations
- `actions` is an array
- each action includes `name`, `logic`, and `run`
- each run step currently supports `kind: "exec"` only
- `run[*].args` must be an array of literal strings and/or `{ "var": "field_name" }` objects
- `run[*].args[*].var` must reference a top-level field declared in `agent_schema.properties`
- `run[*].platform` is optional
- `run[*].platform` may be a single string or an array of strings
- supported platform values are `macos`, `linux`, and `windows`
- platform values are normalized case-insensitively and should be authored in lowercase in configs/docs
- omitted `platform` means the step runs on every runtime OS

## Logic validation expectations
- every `{ "var": "..." }` must match a key in `agent_schema.properties`
- comparison operators (`==`, `!=`, `>`, `>=`, `<`, `<=`) are validated for operand compatibility

## Canonical validation/build commands
- Validate only:
  - `cargo ai hatch <agent-name> --config <config.json> --check`
- Build and export:
  - `cargo ai hatch <agent-name> --config <config.json>`
- Overwrite existing exported binary if needed:
  - `cargo ai hatch <agent-name> --config <config.json> --force`
