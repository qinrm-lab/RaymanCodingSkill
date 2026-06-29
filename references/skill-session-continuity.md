# Extracted Skill Rules

Source: `SKILL.md`

## Session Continuity

- At the start of an opted-in workspace session, run `rayman session status`. If `.RaymanCodingSkill/pending_work.json` has active items, finish those first unless the user explicitly reprioritizes.
- Treat active or recoverable `.RaymanCodingSkill/goals/*.json` records as resumable customer goals. Continue the next executable stage until the goal is success/blocked/failed; do not stop with advice only while concrete work remains. Use `rayman goal run --until blocked` for long-running progress, `rayman goal resume --id <id>` for blocked/partial goals after the blocker is resolved, and `rayman session recover` to find the next exact resume command. Any waiting handoff must preserve the minimum input, `.RaymanCodingSkill/goals/<id>.json` evidence path, resume command, and automatic resume strategy from pending-work metadata.
- At conversation end, every requested function, task status, and changed code path must be complete. If anything is partial, failed, skipped, blocked, in progress, or intentionally deferred, record it with `rayman session close --status partial -m "unfinished summary"` or `rayman session add-pending "title" -m "details"`.
- Code review cannot pass when there is unfinished work. Active pending work and clear markers such as TODO, FIXME, TBD, 未完成, or 待完成 are blocking review findings until completed or tracked as pending.
