# Cargo AI Pattern Selection

Use this file after the user describes the outcome they want. The user should not need to know Cargo AI architecture terms up front.

## Pattern Guide

### Start with `schema-features.json` when:

- the user needs more than one scalar output field type in the same agent
- the user needs `description`, string `enum`, or numeric bounds
- the main uncertainty is the output schema shape rather than action control flow

### Start with `basic-agent.json` when:

- the user wants one agent
- the model output is the main deliverable
- action logic is light or straightforward

### Start with `conditional-when.json` when:

- later steps should run only when a prior condition is true
- one agent is enough, but the action flow branches
- the user wants conditional follow-up behavior inside one agent

### Start with `stop-by-default.json` when:

- failure should stop the action unless explicitly overridden
- a later step depends on an earlier step succeeding
- you want the clearest default control flow

### Start with `continue-on-failure.json` when:

- a step may fail and later steps should still react
- the user wants notify-on-failure, cleanup, or fallback behavior
- you need `failure_mode`, `status_variable`, or `error_variable`

### Start with `child-agent.json` when:

- a parent needs to hand work to another agent
- the child is logically reusable or easier to reason about separately
- the user describes two different roles or stages of work

## Portability Guide

Ask near the end of discovery:
- should this run only on your current machine?
- or should it stay portable across macOS, Windows, and Linux?

Choose the smallest shape that meets that answer:
- portable target
  - prefer minimal actions
  - avoid shell-specific scripts when possible
  - keep file paths relative
- local-machine target
  - local commands are acceptable
  - still keep the JSON explicit and small

## Selection Rule

Infer the pattern yourself and explain it in plain language. Do not push the architecture decision back onto the user unless they explicitly want that level of control.
