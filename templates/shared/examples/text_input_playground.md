# `text_input_playground.json`

Use this as a simple text-input example you can copy into an empty folder and hatch locally.

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

Save it as `text_input_playground.json`, then run:

```bash
cargo ai hatch text_input_playground.json
./text_input_playground
```

For Windows users, run `text_input_playground.exe` or just `text_input_playground`.

If you want the matching explanatory sidecar file next to the JSON, save this as `text_input_playground.md`:

```md
# `text_input_playground`

## Purpose
- Summarize meeting notes in one sentence.

## Inputs
- One text input with the notes to summarize.

## Output
- `summary`: one-sentence summary of the notes.

## Action Flow
- If `summary` is not empty, run `echo "Summary created."`

## Local Loop
- Hatch with `cargo ai hatch text_input_playground.json`
- Run `./text_input_playground`
```
