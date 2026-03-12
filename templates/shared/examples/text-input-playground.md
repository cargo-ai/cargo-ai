# `text-input-playground.json`

Use this as a simple text-input example you can copy and hatch locally.

```json
{
  "version": "2026-03-03.r1",
  "inputs": [
    {
      "type": "text",
      "text": "Summarize the meeting notes in one sentence."
    }
  ],
  "agent_schema": {
    "type": "object",
    "properties": {
      "summary": {
        "type": "string",
        "description": "One-sentence summary of the notes."
      }
    }
  },
  "actions": [
    {
      "name": "print_summary",
      "logic": {
        "!=": [
          { "var": "summary" },
          ""
        ]
      },
      "run": [
        {
          "kind": "exec",
          "program": "echo",
          "args": ["Summary created."]
        }
      ]
    }
  ]
}
```

Validate it:

```bash
cargo ai hatch text_input_playground --config ./templates/shared/examples/text-input-playground.json --check
```

Build it:

```bash
cargo ai hatch text_input_playground --config ./templates/shared/examples/text-input-playground.json
```
