# Extracted Skill Rules

Source: `SKILL.md`

## Agent Skill Installation

RaymanCodingSkill must be discoverable by both Codex and Claude Code after any skill install or update.

- For install/update requests for this skill, run `rayman agent-skill sync` when the CLI is available. The shorthand `rayman skill install` and `rayman skill update` are equivalent.
- `rayman agent-skill sync` refreshes the installed `rayman` CLI binary, the companion `提醒` background reminder binary, and the host skill entries; this is required so new commands such as `rayman auxiliary` and automatic goal/session stop reminders cannot be hidden by older installed binaries.
- Sync both personal agent entries by default:
  - Codex: `~/.codex/skills/raymancodingskill/SKILL.md` (`CODEX_HOME` may override the root).
  - Claude Code: `~/.claude/skills/raymancodingskill/SKILL.md` (`CLAUDE_HOME` may override the root).
- Keep both installed entries thin: they should delegate to this canonical `SKILL.md` and the `rayman` CLI, not duplicate workspace state or long-lived project assumptions.
- Verify with `rayman agent-skill status` after syncing. `rayman agent-skill status` must report Codex, Claude Code, `rayman-cli`, and `rayman-reminder`; if any entry is missing, stale, or failed, report the operation as partial/blocked and record pending work before ending the session.
- Do not update only one host agent unless the user explicitly scopes the request to one target.
- If `rayman auxiliary` is unavailable but the canonical release binary exists, use the canonical release binary directly and then run `agent-skill sync`; do not report auxiliary AI as unsupported until both PATH and canonical release CLI have been checked.
