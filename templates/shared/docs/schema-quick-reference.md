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
 - `array`
 - `object`

Optional top-level property metadata and constraints:
- `description` on any supported field
- `enum` on `string` fields only
- `minimum`, `maximum`, `exclusiveMinimum`, and `exclusiveMaximum` on `number` and `integer` fields
- `enum` values are exact and case-sensitive
- lower bounds may use `minimum` or `exclusiveMinimum`, but not both
- upper bounds may use `maximum` or `exclusiveMaximum`, but not both
- `array` fields must be homogeneous
- `object` fields must declare their shape explicitly
- arrays may contain supported scalar item types or declared-shape object items
- object properties inside structured tool-bound fields may be scalar or `scalar | null`

## Current unsupported schema shapes
These should fail fast during `hatch --check`:
- nested arrays
- deeper nested objects
- union types other than `scalar | null` on object properties inside structured tool-bound fields

## `actions` expectations
- `actions` is an array
- each action includes `name`, `logic`, and `run`
- run steps may use `kind: "exec"`, `kind: "agent"`, `kind: "tool"`, `kind: "email_me"`, or `kind: "generate_image"`
- `run[*].args` must be an array of literal strings and/or `{ "var": "field_name" }` objects
- `run[*].args[*].var` must reference a top-level field declared in `agent_schema.properties`
- `generate_image.reference_images` is optional and may contain `{ "input": "<named image input>" }` or `{ "path": "./assets/reference.png" }`
- `generate_image.reference_images[*].path` must stay at the current level or below; absolute paths and `..` traversal are rejected
- structured top-level fields may flow only into tool params
- scalar-first surfaces such as `logic`, `when`, `run[*].args`, string-part interpolation, and child `run_vars` reject structured field references
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
