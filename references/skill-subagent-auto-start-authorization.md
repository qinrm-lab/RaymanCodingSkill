# Host Subagent Auto-Start Authorization

Source: `SKILL.md`

## Standing Authorization

- The user's standing instruction authorizes the primary AI to spawn Codex host subagents without asking for per-use approval when the host exposes subagents and the work is necessary, independent, and parallelizable.
- In an enabled RaymanCodingSkill workspace, the standing authorization has the same effect as the user explicitly saying `开启subagent`; the explicit phrase is not an additional precondition for suitable subagent dispatch.
- Codex host subagents are main-model or other strong-model child agents for speed. They are not `ai_ubuntu_8888` auxiliary AI and must not be treated as auxiliary advisory providers.
- Use `rayman subagent auto-start --task "<task>" --path <path>` when the primary agent needs host-tool-ready spawn requests. During non-trivial goal plan stages, `rayman goal run` and `rayman goal resume` may emit `HOST_SUBAGENT_DISPATCH_REQUEST {json}` with the same host-ready lane payloads.
- The primary AI must call the host `spawn_agent` tool with the lane payloads, or record an unavailable/failed closeout before continuing the serial path when host subagents cannot be spawned.
- Keep each delegated task bounded. Use independent explorer lanes or non-overlapping worker edit scopes, and omit model overrides unless the user explicitly requests a different model.
- Subagent output is advisory unless the primary AI assigns a bounded non-overlapping worker edit. The primary AI owns integration, validation, and final status.
- Record every spawned host subagent with `rayman subagent record`, record its result with `rayman subagent result`, and record primary-agent disposition with `rayman subagent review`. Bind records to `goal_id` and `dispatch_request_id` when a goal controller emitted the dispatch request.
- Unreviewed records, unresolved conflicts, overlapping scopes, parse-error ledger entries, or unclosed dispatch requests block `goal close --status success`, `session close --status success`, `rayman audit`, and `rayman gate status --check`.
