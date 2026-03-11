# Hatch Check Loop

Use this deterministic loop when authoring agent configs.

## 1) Start from a known-good example
- Copy one of:
  - `.cargo-ai/examples/agent-minimal.json`
  - `.cargo-ai/examples/agent-enum-bounds-valid.json`

## 2) Rename and edit
- Save your draft as `<name>.json`
- Keep field names stable between `agent_schema.properties` and `actions[*].logic` var references

## 3) Validate only (no binary export)
- `cargo ai hatch <name> --config <name>.json --check`

## 4) Fix errors until validation passes
- Build-time errors point to a schema/action path
- Repeat step 3 after each edit

## 5) Build/export once valid
- `cargo ai hatch <name> --config <name>.json`

## 6) Overwrite exported binary only when intentional
- `cargo ai hatch <name> --config <name>.json --force`

## Intentional failure example
- `.cargo-ai/examples/invalid/agent-logic-invalid-var.json` exists to demonstrate fail-fast validation behavior.
- Do not use invalid examples as a production starting point.
