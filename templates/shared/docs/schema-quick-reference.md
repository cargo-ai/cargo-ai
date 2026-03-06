# Cargo-AI Agent Schema Quick Reference

Use this as a concise reference while authoring agent configs.

## Required top-level fields
- `version`
  - format: `YYYY-MM-DD.rN` (example: `2026-03-03.r1`)
- `prompt`
- `agent_schema`
- `resource_urls`
- `actions`

## `agent_schema` expectations
- `agent_schema.type` must be `"object"`
- `agent_schema.properties` must be an object

Supported property `type` values:
- `string`
- `number`
- `integer`
- `boolean`
- `array` (single-level arrays with primitive item types)

## Current unsupported schema shapes
These should fail fast during `hatch --check`:
- nested objects
- union types
- nested arrays (`array` of `array`)

## `actions` expectations
- `actions` is an array
- each action includes `name`, `logic`, and `run`
- each run step currently supports `kind: "exec"` only
- `run[*].args` must be an array of literal strings and/or `{ "var": "field_name" }` objects
- `run[*].args[*].var` must reference a top-level field declared in `agent_schema.properties`
- array-typed schema fields are not supported for arg substitution in this story
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
