# Codex `functions.wait` route guard

This repository carries an additive, project-scoped Codex Hook that prevents a
`functions.wait` call from reaching the executor unless the current session and
turn contain a structurally correlated yielded-cell result. It does not edit,
replace, or weaken the user-level Rayman `Stop` Hook.

The implementation follows the current official OpenAI
[Hooks guide](https://learn.chatgpt.com/docs/hooks). `PreToolUse` can deny a
supported local function tool before execution, but specialized tool paths may
opt out of the default Hook path. Non-managed project Hooks are skipped until a
human reviews and trusts their exact definition through `/hooks`; changing the
definition changes its trust hash.

## Fixed scope and evidence

`.codex/hooks.json` invokes `scripts/codex-wait-route-guard.ps1` through a fixed
PowerShell command. Every handler carries the expected SHA-256 of the script;
the script refuses a different byte identity. The project Hook contains no
`Stop` entry, and the user-level `~/.codex/hooks.json` remains separately owned.

The guard reads only the first session-metadata record and a bounded 4 MiB tail
of the Hook-provided transcript. Enforcement requires all of the following:

- canonical `hook_event_name=PreToolUse` and `tool_name=wait`;
- exact, bounded `tool_input.cell_id` with no unknown input fields;
- transcript session identity equal to the Hook event;
- a current-turn `custom_tool_call` for `exec` correlated by `call_id` to a
  `custom_tool_call_output` beginning with the literal
  `Script running with cell ID <X>`;
- exact equality between `<X>` and the requested cell ID;
- evidence no older than ten minutes and no intervening unrelated tool call;
- after a previous wait, a matching `PostToolUse` receipt proving the same cell
  is still running. Completed, failed, unknown, stale, and `not found` results
  remain closed.

Prose, diagnostic output, guessed IDs, a different turn, malformed JSON,
duplicate JSON properties, an absent transcript, or a state mismatch never
create cell authority. A denial tells the model to call
`collaboration.list_agents({})` and
`collaboration.wait_agent({"timeout_ms":180000})` directly for child agents;
collaboration tools must never be nested inside `functions.exec`.

Runtime observations and continuation state live below
`.RaymanCodingSkill/tmp/codex-wait-route-guard/`. They are session-, turn-,
transcript-, tool-call-, and cell-bound support state, not release evidence and
not a source for restoring cells after compaction or resume.

## Context reinjection

The project configuration covers:

- `SessionStart` for `startup`, `resume`, `clear`, and `compact`;
- `SubagentStart` for every subagent type;
- `PostCompact` for `manual` and `auto` compaction;
- `PreToolUse` and `PostToolUse` for the candidate wait tool names.

The official Hook contract gives `SessionStart` and `SubagentStart` an
`additionalContext` channel, so those events inject the routing rule as
developer context. A root-session compaction also triggers `SessionStart` with
`source=compact` before the immediate continuation. `PostCompact` itself has no
documented `additionalContext` output in the current release contract, so this
Hook emits the supported `systemMessage` there and relies on the paired
`SessionStart(source=compact)` event for model context. This distinction must
not be reported as stronger enforcement than the host provides.

## Two-stage activation

The checked-in definition starts in `Observe` mode. Observe mode records the
canonical event and returns routing context but never denies a tool call.

1. A human places the reviewed candidate at `.codex/hooks.json`, opens `/hooks`,
   reviews and trusts the exact project Hook, then fully restarts Codex.
2. In the fresh session, the agent starts one bounded real `functions.exec`
   operation that yields a literal cell ID and immediately calls
   `functions.wait` with that exact ID.
3. Inspect the new observation file. Coverage is established only when the
   current-session receipt says `hook_event_name=PreToolUse`,
   `tool_name=wait`, and `mode=Observe`. An executor result alone does not prove
   Hook coverage.
4. If the receipt is absent or the canonical name differs, keep Observe mode,
   record a product-level blocker, and do not claim enforcement.
5. Only after coverage is proven, change every fixed handler to `-Mode Enforce`.
   Review the changed definition through `/hooks` and restart again.
6. In a new session, prove an invalid wait is denied before executor dispatch
   without `exec cell ... not found`, prove a real yielded cell remains usable,
   and prove direct collaboration calls are unaffected.

Never use `--dangerously-bypass-hook-trust`. Prompt text and static tests do not
substitute for the fresh-session coverage and end-to-end receipts.

## Deterministic checks

Run the script-level suite with:

```powershell
pwsh -NoProfile -File .\scripts\codex-wait-route-guard.ps1 -DeterministicTest
```

The suite covers a structured yielded cell, diagnostic-string forgery, wrong
and expired numeric IDs, intervening calls, completed reuse, failed output,
retry after `not found`, missing transcript, malformed input, duplicate JSON,
noncanonical tool names, child-agent correction, a still-running continuation,
context outputs, and the exact project-Hook/script hash binding.
