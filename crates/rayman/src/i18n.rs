use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use clap::ValueEnum;
pub const AUTHORED_MESSAGE_TEMPLATES: &[&str] = &[
    "已存最后一次快照并停止自动保存（状态：{status}）。计划任务 '{}' 未注册。",
    "workspace 未激活：已停止自动保存；计划任务 '{}' 未注册。最终快照已跳过；如需抢救快照，运行 `rayman checkpoint salvage-save`。",
    "Codex hooks 正被另一个进程修改: {}；等待锁超过 {} 秒",
    "  另有 {other} 份 recovery-only/partial 快照，用 `rayman checkpoint list` 查看。",
    "  工具 {}: 已找到",
    "  工具 {}: 不可达，本工作区不需要",
    "  工具 {}: 不可达",
    "    需要它来: {}",
    "遗留的原子写临时项 `{name}` 不安全或无效: {error:#}",
    "无法检查: {}",
    "不是普通文件: {}",
    "  工具 {}: {}",
    "  上下文: {} → 运行 `rayman context refresh`",
    "  上下文: {}",
    "  警告: {warning}",
    "orphan restore transaction 未能回滚，本次抢救快照可能捕获了恢复中途的工作区: {error:#}",
    "--supersedes 不能包含重复目标",
    "active goal 不能直接归档；先 `rayman goal close {id} --status partial`（或 blocked）如实记录结果，再归档",
    "archived goal 已有显式 receipt policy；拒绝降级或重复迁移",
    "authority goal 不存在: {authority_goal_id}",
    "authority goal 必须是同 workspace、current-policy 且包含同命令 direct-authority 的有效 archived success",
    "authority goal 缺少 lifecycle proof",
    "authority receipt invocation hash 无效",
    "authority receipt 与 requirement/command/scope 合同不匹配",
    "authority receipt 必须包含至少两次完整稳定执行",
    "authority receipt 未证明同一 workspace fingerprint 上的重复稳定 PASS",
    "checkpoint manifest 不是受支持的 v3 schema: schema={:?} version={}",
    "checkpoint manifest 含不安全相对路径: {text}",
    "checkpoint restore 失败且回滚不完整；已保留 transaction {} 供恢复: {operation_error:#}; rollback: {rollback_error:#}",
    "checkpoint restore 失败，工作区已回滚，但无法清理 transaction {}: {operation_error:#}; cleanup: {cleanup_error:#}",
    "checkpoint restore 失败，工作区已完整回滚: {operation_error:#}",
    "checkpoint {} 是 recovery-only；修复并重新激活工作区后，显式加 --allow-recovery-only 才能恢复",
    "checkpoint 不是完整快照（状态：{:?}）",
    "checkpoint 使用旧 v2 content-only manifest，缺少权限完整性证明，不能安全恢复；请用当前 Rayman 新建 v3 checkpoint",
    "checkpoint 完整性记录含无效 Unix mode: {}",
    "checkpoint 完整性记录缺少 Unix mode 权限证明，不能在 Unix 安全恢复: {}",
    "checkpoint 完整性记录缺少 readonly 权限证明（旧 content-only manifest），请新建 v3 checkpoint: {}",
    "checkpoint 已完整恢复，但无法清理 restore transaction {}",
    "checkpoint 总字节数溢出",
    "checkpoint 文件完整性不匹配: {}",
    "checkpoint 缺少 manifest",
    "checkpoint 缺少 manifest: {}",
    "checkpoint 路径不是普通文件: {}",
    "checkpoint 路径不是有效 UTF-8: {}",
    "checkpoint 路径为空",
    "checkpoint 路径含不安全组件: {}",
    "checkpoint 路径组件不是目录: {}",
    "context schema/workspace identity 不匹配",
    "context 索引包含读取失败条目: {}",
    "context 索引包含重复路径: {}",
    "goal plan --extend 拒绝事后补票；已有未计划变更: {}",
    "goal plan --extend 拒绝已发生变化的新路径: {added}",
    "goal plan --extend 至少需要一个变更路径",
    "goal plan --extend 要求恰好一个基础聚合 plan receipt",
    "goal plan 是首次修改前的一次性聚合合同，不能追加或拆分；请在变更前一次列出完整路径",
    "goal plan 至少需要一个变更路径",
    "lifecycle-only replacement 必须保持 pristine 且只能包含 open must",
    "lifecycle-only replacement 至少需要一个 --supersedes 目标",
    "live lifecycle authority 未证明当前源码上的重复稳定仓库 gate",
    "lifecycle-only authority {label} 不得晚于 replacement_authority.recorded_at",
    "lifecycle-only replacement authority proof 无效: {error}",
    "replacement_authority.recorded_at 不得晚于 goal.updated_at",
    "{label} 不得晚于 replacement_authority.recorded_at",
    "目标 {} 的 supersession 不得早于 replacement_authority.recorded_at",
    "被转移目标 {id} 的 supersession proof 无效: {error}",
    "被转移目标 {id} 的 supersession 不得早于 replacement_authority.recorded_at",
    "被转移目标 {id} 缺少 supersession proof",
    "manifest file_count={} 与完整性条目数={} 不一致",
    "manifest total_bytes={} 与文件完整性总和={} 不一致",
    "manifest 包含重复路径: {}",
    "manifest 含无效 SHA-256: {}",
    "orphan restore transaction 无法安全加载，已保留并拒绝继续: {}",
    "orphan restore transaction 没有 journal 却仍存有备份文件，无从判断该回滚哪些目标；已保留供人工恢复。恢复步骤：检查该目录 backups/ 子目录中的原件、取回仍需要的文件，再删除整个目录以解除对 save/restore/autosave 的阻塞：{}",
    "orphan restore transaction 自动回滚不完整，已保留并拒绝继续: {}",
    "pre-receipt migration 与 receipt-policy migration 不能同时使用",
    "legacy success migration 不能修复当前 command/plan/review 缺口: {}",
    "replacement must 必须与 --supersedes 目标 must（含 typed proof 义务）的精确并集一致",
    "replacement、authority goal 与被转移目标必须彼此不同",
    "restore journal committed 阶段条目状态不完整",
    "restore journal original/expected 路径不一致: {} != {}",
    "restore journal preparing 阶段含 publish_attempted 条目",
    "restore journal schema/version 不受支持: schema={:?} version={}",
    "restore journal 包含重复新建目录: {}",
    "restore journal 包含重复路径: {}",
    "restore journal 发布条目尚未预备目标: {}",
    "restore journal 回滚完成条目从未发布: {}",
    "restore journal 工作区不匹配: recorded={:?} current={:?}",
    "restore journal 新建目录不是任何恢复目标的祖先: {}",
    "restore journal 新建目录路径未规范化: {}",
    "restore journal 路径未规范化: {}",
    "restore staging 完整性不匹配: {}",
    "restore transaction 发布索引越界: {index}",
    "restore transaction 回滚索引越界: {index}",
    "restore transaction 目标尚未预备: {}",
    "restore transaction 缺少 journal.json",
    "restore 新建目录 journal 条目丢失",
    "restore 新建目录逃逸工作区: {}",
    "restore 目录只有创建意图且非空，无法证明所有权，拒绝删除 {}（{error}）；确认目录内容后手动删除它，再删除 transaction 目录 {} 以解除阻塞",
    "restore 目标在备份后发生变化，拒绝覆盖: {}",
    "restore 目标备份完整性不匹配: {}",
    "review receipt 必须绑定已记录的 goal plan",
    "reviewer 与 summary 都不能为空",
    "validation --changed 超出 goal plan: {}",
    "validation receipt 与 immutable goal/requirement contract 不匹配",
    "validation receipt 与命令/影响路径不匹配",
    "validation 拒绝未计划的实际变更: {}",
    "{path}: 无法持久化回滚完成状态: {error:#}",
    "manifest 记录 {} 实为 {}",
    "上下文文件在索引验证后发生变化: {} (size {} != {} or sha256 {} != {})",
    "上下文索引不是 ready（当前: {}）。先运行 `rayman context refresh`。{}",
    "上下文索引拒绝不安全相对路径: {}",
    "上下文索引拒绝链接/reparse 路径: {}",
    "上下文索引拒绝非普通文件: {}",
    "上下文索引文件不属于工作区: {} under {}",
    "上下文索引文件逃逸工作区: {} -> {}",
    "上下文索引无法哈希文件: {}",
    "上下文索引无法复查文件元数据: {}",
    "上下文索引无法统计文件行数: {}",
    "上下文索引无法读取文件: {}",
    "上下文索引无法读取文件元数据: {}",
    "上下文索引条目含不安全路径: {}",
    "上下文索引缺失。先运行 `rayman context refresh`。",
    "上下文索引读取期间文件发生变化: {}",
    "上下文索引路径组件不是目录: {}",
    "不安全的 checkpoint 目标路径: {}",
    "不安全的 checkpoint 相对路径: {}",
    "不能 supersede 目标 {id}: {error}",
    "历史 goal 不满足 receipt_integrity_v1；拒绝刷新 lifecycle proof",
    "历史 lifecycle proof 的 workspace fingerprint 非法，不能生成可核验隔离记录",
    "历史目标缺少旧 lifecycle proof，不能证明该归档证据曾经失效",
    "原子替换目标不是安全普通文件: {}",
    "原本不存在的 restore 目标在发布前出现，拒绝覆盖: {}",
    "发现不安全的 orphan restore transaction，已保留并拒绝继续: {}",
    "只允许隔离 proof 已失效的已归档 success，或无法生成可信归档 proof 的完整 current legacy success；有效或尚未结束的 current goal 不能隐藏",
    "只有 active/current 目标可以记录 plan receipt",
    "只有 active/success 的 current-schema 目标可以记录 review receipt",
    "只有 current goal 可以归档；已迁移的 archived goal 可用 --migrate-unreceipted 幂等刷新 proof",
    "只有 current goal 可以被 supersede",
    "只有 current-schema active/current 目标可以扩展 plan",
    "只有 must 已完整结束的 current-schema archived success 可以隔离",
    "只有 receipt-policy-v2 rollout 前的 schema-v2 success goal 可以迁移 v1 proof",
    "只有符合 rollout 前条件的 schema-v2 success goal 可以刷新 migration proof",
    "回滚后目标完整性不匹配: {}",
    "回滚备份完整性不匹配: {}",
    "回滚目标不是安全普通文件: {}",
    "回滚目标已被第三方修改，拒绝覆盖: {}。原件仍在该 transaction 的 backups/ 子目录中；取回仍需要的内容后删除整个 transaction 目录即可解除对 save/restore/autosave 的阻塞，salvage-save 不受阻塞",
    "完整 checkpoint manifest 含有跳过项或错误记录",
    "完整性记录含无效 SHA-256: {}",
    "工作区已偏离 goal 开工 baseline；拒绝事后补 plan。baseline={} current={}",
    "工作区遍历失败: {error:#}",
    "归档 success 的 lifecycle proof 仍然有效；拒绝把有效证据降级为 quarantine",
    "归档原因不能为空",
    "恢复前源文件完整性发生变化: {}",
    "恢复后目标文件完整性不匹配: {}",
    "恢复目标不是普通文件: {}",
    "恢复目标是目录而非文件: {}",
    "找不到 checkpoint: {id}",
    "拒绝关闭为 blocked：必须先记录至少一个带完整解决方案包的 human/external pending，且不能仍有 agent-owned pending",
    "拒绝关闭为 success：handoff contract invalid: {error}",
    "拒绝关闭为 success：legacy goal 不能生成当前 receipt；只可归档已是 success 的历史记录",
    "拒绝关闭为 success：必须先用 goal validate 写入当前且相关的 receipt: {}",
    "拒绝关闭为 success：目标合约无效: {error}",
    "拒绝写入 lifecycle-only replacement proof: {error}",
    "拒绝写入 lifecycle-only replacement: {error}",
    "拒绝恢复 checkpoint {}：它由旧版本 Rayman 生成，manifest 记录的是大小写折叠后的比较键而非真实文件名，按它恢复出的文件名大小写不可信（{}）；请用当前版本重新 `rayman checkpoint save` 后再恢复",
    "拒绝恢复 recovery-only checkpoint {}：当前激活合同尚未修复",
    "拒绝恢复非完整 checkpoint {}（状态：{:?}）",
    "拒绝清理不属于工作区 checkpoint 的 transaction: {}",
    "拒绝链接/reparse checkpoint 路径组件: {}",
    "拒绝链接/reparse 恢复目标: {}",
    "拒绝链接/reparse 路径: {}",
    "拒绝非普通文件: {}",
    "文件在读取期间变更或变为链接: {}",
    "新目标至少需要一个非空 --must 需求",
    "无效的 restore 新建目录 {}: {error:#}",
    "无法临时解除原子替换目标只读属性: {}",
    "无法列出 checkpoint 树目录: {}",
    "无法列出 checkpoint 目录: {}",
    "无法创建 restore backups: {}",
    "无法创建 restore staging: {}",
    "无法创建 restore transaction 目录: {}",
    "无法创建恢复目标目录: {}",
    "无法创建目标状态目录",
    "无法删除本次 restore 新建目录 {}: {error}",
    "无法删除本次新增 restore 文件: {}",
    "无法发布 restore 文件: {}",
    "无法同步目录: {}",
    "无法回滚 restore 目标: {}",
    "无法备份 restore 目标: {}",
    "无法复查 restore 目标: {}",
    "无法复查已验证上下文文件元数据: {}",
    "无法复查文件元数据: {}",
    "无法安全读取 context 状态: {error:#}",
    "无法扫描 restore transaction: {}",
    "无法扫描目录: {}",
    "无法持久化 restore journal: {}",
    "无法检查 orphan restore transaction: {}",
    "无法检查 restore journal: {}",
    "无法检查 restore 目标: {}",
    "无法检查回滚目标: {}",
    "无法检查目录条目: {}",
    "无法清理 restore transaction: {}",
    "无法清理旧 checkpoint: {}",
    "无法清理没有 journal 的 orphan restore transaction: {}",
    "无法规范化上下文文件: {}",
    "无法解析 goal 文件: current schema: {current_error}; legacy schema: {legacy_error}",
    "无法读取 checkpoint 树条目: {}",
    "无法读取 checkpoint 目录条目: {}",
    "无法读取 checkpoint 路径: {}",
    "无法读取 checkpoint 路径组件: {}",
    "无法读取 restore transaction 条目: {}",
    "无法读取原子替换目标权限: {}",
    "无法读取已验证上下文文件: {}",
    "无法读取已验证上下文文件元数据: {}",
    "无法读取恢复目标: {}",
    "无法读取恢复目标目录: {}",
    "无法读取文件元数据: {}",
    "无法读取目录条目: {}",
    "无法预备 restore staging 文件: {}",
    "替代目标 {replacement_id} lifecycle={}，必须先恢复为 current",
    "替代目标 {replacement_id} 合约无效: {error}",
    "替代目标 {replacement_id} 必须是 current schema；legacy success 只能显式 archive",
    "替代目标不存在: {id}",
    "替代目标不存在: {replacement_id}",
    "替代目标合约无效: {error}",
    "替代目标必须是未授权的 current/active current-schema goal",
    "未知 review_priority: {left}",
    "未知 review_priority: {right}",
    "未知 review_priority: {}",
    "未知历史 receipt policy；当前只支持 {RECEIPT_POLICY_V1}",
    "未知的关闭状态: {status}（可用: success | partial | blocked）",
    "本次新增 restore 目标已被第三方修改，拒绝删除: {}",
    "没有可恢复的 checkpoint",
    "没有完整且已验证的 checkpoint",
    "测试注入：第 {} 个 restore 文件发布失败 ({})",
    "目录",
    "目标 success receipt 未通过当前或历史完整性复核: {}。仅对应 rollout 前历史可显式使用 --migrate-unreceipted 或 --migrate-receipt-policy {RECEIPT_POLICY_V1}",
    "目标 {id} lifecycle={}，不能关闭；先用 `goal current {id}` 恢复为 current",
    "目标 {id} lifecycle={}，不能写入 receipt；先用 `goal current {id}` 恢复为 current",
    "目标 {id} lifecycle={}，不能追加证据；先用 `goal current {id}` 恢复为 current",
    "目标 {id} 不是当前 schema，不能写入可验证 receipt；请新建目标",
    "目标 {id} 不是当前 schema，不能记录 plan receipt",
    "目标 {} 不满足 retiring legacy-success plan reconciliation 条件",
    "目标 {id} 已关闭为 success，不能再追加人工证据；请用 `goal validate` 写入带 receipt 的验证，或先 supersede/archive",
    "目标 {id} 已关闭为 success，不能降级为 {status}；请用新的 baseline-bound goal supersede，或将该记录 archive",
    "目标 {id} 已隔离为 untrusted history；隔离是单向降级，审计记录必须保留，不能恢复为 current",
    "目标 {id} 已隔离为 untrusted history；隔离是单向降级，审计记录必须保留，不能用 migration 刷新为可信历史",
    "目标不能 supersede 自己",
    "目标包含多个 plan receipt；拒绝继续使用可拆分绕过的计划状态",
    "目标合约无效，不能 supersede: {error}",
    "目标合约无效，不能归档: {error}",
    "目标合约无效，不能迁移 historical policy: {error}",
    "目标合约无效，不能隔离 historical receipt: {error}",
    "目标已满足当前 receipt policy，不需要降级迁移",
    "目标已经是 untrusted history quarantine，不能重复隔离",
    "目标标题不能为空",
    "目标状态目录不存在",
    "目标缺少开工 baseline，不能扩展 plan",
    "目标缺少开工 baseline；请新建目标后在首次修改前执行 goal plan",
    "被转移目标 {predecessor_id} 合约无效: {error}",
    "被转移目标 {predecessor_id} 必须是 current 非 success current-schema goal",
    "被转移目标不存在: {predecessor_id}",
    "隔离原因不能为空",
    "隔离后的 lifecycle proof 无效: {error}",
    "隔离后的目标合约无效: {error}",
    "需求不存在: {req_id}",
    "非法目标 id: {id}（只允许字母、数字、下划线和连字符）",
    "验证证据说明不能为空",
    "验证命令不能启动 shell；PowerShell 脚本请用 `pwsh -NoProfile -File <script>.ps1 [参数...]` 这一种形式",
    "Cargo 拓扑权威确认（standard/release 就绪的硬前提）",
    "autosave 计划任务注册与注销",
    "找不到指定的路径",
    "未知 proof kind: {other}",
    "{TOPOLOGY_TOOL_UNAVAILABLE}: cargo 不在本进程 PATH 中",
    "{name} 不在本进程 PATH 中：安装器/工具链只改持久化 PATH，已经开着的终端不会继承；新开一个终端，或先把它的安装目录加进本进程 PATH",
    "源码状态、跟踪文件枚举与 clean-worktree 判定",
    "环境未就绪: {}；无法确认 Cargo 拓扑",
    "无法写入 checkpoint 根目录（默认在用户目录）: {}；受限沙箱下用 --dir 指定工作区内目录，或以主机权限重试",
    "  状态写探针: 写入被拒或探测失败（权限或 ACL）: {}",
    "  状态写探针: 可写；清理探针失败: {}",
    "  状态写探针: 状态目录不存在，未探测",
    "  状态写探针: 可写",
    "  激活元数据写探针: 就绪（原授权元数据 staging 已验证，激活文件未变）",
    "  激活元数据写探针: 无激活合同或平台不支持，未探测",
    "  激活元数据写探针: 失败 phase={:?} class={:?} os_error={} activation_unchanged={:?} cleanup_complete={:?}: {}",
    "workspace 激活合同结构上可 rebind，且当前 activation metadata staging 探针已就绪：运行 `{command}`",
    "workspace 激活合同结构上可 rebind，但当前 activation metadata staging 探针未就绪（phase={:?}, failure_class={}）；先按 failure_class 处理该 action-specific 能力边界，再运行 `{command}`",
    "RaymanCodingSkill 工作区未显式激活（status={}）：运行 `rayman workspace activate --skill-file <canonical-SKILL.md> --yes`；历史 .RaymanCodingSkill 状态不会自动激活 skill",
    "finish 要求当前稳定 authority receipt；先运行 `rayman goal validate {goal_id} --req <req> --message <evidence> --command <project-gate> --changed <path> --authority --repeat 2`",
    "`rayman audit` 已退役；工作区门禁使用 `rayman check --profile standard`，任务交付使用 `rayman finish --goal <id>`，状态卫生使用 `rayman state audit --check`",
    "legacy goal {} 仍为 current（status={}）；legacy 记录不能生成当前 receipt，请显式 archive 历史 success，或新建 current-schema replacement 后 supersede",
    "authority gate 必须是受检的 check-repo/audit-repository/verify-release-contract 脚本、`cargo test --workspace|--all`，或无路径选择器的全工作区 pytest；且不得使用缩小运行范围的选择器",
    "后台继续必须绑定 immediate human consultation，并同时记录非空 background-mechanism、background-authority-evidence 与 background-isolation-evidence",
    "human/external blocker 必须包含 attempts、evidence-path、minimum-input、recommended、alternative、risk、resume-command 与 auto-resume-condition",
    "current success 仍可生成可信 archive proof；拒绝降级为 quarantine，请使用普通 archive 或显式历史 receipt migration",
    "current goal 缺少开工 baseline；不能作为当前成功证据，请用新的 baseline-bound goal supersede，或将已完成记录显式 archive",
    "已安装身份契约不一致：{}",
    "PATH 上找不到 rayman：安装器只改持久化 PATH，已经开着的终端不会继承；新开一个终端，或先把安装目录加进本进程 PATH",
    "PATH 上的 rayman 与当前运行的二进制不是同一份：用仓库 release 二进制重新安装",
    "workspace 未激活：运行 `rayman workspace activate --skill-file <canonical-SKILL.md> --yes`",
    "workspace SKILL.md 与记录的 skill_sha256 不一致：SKILL.md 改动后需重新 activate 重绑",
    "最终 checkpoint 失败，active 状态已尝试回滚但计划任务重注册失败：checkpoint={checkpoint_error}; register={register_error}",
    "RaymanCodingSkill v2：多语言的上下文索引 / 目标 / 检查 / 恢复工作流\nMultilingual context / goal / check / recovery workflow",
    "`rayman context os{suffix}` 已退役；使用 `rayman context refresh` 更新内容索引，使用 `rayman check --goal <id>` 验证任务",
    "停止状态已写入，但最终 checkpoint 失败且 active 状态回滚失败：checkpoint={checkpoint_error}; state={state_error}",
    "lifecycle-only authority 必须是同 workspace、current-policy 且包含同命令 direct-authority 的有效 archived success",
    "`--changed` 证据必须同时提供至少一个 `--validated <command>`，避免把影响面建议误当作已验证事实。",
    "test summary 与独立 list proof 不一致：listed={listed} passed={passed} ignored={ignored}；拒绝混合/伪造输出",
    "项目地图: files={} source={} tests={} modules={} symbols={} deps={} packages={} package_deps={} entrypoints={} risks={}",
    "`rayman subagent` 已退役且 v2 不维护 agent ledger；需要保留未完成工作时使用 `rayman goal pending add`",
    "计划任务已注销，但停止状态写入失败且重新注册失败：state={state_error}; register={register_error}",
    "checkpoint 保存不完整（{} 个错误）；已保留 partial 快照 {} 供取证，不会替代最近完整快照{}",
    "验证命令第 {run_index}/{repeat} 次失败（exit={}）；不会写入 receipt。stdout_sha256={} stderr_sha256={}",
    "自动计划任务目前仅支持 Windows；其它平台请用系统定时器周期调用 `rayman checkpoint save`。",
    "计划任务已注册，但 active 状态写入失败且回滚失败：state={state_error}; rollback={rollback_error}",
    "自动保存：{}（每 {} 分钟，keep={}，auto_stop={}）\n  计划任务 '{}'：{}\n  最近一次触发：{}{}",
    "无法确定用户数据目录（未设置 LOCALAPPDATA/XDG_DATA_HOME/HOME/USERPROFILE），请用 --dir 指定",
    "superseded_by 目标 {replacement_id} lifecycle={}，必须为 current 或带有效 proof 的 archived success",
    "验证命令第 {run_index}/{repeat} 次修改了工作区内容；不会写入 receipt。before={} after={}",
    "恢复会用快照覆盖工作区里的同名文件。确认请加 --yes：rayman checkpoint restore --yes",
    "`rayman context task{suffix}` 已退役；使用 `rayman prepare --goal <id>` 或 `rayman goal show <id>`",
    "superseded_by archived 目标 {replacement_id} 是 untrusted history quarantine，不能作为完成证明",
    "workspace_skill.yaml 第 {line_number} 行包含不受支持的缩进；激活合同只接受顶层标量",
    "无法为原子写入创建独占临时文件（连续 {MAX_TEMP_NAME_ATTEMPTS} 个名称已存在）: {}",
    "无法为原子复制创建独占临时文件（连续 {MAX_TEMP_NAME_ATTEMPTS} 个名称已存在）: {}",
    "计划包含 {planned_paths} 个路径但没有 work package；建议分阶段绑定责任和恢复点",
    "authority validation 第 {run_index} 次运行前 workspace fingerprint 漂移；不会写入 receipt",
    "共享 quality policy 必须是工作区内普通文件，不能是链接/reparse/非常规文件: {}",
    "已存最后一次快照并停止自动保存（状态：{status}）。计划任务 '{}' 已注销。",
    "无法为计划任务 XML 创建独占临时文件（连续 {MAX_NAME_ATTEMPTS} 个名称已存在）",
    "pytest summary 与独立 collect proof 不一致：listed={listed} passed={passed} ignored={ignored}",
    "superseded_by 目标 {replacement_id} 必须是 current schema，legacy success 只能显式 archive",
    "一条真正验证该变更的命令；rayman 自身与 --version/--help 之类的查询不是证据",
    "maintenance cycle rebind 要求 archived command 恰好包含一个 -MaintenanceOrchestrationCycle",
    "已存初始快照 {}（{} 个文件）并注册计划任务 '{}'：每 {} 分钟自动快照{}。",
    "`rayman workspace-skill` 已退役；使用 `rayman workspace status|inspect|activate|rebind|deactivate`",
    "共享 quality policy 的父目录必须是工作区内真实目录，不能是链接/reparse: {}",
    "已保存 recovery-only 快照 {} — {} 个文件；它不会成为默认 latest 或完成证据",
    "validation 必须提供至少一个 `--changed`；非代码需求必须显式使用 `--non-code`；零变更 authority 审计使用 `--workspace-snapshot`",
    "`--workspace-snapshot` 不能与 `--changed` 或 `--non-code` 同时使用",
    "--workspace-snapshot 只允许与 --authority 一起使用",
    "--workspace-snapshot 要求 goal baseline delta 为空；发现真实变更: {}。验证命令尚未执行",
    "workspace snapshot receipt 必须是 authority receipt",
    "workspace snapshot receipt 要求 goal baseline delta 为空；发现真实变更: {}",
    "托管临时目录: {} (exists={}, entries={}, files={}, dirs={}, {:.1} MB, traversal_errors={})",
    "lifecycle authority 第 {run_index} 次运行前 source fingerprint 漂移；不会写入 proof",
    "停止状态写入失败；计划任务已重新注册，autosave 保持 active：{state_error}",
    "工作区遍历不完整（{} 个错误），拒绝把不完整结果当作完整文件集: {}",
    "自动保存状态损坏或不可读取；未修改状态，也未注销计划任务：{error}",
    "计划包含 {planned_paths} 个路径但尚无 progress receipt；长任务存在证据悬崖",
    "验证命令不允许 shell 控制符或命令替换；请提供单一可执行程序及参数",
    "不支持的 goal schema_version={}（当前只接受 v{}；请迁移或重新创建目标）",
    "测试验证命令包含非执行模式 {flag}；receipt 必须实际运行至少一个测试",
    "代码构建/测试命令不能声明为 `--non-code`；必须绑定实际 `--changed` scope",
    "success goal {} 的 must 需求 {} 没有绑定当前工作区的成功 validation receipt",
    "progress receipt 必须证明同一源码快照上的零退出执行与有效输出摘要",
    "human/external blocker 缺少完整 solution package，不能作为咨询或等待边界",
    "lifecycle authority 第 {run_index}/{repeat} 次失败（exit={}）；不会写入 proof",
    "work package id 只允许字母、数字、下划线和连字符，且标题不能为空",
    "lifecycle authority 第 {run_index}/{repeat} 次修改了工作区；不会写入 proof",
    "检测到全部目标均为 success：已存最后一次快照并停止自动保存。",
    "目标 {} 已获 lifecycle-only replacement authority（source={}，predecessors={}）",
    "受管状态包含退役条目或遍历错误；先审阅 `rayman state audit` 输出",
    "maintenance cycle rebind 路径必须是使用 / 的非空 workspace-relative 路径",
    "查询 autosave 计划任务失败，不能把未知状态当作未注册：{detail}",
    "测试命令成功退出但没有可验证的 passed>0 汇总；不会写入 receipt",
    "maintenance cycle rebind 路径禁止 absolute、.、..、prefix 或 root component",
    "未知 lane mode: {value}（可用: advisory-read-only | writer | final-reviewer）",
    "计划已扩展 {plan_extensions} 次；请复核范围是否仍属于同一目标",
    "superseded_by 目标 {replacement_id} 状态为 {}，必须先 gate-ready success",
    "原子复制临时文件完整性不匹配: {} (size {} != {} or sha256 {} != {})",
    "项目地图已刷新: modules={} symbols={} dependencies={} packages={} risks={}",
    "goal {} 仍为 active；用 goal validate 记录实际验证后必须 goal close",
    "maintenance cycle rebind 只接受 cycle-qualified maintenance-review-cycle.json",
    "缺少 baseline 的 goal 不能携带 plan/review/authority/work-package receipt",
    "要求绑定唯一 current goal，但当前有 {} 个；请显式传 --goal <id>",
    "goal {} lifecycle-only replacement 无法在缺少 source fingerprint 时验证",
    "pytest 成功退出但没有可验证的 passed>0 汇总；不会写入 receipt",
    "要求绑定 current goal，但当前没有 current goal；先运行 goal start",
    "current goal 不能保留 lifecycle_reason、superseded_by 或 lifecycle_proof",
    "nextest 暂无独立 list proof 支持；请使用 `cargo test` 生成 receipt",
    "work package 完成要求同包且绑定当前源码快照的 progress receipt",
    "workspace_skill.yaml 第 {line_number} 行包含未闭合或不匹配的引号",
    "任务门禁要求 ready context；使用 --refresh-context 或 prepare/finish",
    "已按显式 retention policy 保留最近 {} 个完整快照，删除 {} 个",
    "当前工作区暂无快照。运行 `rayman checkpoint save` 创建一个。",
    "  发布交接状态: 未检查（本结果仅是工作区 strict-quality）",
    "共享 quality policy 逃逸工作区或不在精确 policy 目录: {} -> {}",
    "验证命令不能启动 shell；请直接提供要执行的程序及参数",
    "archived -MaintenanceOrchestrationCycle 不是 cycle-qualified JSON 路径",
    "high-priority plan 缺少绑定最终源码 fingerprint 的 review receipt",
    "legacy plan 时间顺序必须满足 goal <= baseline <= receipt <= extensions <= updated",
    "  质量: profile={} ready={} errors={} warnings={} covered_sources={}/{}",
    "状态正在被另一个 rayman 进程修改: {}；等待锁超过 {} 秒",
    "lifecycle-only replacement 合约、baseline 或专用迁移形态无效",
    "maintenance cycle rebind 未精确绑定 archived command 的 flag/value",
    "maintenance cycle rebind 路径不得经过 symlink/junction/reparse: {}",
    "只有 current-schema active/current 目标可以记录 progress receipt",
    "索引已刷新: 共 {} 个文件（复用 {}，重算 {}，移除 {}）",
    "goal {} package {} progress receipt {} 已记录（non-authoritative）",
    "historical goal {} lifecycle={} 已保留但不参与当前 readiness{}",
    "validation 不覆盖 {}；需要同一条当前成功 receipt 绑定 {}",
    "  仓库源码产物: 未由 doctor 检查；交接/CI 由 `{}` 验证",
    "  资产: 过时候选 {}，未完成标记 {}（提示，不阻塞）",
    "lifecycle authority 独立 test list proof 失败；不会写入 proof",
    "受管状态存在，但缺少显式 workspace_skill.yaml 激活合同",
    "实际变更 {} 个文件但缺少首次修改前的 goal plan receipt",
    "独立 test list proof 没有列出任何测试；不会写入 receipt",
    "  源码新鲜度: 未由 doctor 证明；交接/CI 必须运行 `{}`",
    "prepare 要求 current active goal；{} 当前 lifecycle={} status={}",
    "未知 consultation timing: {value}（可用: deferred | immediate）",
    "PowerShell 验证脚本不在当前工作区的受检文件集合中",
    "pytest collect proof 没有收集任何测试；不会写入 receipt",
    "superseded_by archived 目标 {replacement_id} proof 无效: {error}",
    "workspace_skill.yaml 第 {line_number} 行缺少 key/value 分隔符",
    "只有 current-schema active/current 目标可以增加 work package",
    "\n  连续失败：{} 次（最近一次：{}）\n  最近错误：{}",
    "lifecycle-only replacement delta 未被 predecessor plan 覆盖: {}",
    "work package {package_id} 已完成，不能追加 progress receipt",
    "workspace_skill.yaml 第 {line_number} 行包含未知字段: {key}",
    "workspace_skill.yaml 第 {line_number} 行包含重复字段: {key}",
    "未知 pending owner: {value}（可用: agent | human | external）",
    "绑定的 goal {id} status={}，必须完成验证并 close success",
    "  项目地图: modules={} symbols={} deps={} packages={} risks={}",
    "PowerShell 验证脚本必须是工作区内的普通 .ps1 文件",
    "goal {id} lane {lane_id} 已关闭：delta={} authoritative=false",
    "goal {} 需求 {} 没有 impact 快照；非代码变更可忽略",
    "lifecycle-only replacement 当前 delta 与授权 proof 不一致",
    "replacement must 与被转移目标 must（含 typed proof 义务）的精确并集不一致",
    "{name} 在 PATH 上只有本进程无法启动的 {} —— rayman 用 `Command::new` 直接创建进程，Windows 只会补 `.exe`，不解析 PATHEXT；请把真正的 {name}.exe 所在目录加进 PATH（或改用提供 .exe 的安装方式）",
    "自动停止失败: {error:#}",
    "嵌套 Cargo manifest 超过 {MAX_NESTED_METADATA_MANIFESTS} 个，已停止逐个解析；把它们纳入同一个 workspace（根 Cargo.toml 的 `[workspace] members`），或把 fixture manifest 排除出索引",
    "skill_file 路径不能以引号开头或结尾（激活合同按未加引号的标量写入）: {recorded_path}",
    "当前工作区没有可恢复的 standard 快照；另有 {other} 份 recovery-only/partial 快照，用 `rayman checkpoint list` 查看。",
    "状态锁正被另一个 rayman 进程占用: {}",
    "停止状态写入失败且计划任务重注册失败：state={persist_error}; register={register_error}",
    "workspace 未激活且没有自动保存状态，无需停止",
    "workspace 未激活，跳过快照: {error:#}",
    "workspace 未激活，最终快照已跳过: {activation_error:#}",
    "workspace 未激活：已停止自动保存并注销计划任务 '{}'。最终快照已跳过；如需抢救快照，运行 `rayman checkpoint salvage-save`。",
    "无法检查被跟踪文件 {}: {error}",
    "需求 {} {gap}",
    "需求 {} 缺少 evidence 文本",
    "需求 {} 缺少验证 receipt",
    "注销计划任务失败（任务仍可能在运行）：{detail}",
    "独立 test list proof 失败（exit={}）；不会写入 receipt",
    "资产扫描: 干净（无过时候选、无未完成标记）。",
    "maintenance cycle rebind 路径必须是唯一规范文本形式",
    "已记录 {req} 证据（目标 {}，impact={}，validated={}）",
    "未启用自动保存。运行 `rayman autosave start` 开启。",
    "checkpoint prune 会删除旧恢复点；传 --yes 显式确认",
    "live lifecycle authority receipt 无效或未绑定当前源码",
    "重复执行只用于 authority gate；请同时传 --authority",
    "lifecycle-only replacement 只接受 current-v2 receipt policy",
    "Cargo workspace 拓扑未获 cargo metadata 权威确认: {}",
    "authority receipt 未证明重复稳定执行或摘要无效",
    "goal plan receipt 无效、未规范化或未绑定 baseline",
    "lifecycle-only replacement 未显式绑定被替代目标 {}",
    "required work package {} 未完成或缺少 progress receipt",
    "无法执行 cargo metadata（cargo 是否在 PATH 中？）",
    "无自动保存状态，也没有已注册的计划任务。",
    "未证明完成的 goal，其 must 未完整转移到 replacement: {}",
    "verified replacement transfer 只允许无额外 migration 的 superseded current-schema success",
    "--authority 要求 --repeat >= 2，以证明稳定固定点",
    "progress 命令修改了源码快照；不会写入 receipt",
    "superseded_by 目标 {replacement_id} 合约无效: {error}",
    "superseded_by 目标 {replacement_id} 尚未 gate-ready: {}",
    "受管状态目录逃逸工作区: {} -> {} (工作区: {})",
    "lifecycle-only replacement 来自不同 workspace identity",
    "pytest collect proof 出现在 stderr，来源不可区分",
    "自动保存失败状态也未能写入: {persist_error:#}",
    "goal 只能携带一个不可拆分的聚合 plan receipt",
    "goal 规划检查与调用方当前 fingerprint 不一致",
    "lane {lane_id} 关闭检查期间源码快照发生漂移",
    "progress 命令失败（exit={}）；不会写入 receipt",
    "允许的状态项 `{name}` 不安全或无效: {error:#}",
    "已执行并记录 {} 的可验证 receipt（目标 {}）",
    "无法取得状态独占锁（权限或 ACL 拒绝）: {}",
    "无法验证共享 quality policy containment {}: {error}",
    "注册成功后回滚计划任务失败：任务未找到",
    "advisory-read-only/final-reviewer lane 不接受 --allow",
    "goal {} lifecycle-only replacement proof 无效: {error}",
    "lane id 只允许字母、数字、下划线和连字符",
    "success goal {} 的 must 需求 {} 未处于 done 状态",
    "-MaintenanceOrchestrationCycle 缺少独立路径参数",
    "external owner 只允许 external_wait/repair_exhausted",
    "lane --allow 只接受普通工作区相对文件路径",
    "lifecycle authority --repeat 必须在 2..=10 范围内",
    "lifecycle-only replacement source fingerprint 已过期",
    "pending.json 第 {} 项（id={}）合同无效: {error}",
    "显式 v1 receipt policy proof 缺少受控迁移标记",
    "agent-owned pending 不能伪装成人工/外部边界",
    "lifecycle-only replacement proof 结构或摘要无效",
    "上下文索引: {} (changed={}, added={}, removed={})",
    "实际变更未被当前 validation receipt 声明: {}",
    "无自动保存状态，遗留计划任务已注销。",
    "自动保存已停止，遗留计划任务已注销。",
    "  位置: .RaymanCodingSkill/context/project_map.json",
    "goal {id} lane {lane_id} 已打开（mode={mode:?}）",
    "maintenance cycle rebind 目标不是普通文件: {}",
    "test list proof 出现在 stderr，来源不可区分",
    "只有 current-schema active/current 目标可以完成 work package",
    "目标目录含不可安全读取的记录: {details}",
    "被转移目标 {id} 的合约或 lifecycle 已失效",
    "；已轮换掉 {rotated} 份更旧的 partial 快照",
    "Cargo manifest 不在已验证上下文索引中: {}",
    "lane ledger id、baseline 或 authority 标记无效",
    "lifecycle-only authority goal 缺少 lifecycle proof",
    "lifecycle_proof 使用了无效的 legacy quarantine",
    "lifecycle_proof 使用了无效的 receipt integrity quarantine",
    "pytest lease manifest 与受管路径不一致: {id}",
    "pytest summary 出现在 stderr，来源不可区分",
    "pytest 成功退出但缺少可验证的终端汇总",
    "work package 完成写入前源码快照发生漂移",
    "work package 指向未知 requirement: {requirement}",
    "无法复算 lifecycle-only replacement 当前 delta",
    "绑定的 goal {id} lifecycle={}，必须为 current",
    "，按显式 retention policy 清理旧快照 {} 个",
    "checkpoint 暂存目录已存在，拒绝覆盖: {}",
    "checkpoint 目标目录已存在，拒绝覆盖: {}",
    "goal baseline 文件清单与 fingerprint 不匹配",
    "maintenance cycle rebind 目标逃逸 workspace: {}",
    "workspace_skill.yaml 第 {line_number} 行缺少值",
    "writer lane 必须声明至少一个 --allow 路径",
    "无法打开状态锁（权限或 ACL 拒绝）: {}",
    "无法读取 lifecycle-only authority goal: {error}",
    "  交接/CI 必须运行 `{SOURCE_FRESH_VERIFIER}`",
    "  模块: {} kind={} lines={} symbols={} public={}",
    "goal baseline fingerprint 与文件清单不匹配",
    "goal {} 状态为 {}，不能作为 standard READY",
    "id、title、detail 与 created_at 都不能为空",
    "progress receipt 写入前源码快照发生漂移",
    "test summary 出现在 stderr，来源不可区分",
    "已保存快照 {} — {} 个文件 ({:.1} MB){}{}",
    "无法启动 schtasks 查询 autosave 计划任务",
    "  运行 `rayman context refresh` 更新索引。",
    "archived goal 必须记录非空 lifecycle_reason",
    "checkpoint {} 已验证：{} 个文件，{:.1} MB",
    "goal progress receipt 结构或源码绑定无效",
    "skill_sha256 与 skill_file 当前内容不一致",
    "过时资产候选（提示，不自动删除）:",
    "项目拓扑: packages={} package_dependencies={}",
    "lifecycle-only replacement proof 无效: {error}",
    "progress --message 与 --command 都不能为空",
    "历史化时的 success receipt proof 无效: {}",
    "复制后的文件与源文件完整性不一致",
    "无法原子替换复制目标 {} -> {}: {error}",
    "无法读取 PowerShell 验证脚本 {}: {error}",
    "源文件在 checkpoint 复制期间发生变化",
    "自动保存已停止，计划任务未注册。",
    "计划任务 XML 写入后被替换或截断: {}",
    "    BLOCKER: pending.json 不可读取: {error}",
    "`--non-code` 不能与 `--changed` 同时使用",
    "superseded_by 目标不存在: {replacement_id}",
    "active goal {} 的 must 需求 {} 仍未完成",
    "autosave 独占锁不是安全普通文件: {}",
    "lifecycle_proof 与当前 goal 合约不匹配",
    "lifecycle_proof 使用了无效的历史迁移",
    "maintenance cycle rebind 文件 hash 已漂移",
    "pytest lease {} 已创建并通过读写探针",
    "写入后的工作区激活合同仍无效: {}",
    "原子写入父目录不安全或不存在: {}",
    "原子复制父目录不安全或不存在: {}",
    "发现不属于工作区根的候选文件: {}",
    "checkpoint 锁被替换为非普通文件: {}",
    "goal {id} work package {package_id} 已创建",
    "goal {id} work package {package_id} 已完成",
    "must {} 缺少当前成功 validation receipt",
    "read-only/reviewer lane {} 发生源码漂移",
    "superseded goal 必须记录 lifecycle_reason",
    "只有 current-schema active/current 目标可以打开 lane",
    "只有 current-schema current 目标可以关闭 lane",
    "无法检查共享 quality policy {}: {error}",
    "  workspace SKILL 一致: {metadata_matches}",
    "lane {lane_id} 关闭被拒绝：{violation}",
    "lifecycle-only replacement proof hash 无效",
    "lifecycle-only replacement 必须是 success",
    "test command 缺少独立 list/collect proof",
    "工作区就绪检查({readiness_scope}): {}",
    "已完成 work package {} 缺少完成时间",
    "无法检查共享 policy 目录 {}: {error}",
    "  PATH 命令一致: {path_matches_running}",
    "`--message` 与 `--command` 都不能为空",
    "pending 绑定的 goal 不存在: {goal_id}",
    "required work package {} 缺少进度收据",
    "{kind}不会被跟随，遍历不完整: {}",
    "原子复制发布前父目录不安全: {}",
    "拒绝遍历链接/reparse 临时条目: {}",
    "拒绝链接/reparse 受管状态文件: {}",
    "拒绝链接/reparse 受管状态目录: {}",
    "无法创建工作区 checkpoint 目录: {}",
    "资产扫描无法读取文件元数据: {}",
    "authority receipt 指向未知 requirement",
    "checkpoint 锁不是安全普通文件: {}",
    "goal {} supersession 合约无效: {error}",
    "goal 包含空的 requirement id 或文本",
    "lifecycle-only replacement 缺少 baseline",
    "superseded goal 必须记录 superseded_by",
    "无法原子替换文件 {} -> {}: {error}",
    "无法计算 goal 实际变更集: {error}",
    "无法计算工作区内容指纹: {error}",
    "  已安装身份 READY: {identity_ready}",
    "无法安全读取受管状态: {error:#}",
    "无法读取被转移目标 {id}: {error}",
    "archived goal 不能设置 superseded_by",
    "autosave 锁句柄不是普通文件: {}",
    "goal {} 需求 {} 缺少 evidence 文本",
    "lane {} allowed_paths 与 mode 不匹配",
    "pending title 与 detail 都不能为空",
    "work package {} requirement 绑定无效",
    "work package {} 进度收据引用无效",
    "不安全的受管状态相对路径: {}",
    "受管状态路径不是普通文件: {}",
    "已从快照 {} 恢复 {} 个文件{}。",
    "无法创建 checkpoint 暂存目录: {}",
    "无法创建原子复制临时文件: {}",
    "无法同步原子复制临时文件: {}",
    "无法复制 checkpoint 文件: {} -> {}",
    "无法校验原子复制临时文件: {}",
    "无法读取原子复制源元数据: {}",
    "无法读取目标状态目录条目: {}",
    "未知 lifecycle receipt policy: {other}",
    "状态锁被替换为非普通文件: {}",
    "；partial 快照轮换失败: {error:#}",
    "  {}  {:?}/{:?}  {} 个文件  {:.1} MB",
    "`--validated <command>` 不能为空。",
    "goal {} lifecycle proof 无效: {error}",
    "lifecycle-only authority goal 不存在",
    "must {} immutable contract 无法计算",
    "open work package {} 含完成态字段",
    "保存后完整性验证失败: {error}",
    "无法创建 pytest lease 子目录: {}",
    "goal {} 需求 {} 缺少验证 receipt",
    "progress receipt 源码快照已过期",
    "pytest lease 探针内容不一致: {}",
    "superseded goal 缺少 lifecycle_proof",
    "无法独占创建计划任务 XML: {}",
    "疑似过时文件名后缀 `{suffix}`",
    "等待 autosave 独占锁超过 {} 秒",
    "错误: 无法序列化输出: {error}",
    "goal progress receipt {} 摘要无效",
    "maintenance cycle rebind 结构无效",
    "v2 状态路径不是普通文件: {}",
    "work package 父子图包含环: {id}",
    "work package 父节点不存在: {id}",
    "原子写入目标没有父目录: {}",
    "原子发布前父目录不安全: {}",
    "原子复制目标没有父目录: {}",
    "只读 lane 检测到源码漂移: {}",
    "拒绝链接/reparse {label}: {shown}",
    "拒绝链接/reparse 状态文件: {}",
    "无法创建 checkpoint 树目录: {}",
    "无法规范化 checkpoint 目录: {}",
    "无法规范化受管状态目录: {}",
    "无法读取{label}元数据: {shown}",
    "无法读取允许的状态文件: {}",
    "状态锁不是安全普通文件: {}",
    "archived goal 缺少 lifecycle_proof",
    "goal 包含重复 requirement id: {}",
    "work package 不存在: {package_id}",
    "work package 已存在: {package_id}",
    "受管状态文件路径不能为空",
    "已存快照 {}（{} 个文件）。",
    "父 work package 不存在: {parent}",
    "验证命令不覆盖 {}；需要 {}",
    "验证命令包含未闭合的引号",
    "  {}  {:?}  (缺或损坏 manifest)",
    "  候选相关测试(启发式): {}",
    "--repeat 必须在 1..=10 范围内",
    "goal 至少需要一个 must 需求",
    "must {} 未完成或缺少 evidence",
    "pytest lease 清理探针失败: {}",
    "skill_file 路径不能包含换行",
    "只有 success/partial/blocked goal 可以 archived",
    "无法复查 autosave 独占锁: {}",
    "无法定位当前 rayman 二进制",
    "无法打开 autosave 独占锁: {}",
    "无法读取 autosave 锁句柄: {}",
    "疑似过时命名标记 `{marker}`",
    "绑定的 goal 不存在: {goal_id}",
    "项目地图中没有文件: {path}",
    "goal 状态为 {}，不是 success",
    "lifecycle_proof 包含非法摘要",
    "pytest lease 缺少 manifest: {id}",
    "skill 必须精确为 {SKILL_NAME}",
    "writer lane 越出允许路径: {}",
    "不支持的临时条目类型: {}",
    "任务准备完成: {} (status={})",
    "受管状态路径不是目录: {}",
    "无法创建 checkpoint 目录: {}",
    "无法创建受管状态目录: {}",
    "无法检查 checkpoint 目录: {}",
    "无法读取受管状态文件: {}",
    "无法读取受管状态目录: {}",
    "无法读取目标状态目录: {}",
    "无法遍历目标状态目录: {}",
    "等待 checkpoint 锁超过 {} 秒",
    "证据 `--message` 不能为空。",
    "资产扫描无法读取文件: {}",
    "  依赖: outgoing={} incoming={}",
    "closed lane {} 关闭证明无效",
    "writer lane {} 越出允许路径",
    "受管状态审计: clean={clean}",
    "无托管临时目录可清理。",
    "无法取得 checkpoint 独占锁",
    "无法提交 checkpoint: {} -> {}",
    "至少提供一个变更路径。",
    "需求不存在: {requirement_id}",
    "验证命令缺少可执行程序",
    "goal 文件不可读取: {} ({})",
    "lane {} delta_paths 未规范化",
    "pytest lease 写探针失败: {}",
    "pytest lease 读探针失败: {}",
    "validation contract 无法计算",
    "{label} 不能包含空字符串",
    "找不到可验证的 checkpoint",
    "无法写入计划任务 XML: {}",
    "无法复查计划任务 XML: {}",
    "无法解析 cargo metadata JSON",
    "无法读取 canonical skill: {}",
    "无法读取 v2 状态文件: {}",
    "cli_contract 必须精确为 {}",
    "open lane {} 含关闭态字段",
    "work package 图无效: {error}",
    "无法取得 autosave 独占锁",
    "无法复制到临时文件: {}",
    "无法复查 checkpoint 锁: {}",
    "无法打开 checkpoint 锁: {}",
    "无法检查 checkpoint 锁: {}",
    "无法规范化工作区根: {}",
    "无法读取原子复制源: {}",
    "无法读取受管状态根: {}",
    "cli_version 必须精确为 {}",
    "已清理托管临时目录。",
    "当前工作区暂无快照。",
    "无法创建受管临时目录",
    "目标 {} 已恢复为 current",
    "绑定的 goal 不存在: {id}",
    "被转移目标不存在: {id}",
    "项目地图不可用: {error}",
    "；未裁剪任何旧恢复点",
    "goal {} 合约无效: {error}",
    "goal_id 不能是空字符串",
    "{label} 不能是空字符串",
    "已安装身份契约: {} v{}",
    "无法释放 pytest lease: {}",
    "注册计划任务失败：{}",
    "lane id 已存在: {lane_id}",
    "pytest lease {} 探针通过",
    "pytest lease 不存在: {id}",
    "work package id/title 无效",
    "{label}不是目录: {shown}",
    "文件在枚举后消失: {}",
    "无效 pytest lease id: {id}",
    "无法保留复制权限: {}",
    "无法写入临时文件: {}",
    "无法执行验证程序: {}",
    "无法清理临时目录: {}",
    "无法确定文件类型: {}",
    "无法读取临时目录: {}",
    "无法读取状态目录: {}",
    "项目地图读取失败: {}",
    "  风险提示: warnings={}",
    "pytest lease {id} 已释放",
    "实际变更超出 plan: {}",
    "待完成项不存在: {id}",
    "无法规范化工作区根",
    "goal review receipt 无效",
    "目标 {} 已由 {} 取代",
    "重复 work package id: {}",
    "cargo metadata 失败: {}",
    "lane 不存在: {lane_id}",
    "无法创建 pytest lease",
    "无法同步父目录: {}",
    "无法复查状态锁: {}",
    "无法检查状态锁: {}",
    "目标 {} 已关闭为 {}",
    "符号匹配: {} ({} 个)",
    "已解决待完成项。",
    "已记录待完成项 {}",
    "找不到指定的文件",
    "指定的任务不存在",
    "无法读取当前目录",
    "暂无 current 目标。",
    "未知的允许状态项",
    "目录在枚举后消失",
    "目标 {} 已归档：{}",
    "验证命令不能为空",
    "，完成后自动停止",
    "  package 依赖方: {}",
    "goal id 或标题为空",
    "goal {} 需求 {} {gap}",
    "快照（旧→新）:",
    "无法打开文件: {}",
    "无法读取文件: {}",
    "缺少 lifecycle_proof",
    "  当前二进制: {}",
    "  直接依赖方: {}",
    "无法解析 JSON: {}",
    "目标不存在: {id}",
    "  package 依赖: {}",
    "  证据: {evidence}",
    "lane {} 尚未关闭",
    "无法序列化 JSON",
    "项目质量({}): {}",
    "enabled 不是 true",
    "未找到符号: {}",
    "缺少 skill_sha256",
    "  建议依据: {}",
    "  待完成项: {}",
    "  历史待完成项（保留，不阻塞）: {}",
    "  直接依赖: {}",
    "受管状态目录",
    "无待完成项。",
    "（未知时间）",
    "缺少 skill_file",
    "错误: {error:#}",
    "      建议: {}",
    "变更计划: {}",
    "影响分析: {}",
    "未完成标记:",
    "  建议验证:",
    "  文件分组:",
    "找不到任务",
    "暂无目标。",
    "非常规文件",
    "（未记录）",
    "，{} 个失败",
    "  位置: {}",
    "  符号: {}",
    "  风险: {}",
    "临时目录",
    "符号链接",
    "（尚无）",
    "文件: {}",
    "  风险:",
    "已停止",
    "已注册",
    "待完成",
    "未完成",
    "未注册",
    "运行中",
    "待办",
    "RaymanCodingSkill 工作区激活身份已漂移（status={}）：运行 `{command}`",
    "skill_file 不可安全读取 {}: {error}",
    "skill_file 不是普通文件: {}",
    "skill_file 当前内容与此 CLI 内嵌的 canonical SKILL.md 不一致",
    "skill_file 在 rebind 写后验证期间发生并发变化",
    "skill_file 在 rebind 发布前发生并发变化",
    "skill_file 在校验期间发生变化: {}",
    "skill_file 路径不得经过链接/reparse: {}",
    "skill_file 路径不是 activate 可生成的安全规范形式",
    "skill_file 路径为空",
    "skill_file 路径组件不是目录: {}",
    "workspace rebind 只接受 enabled: true",
    "workspace rebind 只接受 skill: {SKILL_NAME}",
    "workspace rebind 只接受完整六字段激活合同",
    "workspace rebind 已无需更新，但激活状态仍无效",
    "workspace rebind 拒绝无法原样安全写回的 skill_file",
    "workspace rebind 拒绝格式无效的旧身份字段",
    "workspace rebind 缺少 skill_file",
    "workspace_skill.yaml 在 rebind 发布前发生并发变化",
    "workspace_skill.yaml 必须是有效 UTF-8",
    "写入后的 workspace rebind 合同仍无效: {}",
    "原激活合同不是有效 UTF-8",
    "当前 workspace 激活合同不可 rebind: {}",
    "无法复查 skill_file: {}",
    "无法规范化 skill_file: {}",
    "无法读取 skill_file 路径组件: {}",
    "`goal pending present` 已退役：代理不能自证用户展示。使用 `rayman goal pending render --current` 生成 client-neutral workspace aggregate；渲染本身不构成展示、送达或完成证据",
    "agent pending 不需要 capability-bound legacy migration",
    "capability-bound pending 必须绑定 --goal",
    "capability-bound pending 必须绑定 goal_id",
    "capability_key 与 boundary_class 必须使用稳定规范化形式",
    "capability_key 与 boundary_class 必须同时存在或同时缺省",
    "committed plan publication 必须证明 confirmed==precheck 并记录 committed_at",
    "committed plan publication 的 committed_at 必须是 RFC3339 且不早于 published_at",
    "execution-context requirement 未满足（仅约束本次 doctor 检查，不构成提权或 ACL 授权）：{}",
    "extension intent 缺少 pending 链尾",
    "goal {} 当前不能 render human-boundary aggregate: decision={:?} consultation={:?} reason={}",
    "goal 不属于 {PLAN_PUBLICATION_POLICY_V1} plan publication epoch；旧 goal 只能读取或退休，不能扩展计划",
    "goal 不属于 {PLAN_PUBLICATION_POLICY_V1} plan publication epoch；旧 goal 只能读取或退休，不能追加计划",
    "goal 存在未完成且与当前源码不匹配的 plan extension intent；必须恢复 intent 的 precheck 快照后用同一扩展重试或退休该 goal",
    "goal 存在未完成且与本次调用不匹配的 plan extension intent；拒绝覆盖，必须用原扩展参数重试或退休该 goal",
    "goal 存在未完成且与本次调用不匹配的 plan publish intent；拒绝覆盖，必须恢复 intent 的 precheck 快照后用同一计划重试或退休该 goal",
    "goal 存在未完成的 plan publish intent（kind={:?} intent_sha256={}）；源码可能在计划发布窗口内漂移，必须恢复原快照后重试或退休该 goal",
    "goal.updated_at 不得早于最终 plan publication",
    "goal.updated_at 必须是 RFC3339 timestamp",
    "human owner 只允许 human_input/destructive_boundary/repair_exhausted/execution_context",
    "initial pending publication 后不得已有 extension",
    "initial plan publication precheck 必须等于 goal baseline",
    "legacy agent presentation assertion 只允许绑定 owner=human",
    "legacy agent presentation assertion 必须使用稳定规范化形式",
    "legacy agent presentation assertion 必须绑定明确 goal_id",
    "legacy assertion channel 必须是长度不超过 128 的单行非控制字符文本",
    "legacy assertion reference 必须缺省或为长度不超过 2048 的单行非控制字符文本",
    "legacy assertion 未绑定 migration proof 中的旧 package hash",
    "legacy assertion 未绑定其 legacy stored solution package hash",
    "legacy migration goal 不匹配；拒绝改绑历史 package",
    "legacy migration proof 与当前 v2 package identity 不匹配",
    "legacy migration proof 只能附着在 v2 pending",
    "legacy migration proof 必须使用稳定规范化形式",
    "legacy migration proof 必须声明 from_contract_version=0",
    "legacy pending package hash 不匹配: expected={} supplied={}",
    "legacy pending {id} 缺少完整 solution package",
    "legacy plan chain hash 或单调扩展关系无效",
    "legacy plan chain 不得混入 v16 publication 节点或 intent",
    "legacy plan chain 只允许作为 rollout {PLAN_PUBLICATION_ROLLOUT_AT} 前产生且已退休的历史记录",
    "lifecycle_proof.recorded_at 不得早于 goal.updated_at",
    "lifecycle_proof.recorded_at 必须是 RFC3339 timestamp",
    "non-agent capability boundary 必须绑定 --goal",
    "package_sha256 必须使用小写规范化形式",
    "pending capability contract conflict: ({}, {}) 已由 {} 使用",
    "pending plan publication 不得携带 confirmed/committed 字段",
    "pending publication 与 persisted intent 不匹配",
    "pending publication 缺少 persisted intent",
    "pending render --current 发现损坏的 goal state: {}",
    "pending render --current 没有当前可咨询的 goal",
    "pending render 必须且只能指定 --goal <id> 或 --current",
    "pending render 没有当前可咨询的 v2 human solution package",
    "pending render 至少需要一个 goal",
    "pending {id} 已是当前或未知版本，拒绝 legacy migration",
    "pending {} stored package hash 已漂移",
    "pending 不存在: {id}",
    "pending.json (goal_id, capability_key) 重复: 第 {} 项（id={}）与第 {} 项（id={}）都声明了 ({}, {})",
    "plan chain 外层 hash、baseline 或单调扩展关系无效",
    "plan extension {} 时间顺序必须位于前一 publication 之后",
    "plan publication hash 或必需字段无效",
    "plan publication 时间顺序必须满足 goal <= baseline <= receipt <= published <= committed",
    "plan publication 未绑定 enclosing goal_id",
    "plan publication 未绑定对应 plan payload",
    "plan publish intent hash、goal、timestamp 或 baseline 绑定无效",
    "plan publish intent 缺少对应 pending plan 节点",
    "prepare 发现实际变更 {} 个文件但缺少首次修改前的 goal plan receipt: {}。prepare 不会事后补 plan；先将这些路径恢复到 goal baseline，再按 program/args 逐参数调用: {}",
    "prepare 发现未计划的实际变更: {}。prepare 不会自动扩展 plan；先将这些路径恢复到 goal baseline，再按 program/args 逐参数调用: {}",
    "prepare 最终重验时 goal 已不存在: {goal_id}",
    "prepare 期间源码发生变化，context index 与 goal delta 不属于同一快照: {}；请在源码稳定后重试",
    "prepare 核心验证后 workspace 或 goal 状态发生变化；snapshot readiness 已失效，请重试（workspace {} -> {}；goal {} -> {}）",
    "stored package_sha256 与 solution package 不匹配: stored={} expected={}",
    "v2 human/external pending 必须绑定 goal_id、capability_key 与 boundary_class",
    "v2 pending 必须携带 canonical package_sha256",
    "write_ahead_v1 extension {} 缺少 publication proof",
    "write_ahead_v1 plan receipt 缺少 publication proof",
    "{label} 不能为空",
    "{label} 必须是 1..=256 字节的小写稳定标识，仅允许 a-z、0-9、.、_、:、/、-，且首尾必须是字母或数字",
    "{label} 必须是 64 位十六进制 SHA-256",
    "{label} 必须是 RFC3339 timestamp",
    "不支持的 pending contract_version={}（当前只接受 legacy 0 或 v{}）",
    "已迁移 legacy pending {}。",
    "带 package_sha256 的 pending solution package 必须使用稳定规范化形式",
    "归档后的 lifecycle proof 无效: {error}",
    "归档后的目标合约无效: {error}",
    "当前 workspace snapshot 的文件清单与 fingerprint 不匹配",
    "无法从同一 no-follow 文件句柄读取 workspace_skill.yaml",
    "无法核对 goal plan: {error}",
    "未知 pending kind: {value}（可用: machine_actionable | human_input | external_wait | destructive_boundary | hard_gate | repair_exhausted | execution_context）",
    "未知 plan_publication_policy: {other}",
    "源码在 plan extension 发布 CAS 窗口内发生变化；已保留 fail-closed plan publish intent（precheck={} confirmed={}），恢复原快照后用同一扩展重试或退休该 goal",
    "源码在 plan 发布 CAS 窗口内发生变化；已保留 fail-closed plan publish intent（precheck={} confirmed={}），恢复原快照后用同一计划重试或退休该 goal",
    "目标 {} 不是当前 schema，不能作为 plan reconciliation authority",
    "目标 {} 缺少开工 baseline；不能核对实际变更，请用新的 baseline-bound goal supersede，或将已完成记录显式 archive",
    "缺少 baseline 的 goal 不得携带 plan publication state",
];

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum Language {
    /// Follow RAYMAN_LANG, the process locale, and finally the OS user locale.
    Auto,
    /// Simplified Chinese user interface.
    #[value(name = "zh-CN", alias = "zh", alias = "zh-cn", alias = "zh_CN")]
    ZhCn,
    /// English user interface.
    #[value(name = "en", alias = "en-US", alias = "en-us", alias = "en_US")]
    En,
}

impl std::fmt::Display for Language {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Auto => "auto",
            Self::ZhCn => "zh-CN",
            Self::En => "en",
        })
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum ActiveLanguage {
    ZhCn = 1,
    En = 2,
}

static ACTIVE_LANGUAGE: AtomicU8 = AtomicU8::new(ActiveLanguage::ZhCn as u8);
static JSON_OUTPUT: AtomicBool = AtomicBool::new(false);

pub fn configure(requested: Language, json_output: bool) -> bool {
    let resolved = resolve(requested);
    ACTIVE_LANGUAGE.store(resolved as u8, Ordering::Relaxed);
    JSON_OUTPUT.store(json_output, Ordering::Relaxed);
    json_output
}

pub fn preconfigure_from_process_args() {
    let arguments: Vec<String> = std::env::args_os()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    let mut requested = Language::Auto;
    for (index, argument) in arguments.iter().enumerate() {
        let value = if argument == "--language" || argument == "--lang" {
            arguments.get(index + 1).map(String::as_str)
        } else {
            argument
                .strip_prefix("--language=")
                .or_else(|| argument.strip_prefix("--lang="))
        };
        if let Some(value) = value {
            requested = match value.to_ascii_lowercase().as_str() {
                "en" | "en-us" | "en_us" => Language::En,
                "zh" | "zh-cn" | "zh_cn" => Language::ZhCn,
                _ => Language::Auto,
            };
            break;
        }
    }
    configure(requested, false);
}

pub fn localize_text(text: String) -> String {
    text.lines()
        .map(|line| localize_line(line.to_string()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn resolve(requested: Language) -> ActiveLanguage {
    match requested {
        Language::ZhCn => ActiveLanguage::ZhCn,
        Language::En => ActiveLanguage::En,
        Language::Auto => resolve_auto_language(),
    }
}

fn resolve_auto_language() -> ActiveLanguage {
    for variable in ["RAYMAN_LANG", "LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(variable)
            && let Some(language) = language_from_locale(&value)
        {
            return language;
        }
    }

    #[cfg(windows)]
    if let Some(locale) = windows_user_locale()
        && let Some(language) = language_from_locale(&locale)
    {
        return language;
    }

    // Chinese is the fail-safe default when a host exposes no locale metadata.
    ActiveLanguage::ZhCn
}

fn language_from_locale(locale: &str) -> Option<ActiveLanguage> {
    let normalized = locale.trim().replace('_', "-").to_ascii_lowercase();
    if normalized.is_empty() || normalized == "auto" {
        return None;
    }
    if normalized == "zh" || normalized.starts_with("zh-") {
        Some(ActiveLanguage::ZhCn)
    } else {
        // English is the deterministic fallback until another catalog exists.
        Some(ActiveLanguage::En)
    }
}

#[cfg(windows)]
fn windows_user_locale() -> Option<String> {
    const LOCALE_NAME_MAX_LENGTH: usize = 85;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserDefaultLocaleName(locale_name: *mut u16, locale_name_count: i32) -> i32;
    }

    let mut buffer = [0_u16; LOCALE_NAME_MAX_LENGTH];
    // SAFETY: the writable buffer and exact capacity are passed to the API.
    let count =
        unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), LOCALE_NAME_MAX_LENGTH as i32) };
    if count <= 1 || count as usize > buffer.len() {
        return None;
    }
    String::from_utf16(&buffer[..count as usize - 1]).ok()
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(usize)]
pub enum MessageId {
    CheckpointStatus,
    GoalCreated,
    HandoffCreated,
    HostPatchUnusable,
    HostPatchFix,
    Count,
}

#[derive(Copy, Clone)]
struct CatalogEntry {
    id: MessageId,
    zh_cn: &'static str,
    en: &'static str,
}

const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        id: MessageId::CheckpointStatus,
        zh_cn: "最近完整快照: {} ({}, 保存于 {})",
        en: "Latest complete checkpoint: {} ({}, saved at {})",
    },
    CatalogEntry {
        id: MessageId::GoalCreated,
        zh_cn: "已创建目标 {} ({} 个需求)",
        en: "Goal {} created ({} requirements)",
    },
    CatalogEntry {
        id: MessageId::HandoffCreated,
        zh_cn: "已创建交接目标 {}，来源 {}，commit {}",
        en: "Created handoff goal {} from {} at commit {}",
    },
    // Explicit pairs rather than fragment translation: this text names host
    // config keys and must stay byte-stable in both locales, because an agent
    // reads it to decide whether to stop retrying the host patch tool.
    CatalogEntry {
        id: MessageId::HostPatchUnusable,
        zh_cn: "  宿主补丁工具: 不可用（sandbox={}）；`unelevated` 沙箱无法表达分裂可写根、分裂读限制或 deny-read，内置 apply_patch 在读取目标文件前就被拒绝",
        en: "  Host patch tool: unusable (sandbox={}); the `unelevated` sandbox cannot express split writable roots, split filesystem reads, or deny-read, so the built-in apply_patch is refused before it reads the target file",
    },
    CatalogEntry {
        id: MessageId::HostPatchFix,
        zh_cn: "    修复: 把 Codex 配置改为 `[windows] sandbox = \"elevated\"` 并重启 Codex；在此之前用 `git apply` 从文件应用补丁，不要重试该工具",
        en: "    Fix: set `[windows] sandbox = \"elevated\"` in the Codex config and restart Codex; until then apply patches with `git apply` from a file and stop retrying the tool",
    },
];

const _: [(); MessageId::Count as usize] = [(); CATALOG.len()];

impl MessageId {
    fn entry(self) -> &'static CatalogEntry {
        &CATALOG[self as usize]
    }
}

fn active_language() -> ActiveLanguage {
    match ACTIVE_LANGUAGE.load(Ordering::Relaxed) {
        value if value == ActiveLanguage::En as u8 => ActiveLanguage::En,
        _ => ActiveLanguage::ZhCn,
    }
}

pub fn message(id: MessageId, arguments: &[String]) -> String {
    message_for(id, arguments, active_language())
}

fn message_for(id: MessageId, arguments: &[String], language: ActiveLanguage) -> String {
    let entry = id.entry();
    debug_assert_eq!(entry.id, id);
    let template = match language {
        ActiveLanguage::ZhCn => entry.zh_cn,
        ActiveLanguage::En => entry.en,
    };
    assert_eq!(
        template.matches("{}").count(),
        arguments.len(),
        "message catalog argument mismatch for {id:?}"
    );
    let mut rendered = template.to_string();
    for argument in arguments {
        rendered = rendered.replacen("{}", argument, 1);
    }
    rendered
}

pub fn localize_line(line: String) -> String {
    localize_line_for(line, active_language(), JSON_OUTPUT.load(Ordering::Relaxed))
}

fn localize_line_for(line: String, language: ActiveLanguage, json_output: bool) -> String {
    if json_output {
        return line;
    }

    let indentation_end = line
        .char_indices()
        .find_map(|(index, character)| (!character.is_whitespace()).then_some(index))
        .unwrap_or(line.len());
    let (indentation, content) = line.split_at(indentation_end);

    // A whole-line authored template is strictly more specific than a prefix
    // match: it accounts for every static part of the line, not just its head.
    // Trying the prefix first left each line's tail to fragment guessing, and a
    // prefix whose English already absorbed the tail's meaning then said it
    // twice — `handoff/CI verifies it with verification with ...` on every en
    // `doctor` run.
    if let Some(localized) = localize_authored_message(content, language) {
        return format!("{indentation}{localized}");
    }
    for &(chinese, english) in MESSAGE_PREFIX_CATALOG {
        let (source, target) = match language {
            ActiveLanguage::ZhCn => (english, chinese),
            ActiveLanguage::En => (chinese, english),
        };
        if let Some(remainder) = content.strip_prefix(source) {
            let remainder_indent_end = remainder
                .char_indices()
                .find_map(|(index, character)| (!character.is_whitespace()).then_some(index))
                .unwrap_or(remainder.len());
            let (remainder_indent, remainder_content) = remainder.split_at(remainder_indent_end);
            let localized_content = match localize_authored_message(remainder_content, language) {
                Some(localized) => localized,
                // No authored template matched, so nothing here is known to be
                // framework text; translate only what is provably static.
                None => localize_known_fragments(remainder_content.into(), language),
            };
            return format!("{indentation}{target}{remainder_indent}{localized_content}");
        }
    }
    // A line may carry a complete authored message after a label this build
    // prints in English already (`    BLOCKER: <authored message>`). Neither the
    // whole-line attempt nor the prefix catalog can reach it — the prefix
    // catalog matches Chinese heads, and the label is not one — so the payload
    // fell through to fragment guessing and stayed Chinese under en. Try the
    // text after each `: ` boundary as a whole authored message; anything that
    // does not match one is left exactly as it was.
    if language == ActiveLanguage::En {
        let mut search = 0usize;
        while let Some(offset) = content[search..].find(": ") {
            let start = search + offset + ": ".len();
            if let Some(localized) = localize_authored_message(&content[start..], language) {
                return format!("{indentation}{}{localized}", &content[..start]);
            }
            search = start;
        }
    }
    localize_known_fragments(line, language)
}

#[derive(Debug)]
struct ParsedTemplate {
    statics: Vec<String>,
    placeholders: Vec<String>,
}

fn parse_format_template(template: &str) -> Option<ParsedTemplate> {
    let mut statics = vec![String::new()];
    let mut placeholders = Vec::new();
    let characters = template.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < characters.len() {
        match characters[index] {
            '{' if characters.get(index + 1) == Some(&'{') => {
                statics.last_mut()?.push('{');
                index += 2;
            }
            '}' if characters.get(index + 1) == Some(&'}') => {
                statics.last_mut()?.push('}');
                index += 2;
            }
            '{' => {
                let start = index + 1;
                index = start;
                while index < characters.len() && characters[index] != '}' {
                    index += 1;
                }
                if index == characters.len() {
                    return None;
                }
                placeholders.push(characters[start..index].iter().collect());
                statics.push(String::new());
                index += 1;
            }
            character => {
                statics.last_mut()?.push(character);
                index += 1;
            }
        }
    }
    Some(ParsedTemplate {
        statics,
        placeholders,
    })
}

fn match_authored_template(template: &str, rendered: &str) -> Option<Vec<String>> {
    let parsed = parse_format_template(template)?;
    let mut remainder = rendered.strip_prefix(parsed.statics.first()?)?;
    let mut captures = Vec::with_capacity(parsed.placeholders.len());
    for (index, next_static) in parsed.statics.iter().skip(1).enumerate() {
        if next_static.is_empty() {
            if index + 1 == parsed.placeholders.len() {
                captures.push(remainder.to_string());
                remainder = "";
            } else {
                captures.push(String::new());
            }
        } else {
            let boundary = remainder.find(next_static)?;
            captures.push(remainder[..boundary].to_string());
            remainder = &remainder[boundary + next_static.len()..];
        }
    }
    remainder.is_empty().then_some(captures)
}

fn render_translated_template(template: &str, captures: &[String]) -> Option<String> {
    let parsed = parse_format_template(template)?;
    if parsed.placeholders.len() != captures.len() {
        return None;
    }
    let mut rendered = parsed.statics.first()?.clone();
    for (index, capture) in captures.iter().enumerate() {
        rendered.push_str(capture);
        rendered.push_str(&parsed.statics[index + 1]);
    }
    Some(rendered)
}

fn translate_authored_template(template: &str) -> String {
    static SORTED_TRANSLATIONS: std::sync::OnceLock<Vec<(&'static str, &'static str)>> =
        std::sync::OnceLock::new();
    let translations = SORTED_TRANSLATIONS.get_or_init(|| {
        let mut translations = MESSAGE_PREFIX_CATALOG
            .iter()
            .chain(MESSAGE_FRAGMENT_CATALOG)
            .chain(TEMPLATE_FRAGMENT_CATALOG)
            .copied()
            .collect::<Vec<_>>();
        translations.sort_by(|left, right| {
            right
                .0
                .chars()
                .count()
                .cmp(&left.0.chars().count())
                .then(left.0.cmp(right.0))
        });
        translations
    });
    let mut translated = template.to_string();
    for &(chinese, english) in translations {
        translated = translated.replace(chinese, english);
    }
    translated
}

fn localize_authored_message(content: &str, language: ActiveLanguage) -> Option<String> {
    localize_authored_message_within(content, language, 0)
}

fn localize_authored_capture(
    capture: &str,
    language: ActiveLanguage,
    depth: usize,
) -> Option<String> {
    let segments = capture.split("; ").collect::<Vec<_>>();
    if segments.len() > 1
        && let Some(localized) = segments
            .iter()
            .map(|segment| localize_authored_message_within(segment, language, depth))
            .collect::<Option<Vec<_>>>()
    {
        return Some(localized.join("; "));
    }
    localize_authored_message_within(capture, language, depth)
}

/// Authored messages compose: `doctor` and `check` pass other framework-authored
/// Chinese (a toolchain state, a required_for reason, a blocker) *through* a
/// placeholder. Reinserting captures byte-for-byte therefore left framework text
/// untranslated in en output even though every individual message is covered by
/// the catalog and the coverage test passes.
///
/// A capture is re-localized only when it matches an authored template in full.
/// Framework gap lists are split only when every `; `-separated segment matches,
/// so user content (goal titles, requirement text, paths) is still reinserted
/// verbatim. `depth` bounds the recursion in case a template ever degenerates to
/// a capture as long as its input.
fn localize_authored_message_within(
    content: &str,
    language: ActiveLanguage,
    depth: usize,
) -> Option<String> {
    const MAX_COMPOSED_DEPTH: usize = 4;
    if language != ActiveLanguage::En || !contains_han_text(content) {
        return None;
    }
    let mut best: Option<(usize, String)> = None;
    for template in AUTHORED_MESSAGE_TEMPLATES {
        // Callers hand us the line with its indentation already split off, but
        // authored templates keep the leading spaces of the source literal
        // (`"  仓库源码产物: …"`). Matching the raw template therefore never fired
        // for any indented message, which silently routed all of them to prefix
        // + fragment guessing. Indentation is re-attached by the caller.
        let template = template.trim_start();
        let Some(captures) = match_authored_template(template, content) else {
            continue;
        };
        let translated = translate_authored_template(template);
        if contains_han_text(&translated) {
            continue;
        }
        let captures = if depth >= MAX_COMPOSED_DEPTH {
            captures
        } else {
            captures
                .iter()
                .map(|capture| {
                    localize_authored_capture(capture, language, depth + 1)
                        .unwrap_or_else(|| capture.clone())
                })
                .collect()
        };
        let Some(rendered) = render_translated_template(&translated, &captures) else {
            continue;
        };
        let specificity = parse_format_template(template)?
            .statics
            .iter()
            .map(|part| part.chars().count())
            .sum();
        if best
            .as_ref()
            .is_none_or(|(best_specificity, _)| specificity > *best_specificity)
        {
            best = Some((specificity, rendered));
        }
    }
    best.map(|(_, rendered)| rendered)
}

fn contains_han_text(text: &str) -> bool {
    text.chars().any(is_han)
}

// Prefixes are anchored after indentation, and authored templates reinsert their
// captured dynamic values byte-for-byte. Known-fragment translation is skipped for
// any fragment embedded inside an ideographic (Han) word (see
// replace_fragment_outside_han_words), so goal titles, requirement text, and Unicode
// paths are preserved rather than partially translated.
const MESSAGE_PREFIX_CATALOG: &[(&str, &str)] = &[
    ("已安装身份契约:", "Installed identity contract:"),
    ("当前二进制:", "Running binary:"),
    ("PATH 命令一致:", "PATH command matches:"),
    (
        "仓库源码产物: 未由 doctor 检查；交接/CI 由",
        "Repository source artifact: not checked by doctor; handoff/CI verifies it with",
    ),
    ("workspace SKILL 一致:", "Workspace SKILL matches:"),
    ("已安装身份 READY:", "Installed identity READY:"),
    (
        "源码新鲜度: 未由 doctor 证明；交接/CI 必须运行",
        "Source freshness: not proven by doctor; handoff/CI must run",
    ),
    (
        "当前工作区暂无快照。运行 `rayman checkpoint save` 创建一个。",
        "No workspace checkpoint exists. Run `rayman checkpoint save` to create one.",
    ),
    (
        "资产扫描: 干净（无过时候选、无未完成标记）。",
        "Asset scan: clean (no stale candidates or work-in-progress markers).",
    ),
    (
        "发布交接状态: 未检查（本结果仅是工作区 strict-quality）",
        "Release handoff: not checked (workspace strict-quality only)",
    ),
    (
        "运行 `rayman context refresh` 更新索引。",
        "Run `rayman context refresh` to update the index.",
    ),
    ("当前工作区暂无快照。", "No workspace checkpoint exists."),
    (
        "无托管临时目录可清理。",
        "No managed temp directory to clean.",
    ),
    ("已清理托管临时目录。", "Managed temp directory cleaned."),
    ("无待完成项。", "No pending items."),
    ("暂无 current 目标。", "No current goals."),
    ("暂无目标。", "No goals."),
    (
        "过时资产候选（提示，不自动删除）:",
        "Stale asset candidates (advisory; never auto-deleted):",
    ),
    (
        "候选相关测试(启发式):",
        "Candidate related tests (heuristic):",
    ),
    (
        "RaymanCodingSkill 工作区激活:",
        "RaymanCodingSkill workspace activation:",
    ),
    ("工作区就绪:", "Workspace readiness:"),
    ("任务准备完成:", "Task preparation complete:"),
    ("上下文索引:", "Context index:"),
    ("索引已刷新:", "Index refreshed:"),
    ("托管临时目录:", "Managed temp directory:"),
    ("受管状态审计:", "Managed state audit:"),
    ("项目地图已刷新:", "Project map refreshed:"),
    ("快照（旧→新）:", "Checkpoints (oldest to newest):"),
    ("最近完整快照:", "Latest complete checkpoint:"),
    ("已创建目标", "Goal created"),
    ("已记录待完成项", "Pending item recorded"),
    ("已解决待完成项。", "Pending item resolved."),
    ("待完成项:", "Pending items:"),
    ("资产扫描:", "Asset scan:"),
    ("未完成标记:", "Work-in-progress markers:"),
    ("项目地图:", "Project map:"),
    ("文件:", "File:"),
    ("符号匹配:", "Symbol matches:"),
    ("未找到符号:", "Symbol not found:"),
    ("影响分析:", "Impact analysis:"),
    ("变更计划:", "Change plan:"),
    ("项目质量(", "Project quality("),
    ("建议验证:", "Recommended validation:"),
    ("建议依据:", "Recommendation basis:"),
    ("文件分组:", "File groups:"),
    ("风险提示:", "Risk summary:"),
    ("风险:", "Risks:"),
    ("符号:", "Symbols:"),
    ("错误:", "Error:"),
    ("问题:", "Issue:"),
    ("源码错误:", "Source error:"),
    ("源码:", "Source:"),
    ("激活:", "Activation:"),
    ("命令:", "Command:"),
    ("阻断:", "BLOCKER:"),
    ("任务阻断:", "TASK BLOCKER:"),
    ("警告:", "Warning:"),
    ("包:", "Package:"),
    (
        "nextest 暂无独立 list proof 支持；请使用 `cargo test` 生成 receipt",
        "nextest does not yet support independent list proof; use `cargo test` to generate the receipt",
    ),
    (
        "`--non-code` 不能与 `--changed` 同时使用",
        "`--non-code` cannot be used together with `--changed`",
    ),
    (
        "`--workspace-snapshot` 不能与 `--changed` 或 `--non-code` 同时使用",
        "`--workspace-snapshot` cannot be used together with `--changed` or `--non-code`",
    ),
    (
        "validation 必须提供至少一个 `--changed`；非代码需求必须显式使用 `--non-code`；零变更 authority 审计使用 `--workspace-snapshot`",
        "validation must provide at least one `--changed`; non-code requirements must explicitly use `--non-code`; use `--workspace-snapshot` for a zero-delta authority audit",
    ),
    (
        "--workspace-snapshot 只允许与 --authority 一起使用",
        "--workspace-snapshot can only be used together with --authority",
    ),
    (
        "--workspace-snapshot 要求 goal baseline delta 为空；发现真实变更: {}。验证命令尚未执行",
        "--workspace-snapshot requires an empty goal baseline delta; real changes found: {}. The validation command was not executed",
    ),
    (
        "workspace snapshot receipt 必须是 authority receipt",
        "a workspace snapshot receipt must be an authority receipt",
    ),
    (
        "workspace snapshot receipt 要求 goal baseline delta 为空；发现真实变更: {}",
        "a workspace snapshot receipt requires an empty goal baseline delta; real changes found: {}",
    ),
    (
        "受管状态包含退役条目或遍历错误；先审阅 `rayman state audit` 输出",
        "managed state contains retired entries or traversal errors; review `rayman state audit` output first",
    ),
    (
        "`--changed` 证据必须同时提供至少一个 `--validated <command>`，避免把影响面建议误当作已验证事实。",
        "`--changed` evidence must also provide at least one `--validated <command>` so impact suggestions are not mistaken for verified facts.",
    ),
    (
        "无法读取 autosave 锁句柄: {}",
        "unable to read autosave lock handle: {}",
    ),
    (
        "autosave 锁句柄不是普通文件: {}",
        "autosave lock handle is not a regular file: {}",
    ),
    (
        "已存初始快照 {}（{} 个文件）并注册计划任务 '{}'：每 {} 分钟自动快照{}。",
        "saved initial checkpoint {} ({} files) and registered scheduled task '{}': checkpoint every {} minutes{}.",
    ),
    (
        "停止状态写入失败；计划任务已重新注册，autosave 保持 active：{state_error}",
        "stop-state write failed; scheduled task was re-registered and autosave remains active: {state_error}",
    ),
    (
        "自动保存状态损坏或不可读取；未修改状态，也未注销计划任务：{error}",
        "autosave state is corrupt or unreadable; state and scheduled task were left unchanged: {error}",
    ),
    (
        "checkpoint 锁被替换为非普通文件: {}",
        "checkpoint lock was replaced by a non-regular file: {}",
    ),
    (
        "无法确定用户数据目录（未设置 LOCALAPPDATA/XDG_DATA_HOME/HOME/USERPROFILE），请用 --dir 指定",
        "unable to determine the user data directory (LOCALAPPDATA/XDG_DATA_HOME/HOME/USERPROFILE are unset); specify it with --dir",
    ),
    (
        "checkpoint 保存不完整（{} 个错误）；已保留 partial 快照 {} 供取证，不会替代最近完整快照{}",
        "checkpoint save is incomplete ({} errors); partial checkpoint {} was retained for forensics and will not replace the latest complete checkpoint{}",
    ),
    (
        "实际变更 {} 个文件但缺少首次修改前的 goal plan receipt",
        "{} files changed but the goal lacks a plan receipt recorded before the first modification",
    ),
    (
        "Cargo 拓扑权威确认（standard/release 就绪的硬前提）",
        "authoritative Cargo topology (a hard precondition for standard/release readiness)",
    ),
    (
        "autosave 计划任务注册与注销",
        "autosave scheduled-task registration and removal",
    ),
    (
        "{TOPOLOGY_TOOL_UNAVAILABLE}: cargo 不在本进程 PATH 中",
        "{TOPOLOGY_TOOL_UNAVAILABLE}: cargo is not on this process PATH",
    ),
    (
        "{name} 不在本进程 PATH 中：安装器/工具链只改持久化 PATH，已经开着的终端不会继承；新开一个终端，或先把它的安装目录加进本进程 PATH",
        "{name} is not on this process PATH: installers and toolchains only update the persistent PATH, which an already open terminal never inherits; open a new terminal, or prepend its install directory to this process PATH",
    ),
    ("不可达（{}）", "unreachable ({})"),
    (
        "不可达（本工作区不需要）",
        "unreachable (not needed by this workspace)",
    ),
    ("已找到", "found"),
    (
        "源码状态、跟踪文件枚举与 clean-worktree 判定",
        "source state, tracked-file enumeration, and clean-worktree decisions",
    ),
    (
        "环境未就绪: {}；无法确认 Cargo 拓扑",
        "environment is not ready: {}; Cargo topology cannot be confirmed",
    ),
    (
        "验证命令不能启动 shell；PowerShell 脚本请用 `pwsh -NoProfile -File <script>.ps1 [参数...]` 这一种形式",
        "a validation command must not launch a shell; record a PowerShell script only as `pwsh -NoProfile -File <script>.ps1 [args...]`",
    ),
    (
        "已安装身份契约不一致：{}",
        "installed identity contract is inconsistent: {}",
    ),
    (
        "PATH 上找不到 rayman：安装器只改持久化 PATH，已经开着的终端不会继承；新开一个终端，或先把安装目录加进本进程 PATH",
        "rayman is not on PATH: the installer only updates the persistent PATH, which an already open terminal never inherits; open a new terminal, or prepend the install directory to this process PATH",
    ),
    (
        "PATH 上的 rayman 与当前运行的二进制不是同一份：用仓库 release 二进制重新安装",
        "the rayman on PATH is not the binary that is running: reinstall from the repository release binary",
    ),
    (
        "workspace 未激活：运行 `rayman workspace activate --skill-file <canonical-SKILL.md> --yes`",
        "workspace is not activated: run `rayman workspace activate --skill-file <canonical-SKILL.md> --yes`",
    ),
    (
        "workspace SKILL.md 与记录的 skill_sha256 不一致：SKILL.md 改动后需重新 activate 重绑",
        "workspace SKILL.md does not match the recorded skill_sha256: re-run activate to rebind after editing SKILL.md",
    ),
    (
        "只有 success/partial/blocked goal 可以 archived",
        "only success, partial, or blocked goals can be archived",
    ),
    (
        "active goal 不能直接归档；先 `rayman goal close {id} --status partial`（或 blocked）如实记录结果，再归档",
        "an active goal cannot be archived directly; first record the real outcome with `rayman goal close {id} --status partial` (or blocked), then archive it",
    ),
    (
        "legacy goal {} 仍为 current（status={}）；legacy 记录不能生成当前 receipt，请显式 archive 历史 success，或新建 current-schema replacement 后 supersede",
        "legacy goal {} remains current (status={}); legacy records cannot produce current receipts, so explicitly archive the historical success or create a current-schema replacement and then supersede it",
    ),
    (
        "只有 success goal 可以 archived",
        "only successful goals can be archived",
    ),
    (
        "superseded_by archived 目标 {replacement_id} 是 untrusted history quarantine，不能作为完成证明",
        "superseded_by archived goal {replacement_id} is an untrusted history quarantine and cannot prove completion",
    ),
    (
        "lifecycle_proof 使用了无效的 receipt integrity quarantine",
        "lifecycle_proof uses an invalid receipt-integrity quarantine",
    ),
    (
        "未证明完成的 goal，其 must 未完整转移到 replacement: {}",
        "the unproven goal's must requirements were not fully transferred to the replacement: {}",
    ),
    (
        "verified replacement transfer 只允许无额外 migration 的 superseded current-schema success",
        "verified replacement transfer allows only a superseded current-schema success without an additional migration",
    ),
    (
        "lane {} delta_paths 未规范化",
        "lane {} delta_paths are not normalized",
    ),
    (
        "maintenance cycle rebind 要求 archived command 恰好包含一个 -MaintenanceOrchestrationCycle",
        "maintenance-cycle rebind requires the archived command to contain exactly one -MaintenanceOrchestrationCycle",
    ),
    (
        "maintenance cycle rebind 路径必须是使用 / 的非空 workspace-relative 路径",
        "maintenance-cycle rebind path must be a non-empty workspace-relative path using /",
    ),
    (
        "maintenance cycle rebind 路径禁止 absolute、.、..、prefix 或 root component",
        "maintenance-cycle rebind path forbids absolute, ., .., prefix, or root components",
    ),
    (
        "maintenance cycle rebind 路径必须是唯一规范文本形式",
        "maintenance-cycle rebind path must use its unique normalized textual form",
    ),
    (
        "maintenance cycle rebind 路径不得经过 symlink/junction/reparse: {}",
        "maintenance-cycle rebind path must not traverse a symlink/junction/reparse point: {}",
    ),
    (
        "maintenance cycle rebind 目标逃逸 workspace: {}",
        "maintenance-cycle rebind target escapes the workspace: {}",
    ),
    (
        "archived -MaintenanceOrchestrationCycle 不是 cycle-qualified JSON 路径",
        "archived -MaintenanceOrchestrationCycle is not a cycle-qualified JSON path",
    ),
    (
        "maintenance cycle rebind 结构无效",
        "maintenance-cycle rebind structure is invalid",
    ),
    (
        "maintenance cycle rebind 未精确绑定 archived command 的 flag/value",
        "maintenance-cycle rebind does not exactly bind the archived command's flag/value",
    ),
    (
        "maintenance cycle rebind 文件 hash 已漂移",
        "maintenance-cycle rebind file hash has drifted",
    ),
    (
        "pytest 成功退出但没有可验证的 passed>0 汇总；不会写入 receipt",
        "pytest succeeded but has no verifiable passed>0 summary; receipt will not be recorded",
    ),
    (
        "测试命令成功退出但没有可验证的 passed>0 汇总；不会写入 receipt",
        "test command succeeded but has no verifiable passed>0 summary; receipt will not be recorded",
    ),
    (
        "validation 不覆盖 {}；需要同一条当前成功 receipt 绑定 {}",
        "validation does not cover {}; the same current successful receipt must bind {}",
    ),
    (
        "goal 状态为 {}，不是 success",
        "goal status is {}, not success",
    ),
    (
        "goal {id} lane {lane_id} 已打开（mode={mode:?}）",
        "goal {id} lane {lane_id} opened (mode={mode:?})",
    ),
    (
        "progress 命令修改了源码快照；不会写入 receipt",
        "progress command modified the source snapshot; receipt will not be recorded",
    ),
    (
        "pytest lease {} 已创建并通过读写探针",
        "pytest lease {} was created and passed the read/write probe",
    ),
    ("pytest lease {id} 已释放", "pytest lease {id} was released"),
    (
        "--authority 要求 --repeat >= 2，以证明稳定固定点",
        "--authority requires --repeat >= 2 to prove a stable fixed point",
    ),
    (
        "任务门禁要求 ready context；使用 --refresh-context 或 prepare/finish",
        "the task gate requires ready context; use --refresh-context or prepare/finish",
    ),
    (
        "prepare 要求 current active goal；{} 当前 lifecycle={} status={}",
        "prepare requires a current active goal; {} currently has lifecycle={} status={}",
    ),
    (
        "pytest lease manifest 与受管路径不一致: {id}",
        "pytest lease manifest does not match its managed path: {id}",
    ),
    (
        "共享 quality policy 的父目录必须是工作区内真实目录，不能是链接/reparse: {}",
        "the shared quality policy parent must be a real workspace directory, not linked/reparse: {}",
    ),
    ("enabled 不是 true", "enabled is not true"),
    ("阻断项:", "Blockers:"),
    ("警告项:", "Warnings:"),
    ("按角色统计:", "Findings by role:"),
    ("未删除任何文件", "No files were deleted"),
];
// Fixed fragments cover structured lines whose dynamic identifiers or counts are
// formatted by the producing module. Only known source phrases are replaced;
// arbitrary Unicode goal titles and paths are never normalized.
// This glossary is applied only to authored format templates after dynamic fields
// have been extracted. Short entries are therefore safe: user titles, paths, and
// evidence text are reinserted byte-for-byte after the static template is translated.
const TEMPLATE_FRAGMENT_CATALOG: &[(&str, &str)] = &[
    (
        "lifecycle-only authority {label} 不得晚于 replacement_authority.recorded_at",
        "lifecycle-only authority {label} must not be later than replacement_authority.recorded_at",
    ),
    (
        "lifecycle-only replacement authority proof 无效: {error}",
        "lifecycle-only replacement authority proof is invalid: {error}",
    ),
    (
        "replacement_authority.recorded_at 不得晚于 goal.updated_at",
        "replacement_authority.recorded_at must not be later than goal.updated_at",
    ),
    (
        "{label} 不得晚于 replacement_authority.recorded_at",
        "{label} must not be later than replacement_authority.recorded_at",
    ),
    (
        "目标 {} 的 supersession 不得早于 replacement_authority.recorded_at",
        "goal {} supersession must not be earlier than replacement_authority.recorded_at",
    ),
    (
        "被转移目标 {id} 的 supersession proof 无效: {error}",
        "transferred goal {id} has an invalid supersession proof: {error}",
    ),
    (
        "被转移目标 {id} 的 supersession 不得早于 replacement_authority.recorded_at",
        "transferred goal {id} supersession must not be earlier than replacement_authority.recorded_at",
    ),
    (
        "被转移目标 {id} 缺少 supersession proof",
        "transferred goal {id} is missing its supersession proof",
    ),
    (
        "legacy success migration 不能修复当前 command/plan/review 缺口: {}",
        "legacy success migration cannot repair current command/plan/review gaps: {}",
    ),
    (
        "legacy plan 时间顺序必须满足 goal <= baseline <= receipt <= extensions <= updated",
        "legacy plan chronology must satisfy goal <= baseline <= receipt <= extensions <= updated",
    ),
    (
        "目标 {} 不满足 retiring legacy-success plan reconciliation 条件",
        "goal {} does not satisfy the retiring legacy-success plan reconciliation conditions",
    ),
    (
        "未知 pending owner: {value}（可用: agent | human | external）",
        "unknown pending owner: {value} (available: agent | human | external)",
    ),
    (
        "未知 pending kind: {value}（可用: machine_actionable | human_input | external_wait | destructive_boundary | hard_gate | repair_exhausted | execution_context）",
        "unknown pending kind: {value} (available: machine_actionable | human_input | external_wait | destructive_boundary | hard_gate | repair_exhausted | execution_context)",
    ),
    (
        "pending.json 第 {} 项（id={}）合同无效: {error}",
        "pending.json item {} (id={}) has an invalid contract: {error}",
    ),
    (
        "pending title 与 detail 都不能为空",
        "pending title and detail must both be non-empty",
    ),
    (
        "capability-bound pending 必须绑定 --goal",
        "capability-bound pending must be bound to --goal",
    ),
    (
        "non-agent capability boundary 必须绑定 --goal",
        "non-agent capability boundary must be bound to --goal",
    ),
    ("pending 不存在: {id}", "pending does not exist: {id}"),
    (
        "legacy pending {id} 缺少完整 solution package",
        "legacy pending {id} is missing a complete solution package",
    ),
    (
        "legacy pending package hash 不匹配: expected={} supplied={}",
        "legacy pending package hash mismatch: expected={} supplied={}",
    ),
    (
        "pending capability contract conflict: ({}, {}) 已由 {} 使用",
        "pending capability contract conflict: ({}, {}) is already used by {}",
    ),
    (
        "pending render 至少需要一个 goal",
        "pending render requires at least one goal",
    ),
    (
        "human owner 只允许 human_input/destructive_boundary/repair_exhausted/execution_context",
        "human owner only allows human_input/destructive_boundary/repair_exhausted/execution_context",
    ),
    (
        "不支持的 pending contract_version={}（当前只接受 legacy 0 或 v{}）",
        "unsupported pending contract_version={} (only legacy 0 or v{} is currently accepted)",
    ),
    (
        "capability-bound pending 必须绑定 goal_id",
        "capability-bound pending must be bound to goal_id",
    ),
    (
        "v2 human/external pending 必须绑定 goal_id、capability_key 与 boundary_class",
        "v2 human/external pending must be bound to goal_id, capability_key, and boundary_class",
    ),
    (
        "stored package_sha256 与 solution package 不匹配: stored={} expected={}",
        "stored package_sha256 does not match the solution package: stored={} expected={}",
    ),
    (
        "legacy migration proof 必须声明 from_contract_version=0",
        "legacy migration proof must declare from_contract_version=0",
    ),
    (
        "legacy migration proof 与当前 v2 package identity 不匹配",
        "legacy migration proof does not match the current v2 package identity",
    ),
    (
        "legacy agent presentation assertion 只允许绑定 owner=human",
        "legacy agent presentation assertion may only be bound to owner=human",
    ),
    ("{label} 不能为空", "{label} must not be empty"),
    (
        "{label} 必须是 RFC3339 timestamp",
        "{label} must be an RFC3339 timestamp",
    ),
    (
        "pending 绑定的 goal 不存在: {goal_id}",
        "pending-bound goal does not exist: {goal_id}",
    ),
    (
        "    BLOCKER: pending.json 不可读取: {error}",
        "    BLOCKER: pending.json is unreadable: {error}",
    ),
    (
        "goal.updated_at 必须是 RFC3339 timestamp",
        "goal.updated_at must be an RFC3339 timestamp",
    ),
    (
        "lifecycle_proof.recorded_at 必须是 RFC3339 timestamp",
        "lifecycle_proof.recorded_at must be an RFC3339 timestamp",
    ),
    (
        "pending publication 与 persisted intent 不匹配",
        "pending publication does not match the persisted intent",
    ),
    (
        "pending publication 缺少 persisted intent",
        "pending publication is missing the persisted intent",
    ),
    (
        "plan publish intent hash、goal、timestamp 或 baseline 绑定无效",
        "plan publish intent hash, goal, timestamp, or baseline binding is invalid",
    ),
    (
        "write_ahead_v1 extension {} 缺少 publication proof",
        "write_ahead_v1 extension {} is missing publication proof",
    ),
    (
        "write_ahead_v1 plan receipt 缺少 publication proof",
        "write_ahead_v1 plan receipt is missing publication proof",
    ),
    (
        "当前 workspace snapshot 的文件清单与 fingerprint 不匹配",
        "the current workspace snapshot's file list does not match its fingerprint",
    ),
    (
        "未知 plan_publication_policy: {other}",
        "unknown plan_publication_policy: {other}",
    ),
    (
        "execution-context requirement 未满足（仅约束本次 doctor 检查，不构成提权或 ACL 授权）：{}",
        "execution-context requirement was not met (this applies only to the current doctor check and does not grant elevation or ACL authorization): {}",
    ),
    (
        "目标 {} 不是当前 schema，不能作为 plan reconciliation authority",
        "goal {} is not on the current schema and cannot serve as plan reconciliation authority",
    ),
    (
        "目标 {} 缺少开工 baseline；不能核对实际变更，请用新的 baseline-bound goal supersede，或将已完成记录显式 archive",
        "goal {} is missing its starting baseline; actual changes cannot be reconciled; use a new baseline-bound goal to supersede it, or explicitly archive the completed record",
    ),
    (
        "无法核对 goal plan: {error}",
        "unable to reconcile goal plan: {error}",
    ),
    (
        "goal 存在未完成的 plan publish intent（kind={:?} intent_sha256={}）；源码可能在计划发布窗口内漂移，必须恢复原快照后重试或退休该 goal",
        "goal has an unfinished plan publish intent (kind={:?} intent_sha256={}); source may have drifted during the plan publication window; restore the original snapshot and retry or retire the goal",
    ),
    (
        "lifecycle_proof.recorded_at 不得早于 goal.updated_at",
        "lifecycle_proof.recorded_at must not precede goal.updated_at",
    ),
    (
        "pending.json (goal_id, capability_key) 重复: 第 {} 项（id={}）与第 {} 项（id={}）都声明了 ({}, {})",
        "pending.json contains a duplicate (goal_id, capability_key): item {} (id={}) and item {} (id={}) both declare ({}, {})",
    ),
    (
        "pending {id} 已是当前或未知版本，拒绝 legacy migration",
        "pending {id} is already current or has an unknown version; legacy migration is refused",
    ),
    (
        "agent pending 不需要 capability-bound legacy migration",
        "agent-owned pending packages do not need capability-bound legacy migration",
    ),
    (
        "legacy migration goal 不匹配；拒绝改绑历史 package",
        "legacy migration goal does not match; refusing to rebind the historical package",
    ),
    (
        "goal {} 当前不能 render human-boundary aggregate: decision={:?} consultation={:?} reason={}",
        "goal {} cannot currently render a human-boundary aggregate: decision={:?} consultation={:?} reason={}",
    ),
    (
        "pending render 没有当前可咨询的 v2 human solution package",
        "pending render found no currently consultable v2 human solution package",
    ),
    (
        "pending {} stored package hash 已漂移",
        "pending {} stored package hash has drifted",
    ),
    (
        "{label} 必须是 1..=256 字节的小写稳定标识，仅允许 a-z、0-9、.、_、:、/、-，且首尾必须是字母或数字",
        "{label} must be a stable lowercase identifier of 1..=256 bytes, using only a-z, 0-9, ., _, :, /, or -, and must begin and end with a letter or digit",
    ),
    (
        "{label} 必须是 64 位十六进制 SHA-256",
        "{label} must be a 64-character hexadecimal SHA-256",
    ),
    (
        "capability_key 与 boundary_class 必须使用稳定规范化形式",
        "capability_key and boundary_class must use stable normalized forms",
    ),
    (
        "capability_key 与 boundary_class 必须同时存在或同时缺省",
        "capability_key and boundary_class must either both be present or both be absent",
    ),
    (
        "v2 pending 必须携带 canonical package_sha256",
        "v2 pending must carry the canonical package_sha256",
    ),
    (
        "package_sha256 必须使用小写规范化形式",
        "package_sha256 must use lowercase normalized form",
    ),
    (
        "带 package_sha256 的 pending solution package 必须使用稳定规范化形式",
        "a pending solution package with package_sha256 must use stable normalized form",
    ),
    (
        "legacy migration proof 只能附着在 v2 pending",
        "legacy migration proof may be attached only to a v2 pending package",
    ),
    (
        "legacy migration proof 必须使用稳定规范化形式",
        "legacy migration proof must use stable normalized form",
    ),
    (
        "legacy agent presentation assertion 必须绑定明确 goal_id",
        "legacy agent presentation assertion must bind an explicit goal_id",
    ),
    (
        "legacy agent presentation assertion 必须使用稳定规范化形式",
        "legacy agent presentation assertion must use stable normalized form",
    ),
    (
        "legacy assertion channel 必须是长度不超过 128 的单行非控制字符文本",
        "legacy assertion channel must be single-line, control-character-free text no longer than 128 characters",
    ),
    (
        "legacy assertion reference 必须缺省或为长度不超过 2048 的单行非控制字符文本",
        "legacy assertion reference must be absent or single-line, control-character-free text no longer than 2048 characters",
    ),
    (
        "legacy assertion 未绑定其 legacy stored solution package hash",
        "legacy assertion is not bound to its legacy stored solution package hash",
    ),
    (
        "legacy assertion 未绑定 migration proof 中的旧 package hash",
        "legacy assertion is not bound to the old package hash in the migration proof",
    ),
    (
        "plan publication 未绑定 enclosing goal_id",
        "plan publication is not bound to its enclosing goal_id",
    ),
    (
        "plan publication hash 或必需字段无效",
        "plan publication hash or required fields are invalid",
    ),
    (
        "pending plan publication 不得携带 confirmed/committed 字段",
        "pending plan publication must not carry confirmed/committed fields",
    ),
    (
        "committed plan publication 必须证明 confirmed==precheck 并记录 committed_at",
        "committed plan publication must prove confirmed==precheck and record committed_at",
    ),
    (
        "committed plan publication 的 committed_at 必须是 RFC3339 且不早于 published_at",
        "committed plan publication committed_at must be RFC3339 and must not precede published_at",
    ),
    (
        "plan publication 未绑定对应 plan payload",
        "plan publication is not bound to its corresponding plan payload",
    ),
    (
        "缺少 baseline 的 goal 不得携带 plan publication state",
        "a goal without a baseline must not carry plan publication state",
    ),
    (
        "legacy plan chain 只允许作为 rollout {PLAN_PUBLICATION_ROLLOUT_AT} 前产生且已退休的历史记录",
        "legacy plan chain is permitted only for a retired historical record created before rollout {PLAN_PUBLICATION_ROLLOUT_AT}",
    ),
    (
        "legacy plan chain 不得混入 v16 publication 节点或 intent",
        "legacy plan chain must not contain v16 publication nodes or intents",
    ),
    (
        "legacy plan chain hash 或单调扩展关系无效",
        "legacy plan chain hash or monotonic extension relationship is invalid",
    ),
    (
        "plan publish intent 缺少对应 pending plan 节点",
        "plan publish intent is missing its corresponding pending plan node",
    ),
    (
        "plan chain 外层 hash、baseline 或单调扩展关系无效",
        "plan chain outer hash, baseline, or monotonic extension relationship is invalid",
    ),
    (
        "initial pending publication 后不得已有 extension",
        "initial pending publication must not already have an extension",
    ),
    (
        "initial plan publication precheck 必须等于 goal baseline",
        "initial plan publication precheck must equal the goal baseline",
    ),
    (
        "extension intent 缺少 pending 链尾",
        "extension intent is missing the pending chain tail",
    ),
    (
        "plan publication 时间顺序必须满足 goal <= baseline <= receipt <= published <= committed",
        "plan publication timestamps must satisfy goal <= baseline <= receipt <= published <= committed",
    ),
    (
        "plan extension {} 时间顺序必须位于前一 publication 之后",
        "plan extension {} must be timestamped after the preceding publication",
    ),
    (
        "goal.updated_at 不得早于最终 plan publication",
        "goal.updated_at must not precede the final plan publication",
    ),
    (
        "goal 不属于 {PLAN_PUBLICATION_POLICY_V1} plan publication epoch；旧 goal 只能读取或退休，不能追加计划",
        "goal does not belong to the {PLAN_PUBLICATION_POLICY_V1} plan publication epoch; old goals may only be read or retired and cannot have plans appended",
    ),
    (
        "goal 存在未完成且与本次调用不匹配的 plan publish intent；拒绝覆盖，必须恢复 intent 的 precheck 快照后用同一计划重试或退休该 goal",
        "goal has an unfinished plan publish intent that does not match this invocation; refusing to overwrite it; restore the intent's precheck snapshot and retry with the same plan, or retire the goal",
    ),
    (
        "源码在 plan 发布 CAS 窗口内发生变化；已保留 fail-closed plan publish intent（precheck={} confirmed={}），恢复原快照后用同一计划重试或退休该 goal",
        "source changed during the plan publication CAS window; the fail-closed plan publish intent was retained (precheck={} confirmed={}); restore the original snapshot and retry with the same plan, or retire the goal",
    ),
    (
        "goal 不属于 {PLAN_PUBLICATION_POLICY_V1} plan publication epoch；旧 goal 只能读取或退休，不能扩展计划",
        "goal does not belong to the {PLAN_PUBLICATION_POLICY_V1} plan publication epoch; old goals may only be read or retired and cannot have their plans extended",
    ),
    (
        "goal 存在未完成且与当前源码不匹配的 plan extension intent；必须恢复 intent 的 precheck 快照后用同一扩展重试或退休该 goal",
        "goal has an unfinished plan extension intent that does not match the current source; restore the intent's precheck snapshot and retry with the same extension, or retire the goal",
    ),
    (
        "goal 存在未完成且与本次调用不匹配的 plan extension intent；拒绝覆盖，必须用原扩展参数重试或退休该 goal",
        "goal has an unfinished plan extension intent that does not match this invocation; refusing to overwrite it; retry with the original extension arguments or retire the goal",
    ),
    (
        "源码在 plan extension 发布 CAS 窗口内发生变化；已保留 fail-closed plan publish intent（precheck={} confirmed={}），恢复原快照后用同一扩展重试或退休该 goal",
        "source changed during the plan extension publication CAS window; the fail-closed plan publish intent was retained (precheck={} confirmed={}); restore the original snapshot and retry with the same extension, or retire the goal",
    ),
    (
        "归档后的目标合约无效: {error}",
        "archived goal contract is invalid: {error}",
    ),
    (
        "归档后的 lifecycle proof 无效: {error}",
        "archived lifecycle proof is invalid: {error}",
    ),
    (
        "pending render --current 发现损坏的 goal state: {}",
        "pending render --current found corrupt goal state: {}",
    ),
    (
        "pending render --current 没有当前可咨询的 goal",
        "pending render --current found no currently consultable goal",
    ),
    (
        "pending render 必须且只能指定 --goal <id> 或 --current",
        "pending render must specify exactly one of --goal <id> or --current",
    ),
    ("已迁移 legacy pending {}。", "migrated legacy pending {}."),
    (
        "`goal pending present` 已退役：代理不能自证用户展示。使用 `rayman goal pending render --current` 生成 client-neutral workspace aggregate；渲染本身不构成展示、送达或完成证据",
        "`goal pending present` has been retired: an agent cannot self-attest that it presented content to the user. Use `rayman goal pending render --current` to generate the client-neutral workspace aggregate; rendering alone is not evidence of presentation, delivery, or completion",
    ),
    (
        "prepare 发现未计划的实际变更: {}。prepare 不会自动扩展 plan；先将这些路径恢复到 goal baseline，再按 program/args 逐参数调用: {}",
        "prepare found unplanned actual changes: {}. prepare will not extend the plan automatically; first restore these paths to the goal baseline, then invoke the provided program with each args item passed as a separate argument: {}",
    ),
    (
        "prepare 发现实际变更 {} 个文件但缺少首次修改前的 goal plan receipt: {}。prepare 不会事后补 plan；先将这些路径恢复到 goal baseline，再按 program/args 逐参数调用: {}",
        "prepare found {} actually changed files but no goal plan receipt recorded before the first modification: {}. prepare will not add a plan retroactively; first restore these paths to the goal baseline, then invoke the provided program with each args item passed as a separate argument: {}",
    ),
    (
        "prepare 期间源码发生变化，context index 与 goal delta 不属于同一快照: {}；请在源码稳定后重试",
        "source changed during prepare, so the context index and goal delta do not belong to the same snapshot: {}; retry after the source stabilizes",
    ),
    (
        "prepare 最终重验时 goal 已不存在: {goal_id}",
        "the goal no longer exists during prepare's final revalidation: {goal_id}",
    ),
    (
        "prepare 核心验证后 workspace 或 goal 状态发生变化；snapshot readiness 已失效，请重试（workspace {} -> {}；goal {} -> {}）",
        "workspace or goal state changed after prepare's core verification; snapshot readiness is no longer valid, so retry (workspace {} -> {}; goal {} -> {})",
    ),
    (
        "无法从同一 no-follow 文件句柄读取 workspace_skill.yaml",
        "could not read workspace_skill.yaml from the same no-follow file handle",
    ),
    (
        "激活元数据写探针: 就绪（原授权元数据 staging 已验证，激活文件未变）",
        "activation-metadata write probe: ready (original authorization metadata staging verified; activation unchanged)",
    ),
    (
        "激活元数据写探针: 无激活合同或平台不支持，未探测",
        "activation-metadata write probe: not probed (activation contract absent or platform unsupported)",
    ),
    (
        "激活元数据写探针: 失败 phase={:?} class={:?} os_error={} activation_unchanged={:?} cleanup_complete={:?}: {}",
        "activation-metadata write probe: failed phase={:?} class={:?} os_error={} activation_unchanged={:?} cleanup_complete={:?}: {}",
    ),
    (
        "workspace 激活合同结构上可 rebind，且当前 activation metadata staging 探针已就绪：运行 `{command}`",
        "workspace activation contract is structurally rebindable and the current activation-metadata staging probe is ready: run `{command}`",
    ),
    (
        "workspace 激活合同结构上可 rebind，但当前 activation metadata staging 探针未就绪（phase={:?}, failure_class={}）；先按 failure_class 处理该 action-specific 能力边界，再运行 `{command}`",
        "workspace activation contract is structurally rebindable, but the current activation-metadata staging probe is not ready (phase={:?}, failure_class={}); handle that action-specific capability boundary according to failure_class before running `{command}`",
    ),
    (
        "RaymanCodingSkill 工作区激活身份已漂移（status={}）：运行 `{command}`",
        "RaymanCodingSkill workspace activation identity has drifted (status={}): run `{command}`",
    ),
    (
        "skill_file 不可安全读取 {}: {error}",
        "could not safely read skill_file {}: {error}",
    ),
    (
        "skill_file 不是普通文件: {}",
        "skill_file is not an ordinary file: {}",
    ),
    (
        "skill_file 当前内容与此 CLI 内嵌的 canonical SKILL.md 不一致",
        "skill_file contents do not match the canonical SKILL.md embedded in this CLI",
    ),
    (
        "skill_file 在 rebind 写后验证期间发生并发变化",
        "skill_file was concurrently modified during post-write rebind verification",
    ),
    (
        "skill_file 在 rebind 发布前发生并发变化",
        "skill_file was concurrently modified before rebind publication",
    ),
    (
        "skill_file 在校验期间发生变化: {}",
        "skill_file changed during validation: {}",
    ),
    (
        "skill_file 路径不得经过链接/reparse: {}",
        "skill_file path must not traverse a link/reparse point: {}",
    ),
    (
        "skill_file 路径不是 activate 可生成的安全规范形式",
        "skill_file path is not in a safe canonical form producible by workspace activate",
    ),
    ("skill_file 路径为空", "skill_file path is empty"),
    (
        "skill_file 路径组件不是目录: {}",
        "skill_file path component is not a directory: {}",
    ),
    (
        "workspace rebind 只接受 enabled: true",
        "workspace rebind accepts only enabled: true",
    ),
    (
        "workspace rebind 只接受 skill: {SKILL_NAME}",
        "workspace rebind accepts only skill: {SKILL_NAME}",
    ),
    (
        "workspace rebind 只接受完整六字段激活合同",
        "workspace rebind accepts only a complete six-field activation contract",
    ),
    (
        "workspace rebind 已无需更新，但激活状态仍无效",
        "workspace rebind found no update to apply, but activation is still invalid",
    ),
    (
        "workspace rebind 拒绝无法原样安全写回的 skill_file",
        "workspace rebind refuses a skill_file that cannot be safely written back verbatim",
    ),
    (
        "workspace rebind 拒绝格式无效的旧身份字段",
        "workspace rebind refuses invalid recorded identity fields",
    ),
    (
        "workspace rebind 缺少 skill_file",
        "workspace rebind is missing skill_file",
    ),
    (
        "workspace_skill.yaml 在 rebind 发布前发生并发变化",
        "workspace_skill.yaml was concurrently modified before rebind publication",
    ),
    (
        "workspace_skill.yaml 必须是有效 UTF-8",
        "workspace_skill.yaml must be valid UTF-8",
    ),
    (
        "写入后的 workspace rebind 合同仍无效: {}",
        "the workspace rebind contract remains invalid after writing: {}",
    ),
    (
        "原激活合同不是有效 UTF-8",
        "the original activation contract is not valid UTF-8",
    ),
    (
        "当前 workspace 激活合同不可 rebind: {}",
        "the current workspace activation contract is not eligible for rebind: {}",
    ),
    (
        "无法复查 skill_file: {}",
        "unable to recheck skill_file: {}",
    ),
    (
        "无法规范化 skill_file: {}",
        "unable to canonicalize skill_file: {}",
    ),
    (
        "无法读取 skill_file 路径组件: {}",
        "unable to read a skill_file path component: {}",
    ),
    (
        "workspace 未激活：已停止自动保存并注销计划任务 '{}'。最终快照已跳过；如需抢救快照，运行 `rayman checkpoint salvage-save`。",
        "the workspace is not activated: autosave stopped and scheduled task '{}' was unregistered. The final snapshot was skipped; to salvage a snapshot, run `rayman checkpoint salvage-save`.",
    ),
    (
        "已存最后一次快照并停止自动保存（状态：{status}）。计划任务 '{}' 已注销。",
        "saved a final snapshot and stopped autosave (status: {status}). Scheduled task '{}' was unregistered.",
    ),
    (
        "已存最后一次快照并停止自动保存（状态：{status}）。计划任务 '{}' 未注册。",
        "saved a final snapshot and stopped autosave (status: {status}). Scheduled task '{}' was not registered.",
    ),
    (
        "workspace 未激活：已停止自动保存；计划任务 '{}' 未注册。最终快照已跳过；如需抢救快照，运行 `rayman checkpoint salvage-save`。",
        "the workspace is not activated: autosave stopped; scheduled task '{}' was not registered. The final snapshot was skipped; to salvage a snapshot, run `rayman checkpoint salvage-save`.",
    ),
    (
        "Codex hooks 正被另一个进程修改: {}；等待锁超过 {} 秒",
        "Codex hooks are being modified by another process: {}; waited more than {} seconds for the lock",
    ),
    (
        "另有 {other} 份 recovery-only/partial 快照，用 `rayman checkpoint list` 查看。",
        "there are also {other} recovery-only/partial snapshots — inspect them with `rayman checkpoint list`.",
    ),
    ("工具 {}: 已找到", "tool {}: found"),
    (
        "工具 {}: 不可达，本工作区不需要",
        "tool {}: unreachable, and this workspace does not need it",
    ),
    ("工具 {}: 不可达", "tool {}: unreachable"),
    ("需要它来: {}", "needed for: {}"),
    (
        "遗留的原子写临时项 `{name}` 不安全或无效: {error:#}",
        "leaked atomic-write scratch entry `{name}` is unsafe or invalid: {error:#}",
    ),
    ("无法检查: {}", "could not inspect: {}"),
    ("不是普通文件: {}", "is not an ordinary file: {}"),
    ("工具 {}: {}", "tool {}: {}"),
    (
        "上下文: {} → 运行 `rayman context refresh`",
        "context index: {} → run `rayman context refresh`",
    ),
    ("上下文: {}", "context index: {}"),
    ("警告: {warning}", "warning: {warning}"),
    (
        "orphan restore transaction 未能回滚，本次抢救快照可能捕获了恢复中途的工作区: {error:#}",
        "the orphan restore transaction could not be rolled back, so this salvage snapshot may have captured a workspace mid-restore: {error:#}",
    ),
    (
        "lifecycle-only authority goal 缺少 lifecycle proof",
        "the lifecycle-only authority goal has no lifecycle proof",
    ),
    (
        "资产扫描无法读取文件元数据: {}",
        "the asset scan could not read the file's metadata: {}",
    ),
    (
        "authority goal 不存在: {authority_goal_id}",
        "authority goal does not exist: {authority_goal_id}",
    ),
    (
        "authority goal 必须是同 workspace、current-policy 且包含同命令 direct-authority 的有效 archived success",
        "the authority goal must be a valid archived success in the same workspace, on the current policy, holding a direct-authority receipt for the same command",
    ),
    (
        "authority goal 缺少 lifecycle proof",
        "the authority goal has no lifecycle proof",
    ),
    (
        "authority receipt invocation hash 无效",
        "the authority receipt's invocation hash is invalid",
    ),
    ("checkpoint 缺少 manifest", "the checkpoint has no manifest"),
    (
        "checkpoint 缺少 manifest: {}",
        "the checkpoint has no manifest: {}",
    ),
    (
        "checkpoint 路径不是普通文件: {}",
        "the checkpoint path is not an ordinary file: {}",
    ),
    ("checkpoint 路径为空", "the checkpoint path is empty"),
    (
        "context schema/workspace identity 不匹配",
        "the context schema or workspace identity does not match",
    ),
    (
        "lifecycle-only replacement 至少需要一个 --supersedes 目标",
        "a lifecycle-only replacement requires at least one --supersedes goal",
    ),
    (
        "manifest 包含重复路径: {}",
        "the manifest contains a duplicate path: {}",
    ),
    (
        "restore journal original/expected 路径不一致: {} != {}",
        "restore journal original/expected paths disagree: {} != {}",
    ),
    (
        "restore journal 包含重复路径: {}",
        "the restore journal contains a duplicate path: {}",
    ),
    (
        "restore journal 工作区不匹配: recorded={:?} current={:?}",
        "restore journal workspace mismatch: recorded={:?} current={:?}",
    ),
    (
        "restore transaction 缺少 journal.json",
        "the restore transaction has no journal.json",
    ),
    (
        "review receipt 必须绑定已记录的 goal plan",
        "a review receipt must be bound to a recorded goal plan",
    ),
    (
        "reviewer 与 summary 都不能为空",
        "neither reviewer nor summary may be empty",
    ),
    (
        "validation receipt 与 immutable goal/requirement contract 不匹配",
        "the validation receipt does not match the immutable goal/requirement contract",
    ),
    (
        "上下文索引拒绝链接/reparse 路径: {}",
        "the context index refuses a link/reparse path: {}",
    ),
    (
        "上下文索引文件逃逸工作区: {} -> {}",
        "context index file escapes the workspace: {} -> {}",
    ),
    (
        "上下文索引无法哈希文件: {}",
        "the context index could not hash the file: {}",
    ),
    (
        "上下文索引无法复查文件元数据: {}",
        "the context index could not re-check the file's metadata: {}",
    ),
    (
        "上下文索引无法读取文件: {}",
        "the context index could not read the file: {}",
    ),
    (
        "上下文索引无法读取文件元数据: {}",
        "the context index could not read the file's metadata: {}",
    ),
    (
        "只有 active/current 目标可以记录 plan receipt",
        "only an active/current goal can record a plan receipt",
    ),
    (
        "只有 active/success 的 current-schema 目标可以记录 review receipt",
        "only an active/success current-schema goal can record a review receipt",
    ),
    (
        "回滚目标不是安全普通文件: {}",
        "the rollback target is not a safe ordinary file: {}",
    ),
    (
        "恢复目标不是普通文件: {}",
        "the restore target is not an ordinary file: {}",
    ),
    (
        "拒绝关闭为 success：handoff contract invalid: {error}",
        "refusing to close as success: handoff contract invalid: {error}",
    ),
    (
        "拒绝关闭为 success：目标合约无效: {error}",
        "refusing to close as success: the goal contract is invalid: {error}",
    ),
    (
        "拒绝写入 lifecycle-only replacement proof: {error}",
        "refusing to write the lifecycle-only replacement proof: {error}",
    ),
    (
        "拒绝写入 lifecycle-only replacement: {error}",
        "refusing to write the lifecycle-only replacement: {error}",
    ),
    (
        "拒绝链接/reparse 恢复目标: {}",
        "refusing a link/reparse restore target: {}",
    ),
    (
        "拒绝链接/reparse 路径: {}",
        "refusing a link/reparse path: {}",
    ),
    (
        "无法列出 checkpoint 树目录: {}",
        "could not list the checkpoint tree directory: {}",
    ),
    (
        "无法列出 checkpoint 目录: {}",
        "could not list the checkpoint directory: {}",
    ),
    (
        "无法创建 restore backups: {}",
        "could not create restore backups: {}",
    ),
    (
        "无法创建 restore staging: {}",
        "could not create restore staging: {}",
    ),
    (
        "无法创建 restore transaction 目录: {}",
        "could not create the restore transaction directory: {}",
    ),
    (
        "无法创建恢复目标目录: {}",
        "could not create the restore target directory: {}",
    ),
    (
        "无法创建目标状态目录",
        "could not create the goal state directory",
    ),
    ("无法同步目录: {}", "could not sync the directory: {}"),
    (
        "无法回滚 restore 目标: {}",
        "could not roll back the restore target: {}",
    ),
    (
        "无法复查 restore 目标: {}",
        "could not re-check the restore target: {}",
    ),
    (
        "无法复查文件元数据: {}",
        "could not re-check the file's metadata: {}",
    ),
    (
        "无法安全读取 context 状态: {error:#}",
        "could not read the context state safely: {error:#}",
    ),
    (
        "无法检查 orphan restore transaction: {}",
        "could not inspect the orphan restore transaction: {}",
    ),
    (
        "无法检查 restore journal: {}",
        "could not inspect the restore journal: {}",
    ),
    (
        "无法检查 restore 目标: {}",
        "could not inspect the restore target: {}",
    ),
    (
        "无法检查回滚目标: {}",
        "could not inspect the rollback target: {}",
    ),
    (
        "无法清理 restore transaction: {}",
        "could not clean up the restore transaction: {}",
    ),
    (
        "无法清理没有 journal 的 orphan restore transaction: {}",
        "could not clean up the journal-less orphan restore transaction: {}",
    ),
    (
        "无法解析 goal 文件: current schema: {current_error}; legacy schema: {legacy_error}",
        "could not parse the goal file: current schema: {current_error}; legacy schema: {legacy_error}",
    ),
    (
        "无法读取 checkpoint 路径: {}",
        "could not read the checkpoint path: {}",
    ),
    (
        "无法读取恢复目标: {}",
        "could not read the restore target: {}",
    ),
    (
        "无法读取恢复目标目录: {}",
        "could not read the restore target directory: {}",
    ),
    (
        "无法读取文件元数据: {}",
        "could not read the file's metadata: {}",
    ),
    (
        "未知 review_priority: {left}",
        "unknown review_priority: {left}",
    ),
    (
        "未知 review_priority: {right}",
        "unknown review_priority: {right}",
    ),
    ("未知 review_priority: {}", "unknown review_priority: {}"),
    (
        "未知的关闭状态: {status}（可用: success | partial | blocked）",
        "unknown close status: {status} (available: success | partial | blocked)",
    ),
    (
        "没有完整且已验证的 checkpoint",
        "there is no complete, verified checkpoint",
    ),
    (
        "目标 {id} 已隔离为 untrusted history；隔离是单向降级，审计记录必须保留，不能恢复为 current",
        "goal {id} is quarantined as untrusted history; the quarantine is a one-way downgrade, the audit record must be retained, and it cannot be restored to current",
    ),
    ("目标标题不能为空", "the goal title must not be empty"),
    (
        "目标状态目录不存在",
        "the goal state directory does not exist",
    ),
    (
        "被转移目标 {predecessor_id} 合约无效: {error}",
        "transferred goal {predecessor_id} has an invalid contract: {error}",
    ),
    (
        "被转移目标不存在: {predecessor_id}",
        "transferred goal does not exist: {predecessor_id}",
    ),
    (
        "需求不存在: {req_id}",
        "requirement does not exist: {req_id}",
    ),
    (
        "非法目标 id: {id}（只允许字母、数字、下划线和连字符）",
        "illegal goal id: {id} (only letters, digits, underscores and hyphens are allowed)",
    ),
    (
        "--supersedes 不能包含重复目标",
        "--supersedes must not contain duplicate goals",
    ),
    (
        "archived goal 已有显式 receipt policy；拒绝降级或重复迁移",
        "archived goal already has an explicit receipt policy; refusing to downgrade or migrate twice",
    ),
    (
        "authority receipt 与 requirement/command/scope 合同不匹配",
        "authority receipt does not match the requirement/command/scope contract",
    ),
    (
        "authority receipt 必须包含至少两次完整稳定执行",
        "authority receipt must contain at least two complete stable runs",
    ),
    (
        "authority receipt 未证明同一 workspace fingerprint 上的重复稳定 PASS",
        "authority receipt did not prove a repeated stable PASS on one workspace fingerprint",
    ),
    (
        "checkpoint manifest 不是受支持的 v3 schema: schema={:?} version={}",
        "checkpoint manifest is not a supported v3 schema: schema={:?} version={}",
    ),
    (
        "checkpoint manifest 含不安全相对路径: {text}",
        "checkpoint manifest contains an unsafe relative path: {text}",
    ),
    (
        "checkpoint restore 失败且回滚不完整；已保留 transaction {} 供恢复: {operation_error:#}; rollback: {rollback_error:#}",
        "checkpoint restore failed and the rollback is incomplete; transaction {} was kept for recovery: {operation_error:#}; rollback: {rollback_error:#}",
    ),
    (
        "checkpoint restore 失败，工作区已回滚，但无法清理 transaction {}: {operation_error:#}; cleanup: {cleanup_error:#}",
        "checkpoint restore failed and the workspace was rolled back, but transaction {} could not be cleaned up: {operation_error:#}; cleanup: {cleanup_error:#}",
    ),
    (
        "checkpoint restore 失败，工作区已完整回滚: {operation_error:#}",
        "checkpoint restore failed; the workspace was rolled back completely: {operation_error:#}",
    ),
    (
        "checkpoint {} 是 recovery-only；修复并重新激活工作区后，显式加 --allow-recovery-only 才能恢复",
        "checkpoint {} is recovery-only; repair and re-activate the workspace, then pass --allow-recovery-only explicitly to restore it",
    ),
    (
        "checkpoint 不是完整快照（状态：{:?}）",
        "checkpoint is not a complete snapshot (status: {:?})",
    ),
    (
        "checkpoint 使用旧 v2 content-only manifest，缺少权限完整性证明，不能安全恢复；请用当前 Rayman 新建 v3 checkpoint",
        "checkpoint uses the old v2 content-only manifest and lacks permission integrity proof, so it cannot be restored safely; create a v3 checkpoint with the current Rayman",
    ),
    (
        "checkpoint 完整性记录含无效 Unix mode: {}",
        "checkpoint integrity record has an invalid Unix mode: {}",
    ),
    (
        "checkpoint 完整性记录缺少 Unix mode 权限证明，不能在 Unix 安全恢复: {}",
        "checkpoint integrity record lacks Unix mode permission proof and cannot be restored safely on Unix: {}",
    ),
    (
        "checkpoint 完整性记录缺少 readonly 权限证明（旧 content-only manifest），请新建 v3 checkpoint: {}",
        "checkpoint integrity record lacks readonly permission proof (old content-only manifest); create a v3 checkpoint: {}",
    ),
    (
        "checkpoint 已完整恢复，但无法清理 restore transaction {}",
        "the checkpoint was restored completely, but restore transaction {} could not be cleaned up",
    ),
    (
        "checkpoint 总字节数溢出",
        "checkpoint total byte count overflowed",
    ),
    (
        "checkpoint 文件完整性不匹配: {}",
        "checkpoint file integrity mismatch: {}",
    ),
    (
        "checkpoint 路径不是有效 UTF-8: {}",
        "checkpoint path is not valid UTF-8: {}",
    ),
    (
        "checkpoint 路径含不安全组件: {}",
        "checkpoint path contains an unsafe component: {}",
    ),
    (
        "checkpoint 路径组件不是目录: {}",
        "checkpoint path component is not a directory: {}",
    ),
    (
        "context 索引包含读取失败条目: {}",
        "context index contains an entry that failed to read: {}",
    ),
    (
        "context 索引包含重复路径: {}",
        "context index contains a duplicate path: {}",
    ),
    (
        "goal plan --extend 拒绝事后补票；已有未计划变更: {}",
        "goal plan --extend refuses a retroactive ticket; unplanned changes already exist: {}",
    ),
    (
        "goal plan --extend 拒绝已发生变化的新路径: {added}",
        "goal plan --extend refuses a new path that has already changed: {added}",
    ),
    (
        "goal plan --extend 至少需要一个变更路径",
        "goal plan --extend requires at least one changed path",
    ),
    (
        "goal plan --extend 要求恰好一个基础聚合 plan receipt",
        "goal plan --extend requires exactly one base aggregate plan receipt",
    ),
    (
        "goal plan 是首次修改前的一次性聚合合同，不能追加或拆分；请在变更前一次列出完整路径",
        "goal plan is a one-time aggregate contract taken before the first edit; it cannot be appended to or split. List every path once, before changing anything",
    ),
    (
        "goal plan 至少需要一个变更路径",
        "goal plan requires at least one changed path",
    ),
    (
        "lifecycle-only replacement 必须保持 pristine 且只能包含 open must",
        "a lifecycle-only replacement must stay pristine and may contain only open musts",
    ),
    (
        "live lifecycle authority 未证明当前源码上的重复稳定仓库 gate",
        "the live lifecycle authority did not prove a repeated stable repository gate on the current source",
    ),
    (
        "manifest file_count={} 与完整性条目数={} 不一致",
        "manifest file_count={} does not match the integrity entry count={}",
    ),
    (
        "manifest total_bytes={} 与文件完整性总和={} 不一致",
        "manifest total_bytes={} does not match the file integrity sum={}",
    ),
    (
        "manifest 含无效 SHA-256: {}",
        "manifest contains an invalid SHA-256: {}",
    ),
    (
        "orphan restore transaction 无法安全加载，已保留并拒绝继续: {}",
        "orphan restore transaction could not be loaded safely; it was kept and the run refuses to continue: {}",
    ),
    (
        "orphan restore transaction 没有 journal 却仍存有备份文件，无从判断该回滚哪些目标；已保留供人工恢复。恢复步骤：检查该目录 backups/ 子目录中的原件、取回仍需要的文件，再删除整个目录以解除对 save/restore/autosave 的阻塞：{}",
        "orphan restore transaction has no journal yet still holds backup files, so there is no way to tell which targets to roll back; it was kept for manual recovery. Recovery steps: inspect the originals under that directory's backups/, take back whatever you still need, then delete the whole directory to unblock save/restore/autosave: {}",
    ),
    (
        "orphan restore transaction 自动回滚不完整，已保留并拒绝继续: {}",
        "orphan restore transaction could not be rolled back completely; it was kept and the run refuses to continue: {}",
    ),
    (
        "pre-receipt migration 与 receipt-policy migration 不能同时使用",
        "pre-receipt migration and receipt-policy migration cannot be used together",
    ),
    (
        "replacement must 必须与 --supersedes 目标 must（含 typed proof 义务）的精确并集一致",
        "the replacement musts must equal the exact union of the --supersedes goals' musts (including typed proof obligations)",
    ),
    (
        "replacement、authority goal 与被转移目标必须彼此不同",
        "the replacement, the authority goal and the transferred goals must all differ",
    ),
    (
        "restore journal committed 阶段条目状态不完整",
        "restore journal committed-phase entry states are incomplete",
    ),
    (
        "restore journal preparing 阶段含 publish_attempted 条目",
        "restore journal preparing phase contains a publish_attempted entry",
    ),
    (
        "restore journal schema/version 不受支持: schema={:?} version={}",
        "restore journal schema/version is unsupported: schema={:?} version={}",
    ),
    (
        "restore journal 包含重复新建目录: {}",
        "restore journal contains a duplicate created directory: {}",
    ),
    (
        "restore journal 发布条目尚未预备目标: {}",
        "restore journal publish entry has no prepared destination: {}",
    ),
    (
        "restore journal 回滚完成条目从未发布: {}",
        "restore journal rollback-complete entry was never published: {}",
    ),
    (
        "restore journal 新建目录不是任何恢复目标的祖先: {}",
        "restore journal created directory is not an ancestor of any restore target: {}",
    ),
    (
        "restore journal 新建目录路径未规范化: {}",
        "restore journal created-directory path is not normalized: {}",
    ),
    (
        "restore journal 路径未规范化: {}",
        "restore journal path is not normalized: {}",
    ),
    (
        "restore staging 完整性不匹配: {}",
        "restore staging integrity mismatch: {}",
    ),
    (
        "restore transaction 发布索引越界: {index}",
        "restore transaction publish index out of range: {index}",
    ),
    (
        "restore transaction 回滚索引越界: {index}",
        "restore transaction rollback index out of range: {index}",
    ),
    (
        "restore transaction 目标尚未预备: {}",
        "restore transaction destination is not prepared: {}",
    ),
    (
        "restore 新建目录 journal 条目丢失",
        "restore created-directory journal entry is missing",
    ),
    (
        "restore 新建目录逃逸工作区: {}",
        "restore created directory escapes the workspace: {}",
    ),
    (
        "restore 目录只有创建意图且非空，无法证明所有权，拒绝删除 {}（{error}）；确认目录内容后手动删除它，再删除 transaction 目录 {} 以解除阻塞",
        "the restore directory only has a creation intent and is not empty, so ownership cannot be proven and it will not be deleted: {} ({error}). Check its contents, delete it by hand, then delete transaction directory {} to clear the block",
    ),
    (
        "restore 目标在备份后发生变化，拒绝覆盖: {}",
        "the restore target changed after it was backed up; refusing to overwrite: {}",
    ),
    (
        "restore 目标备份完整性不匹配: {}",
        "restore target backup integrity mismatch: {}",
    ),
    (
        "validation --changed 超出 goal plan: {}",
        "validation --changed goes beyond the goal plan: {}",
    ),
    (
        "validation receipt 与命令/影响路径不匹配",
        "the validation receipt does not match the command/impact paths",
    ),
    (
        "validation 拒绝未计划的实际变更: {}",
        "validation refuses unplanned actual changes: {}",
    ),
    (
        "{path}: 无法持久化回滚完成状态: {error:#}",
        "{path}: could not persist the rollback-complete state: {error:#}",
    ),
    (
        "manifest 记录 {} 实为 {}",
        "manifest recorded {}, which is actually {}",
    ),
    (
        "上下文文件在索引验证后发生变化: {} (size {} != {} or sha256 {} != {})",
        "context file changed after index verification: {} (size {} != {} or sha256 {} != {})",
    ),
    (
        "上下文索引不是 ready（当前: {}）。先运行 `rayman context refresh`。{}",
        "the context index is not ready (current: {}). Run `rayman context refresh` first.{}",
    ),
    (
        "上下文索引拒绝不安全相对路径: {}",
        "the context index refuses an unsafe relative path: {}",
    ),
    (
        "上下文索引拒绝非普通文件: {}",
        "the context index refuses a non-ordinary file: {}",
    ),
    (
        "上下文索引文件不属于工作区: {} under {}",
        "context index file does not belong to the workspace: {} under {}",
    ),
    (
        "上下文索引无法统计文件行数: {}",
        "the context index could not count the file's lines: {}",
    ),
    (
        "上下文索引条目含不安全路径: {}",
        "context index entry contains an unsafe path: {}",
    ),
    (
        "上下文索引缺失。先运行 `rayman context refresh`。",
        "the context index is missing. Run `rayman context refresh` first.",
    ),
    (
        "上下文索引读取期间文件发生变化: {}",
        "context index file changed while it was being read: {}",
    ),
    (
        "上下文索引路径组件不是目录: {}",
        "context index path component is not a directory: {}",
    ),
    (
        "不安全的 checkpoint 目标路径: {}",
        "unsafe checkpoint destination path: {}",
    ),
    (
        "不安全的 checkpoint 相对路径: {}",
        "unsafe checkpoint relative path: {}",
    ),
    (
        "不能 supersede 目标 {id}: {error}",
        "cannot supersede goal {id}: {error}",
    ),
    (
        "历史 goal 不满足 receipt_integrity_v1；拒绝刷新 lifecycle proof",
        "the historical goal does not satisfy receipt_integrity_v1; refusing to refresh the lifecycle proof",
    ),
    (
        "历史 lifecycle proof 的 workspace fingerprint 非法，不能生成可核验隔离记录",
        "the historical lifecycle proof has an invalid workspace fingerprint, so no verifiable quarantine record can be issued",
    ),
    (
        "历史目标缺少旧 lifecycle proof，不能证明该归档证据曾经失效",
        "the historical goal has no old lifecycle proof, so there is no way to prove the archived evidence ever became invalid",
    ),
    (
        "原子替换目标不是安全普通文件: {}",
        "the atomic replacement target is not a safe ordinary file: {}",
    ),
    (
        "原本不存在的 restore 目标在发布前出现，拒绝覆盖: {}",
        "a restore target that did not exist appeared before publishing; refusing to overwrite: {}",
    ),
    (
        "发现不安全的 orphan restore transaction，已保留并拒绝继续: {}",
        "found an unsafe orphan restore transaction; it was kept and the run refuses to continue: {}",
    ),
    (
        "只允许隔离 proof 已失效的已归档 success，或无法生成可信归档 proof 的完整 current legacy success；有效或尚未结束的 current goal 不能隐藏",
        "only an archived success with an invalid proof, or a complete current legacy success with no trusted archive proof, may be quarantined; valid or unfinished current goals cannot be hidden",
    ),
    (
        "历史待完成项（保留，不阻塞）: {}",
        "Historical pending items (retained, non-blocking): {}",
    ),
    (
        "只有 current goal 可以归档；已迁移的 archived goal 可用 --migrate-unreceipted 幂等刷新 proof",
        "only a current goal can be archived; an already migrated archived goal can be refreshed idempotently with --migrate-unreceipted",
    ),
    (
        "只有 current goal 可以被 supersede",
        "only a current goal can be superseded",
    ),
    (
        "只有 current-schema active/current 目标可以扩展 plan",
        "only a current-schema active/current goal can extend its plan",
    ),
    (
        "只有 must 已完整结束的 current-schema archived success 可以隔离",
        "only a current-schema archived success whose musts are fully finished can be quarantined",
    ),
    (
        "只有 receipt-policy-v2 rollout 前的 schema-v2 success goal 可以迁移 v1 proof",
        "only a schema-v2 success goal from before the receipt-policy-v2 rollout can migrate a v1 proof",
    ),
    (
        "只有符合 rollout 前条件的 schema-v2 success goal 可以刷新 migration proof",
        "only a schema-v2 success goal that meets the pre-rollout conditions can refresh its migration proof",
    ),
    (
        "回滚后目标完整性不匹配: {}",
        "target integrity mismatch after rollback: {}",
    ),
    (
        "回滚备份完整性不匹配: {}",
        "rollback backup integrity mismatch: {}",
    ),
    (
        "回滚目标已被第三方修改，拒绝覆盖: {}。原件仍在该 transaction 的 backups/ 子目录中；取回仍需要的内容后删除整个 transaction 目录即可解除对 save/restore/autosave 的阻塞，salvage-save 不受阻塞",
        "the rollback target was modified by a third party; refusing to overwrite: {}. The originals are still under that transaction's backups/ directory; take back whatever you still need, then delete the whole transaction directory to unblock save/restore/autosave. salvage-save is never blocked by it",
    ),
    (
        "完整 checkpoint manifest 含有跳过项或错误记录",
        "a complete checkpoint manifest contains skipped items or error records",
    ),
    (
        "完整性记录含无效 SHA-256: {}",
        "integrity record contains an invalid SHA-256: {}",
    ),
    (
        "工作区已偏离 goal 开工 baseline；拒绝事后补 plan。baseline={} current={}",
        "the workspace has drifted from the goal's starting baseline; refusing a retroactive plan. baseline={} current={}",
    ),
    (
        "工作区遍历失败: {error:#}",
        "workspace traversal failed: {error:#}",
    ),
    (
        "归档 success 的 lifecycle proof 仍然有效；拒绝把有效证据降级为 quarantine",
        "the archived success still has a valid lifecycle proof; refusing to downgrade valid evidence to a quarantine",
    ),
    (
        "current success 仍可生成可信 archive proof；拒绝降级为 quarantine，请使用普通 archive 或显式历史 receipt migration",
        "the current success can still produce a trusted archive proof; refusing to downgrade it to quarantine, so use ordinary archive or an explicit historical receipt migration",
    ),
    ("归档原因不能为空", "the archive reason must not be empty"),
    (
        "恢复前源文件完整性发生变化: {}",
        "source file integrity changed before the restore: {}",
    ),
    (
        "恢复后目标文件完整性不匹配: {}",
        "target file integrity mismatch after the restore: {}",
    ),
    (
        "恢复目标是目录而非文件: {}",
        "the restore target is a directory, not a file: {}",
    ),
    ("找不到 checkpoint: {id}", "checkpoint not found: {id}"),
    (
        "拒绝关闭为 blocked：必须先记录至少一个带完整解决方案包的 human/external pending，且不能仍有 agent-owned pending",
        "refusing to close as blocked: record at least one human/external pending with a complete resolution package first, and no agent-owned pending may remain",
    ),
    (
        "拒绝关闭为 success：legacy goal 不能生成当前 receipt；只可归档已是 success 的历史记录",
        "refusing to close as success: a legacy goal cannot produce a current receipt; only an already-success historical record can be archived",
    ),
    (
        "拒绝关闭为 success：必须先用 goal validate 写入当前且相关的 receipt: {}",
        "refusing to close as success: write a current and relevant receipt with goal validate first: {}",
    ),
    (
        "拒绝恢复 checkpoint {}：它由旧版本 Rayman 生成，manifest 记录的是大小写折叠后的比较键而非真实文件名，按它恢复出的文件名大小写不可信（{}）；请用当前版本重新 `rayman checkpoint save` 后再恢复",
        "refusing to restore checkpoint {}: it was produced by an older Rayman whose manifest recorded case-folded comparison keys instead of real file names, so the restored file-name case is untrustworthy ({}); re-run `rayman checkpoint save` with the current version and restore from that",
    ),
    (
        "拒绝恢复 recovery-only checkpoint {}：当前激活合同尚未修复",
        "refusing to restore recovery-only checkpoint {}: the current activation contract is not repaired yet",
    ),
    (
        "拒绝恢复非完整 checkpoint {}（状态：{:?}）",
        "refusing to restore a non-complete checkpoint {} (status: {:?})",
    ),
    (
        "拒绝清理不属于工作区 checkpoint 的 transaction: {}",
        "refusing to clean up a transaction that does not belong to this workspace's checkpoint: {}",
    ),
    (
        "拒绝链接/reparse checkpoint 路径组件: {}",
        "refusing a link/reparse checkpoint path component: {}",
    ),
    ("拒绝非普通文件: {}", "refusing a non-ordinary file: {}"),
    (
        "文件在读取期间变更或变为链接: {}",
        "the file changed or became a link while it was being read: {}",
    ),
    (
        "新目标至少需要一个非空 --must 需求",
        "a new goal requires at least one non-empty --must requirement",
    ),
    (
        "无效的 restore 新建目录 {}: {error:#}",
        "invalid restore created directory {}: {error:#}",
    ),
    (
        "无法临时解除原子替换目标只读属性: {}",
        "could not temporarily clear the read-only attribute on the atomic replacement target: {}",
    ),
    (
        "无法删除本次 restore 新建目录 {}: {error}",
        "could not delete the directory this restore created: {} ({error})",
    ),
    (
        "无法删除本次新增 restore 文件: {}",
        "could not delete the file this restore added: {}",
    ),
    (
        "无法发布 restore 文件: {}",
        "could not publish the restore file: {}",
    ),
    (
        "无法备份 restore 目标: {}",
        "could not back up the restore target: {}",
    ),
    (
        "无法复查已验证上下文文件元数据: {}",
        "could not re-check the verified context file's metadata: {}",
    ),
    (
        "无法扫描 restore transaction: {}",
        "could not scan restore transactions: {}",
    ),
    ("无法扫描目录: {}", "could not scan the directory: {}"),
    (
        "无法持久化 restore journal: {}",
        "could not persist the restore journal: {}",
    ),
    (
        "无法检查目录条目: {}",
        "could not inspect the directory entry: {}",
    ),
    (
        "无法清理旧 checkpoint: {}",
        "could not clean up the old checkpoint: {}",
    ),
    (
        "无法规范化上下文文件: {}",
        "could not canonicalize the context file: {}",
    ),
    (
        "无法读取 checkpoint 树条目: {}",
        "could not read the checkpoint tree entry: {}",
    ),
    (
        "无法读取 checkpoint 目录条目: {}",
        "could not read the checkpoint directory entry: {}",
    ),
    (
        "无法读取 checkpoint 路径组件: {}",
        "could not read the checkpoint path component: {}",
    ),
    (
        "无法读取 restore transaction 条目: {}",
        "could not read the restore transaction entry: {}",
    ),
    (
        "无法读取原子替换目标权限: {}",
        "could not read the atomic replacement target's permissions: {}",
    ),
    (
        "无法读取已验证上下文文件: {}",
        "could not read the verified context file: {}",
    ),
    (
        "无法读取已验证上下文文件元数据: {}",
        "could not read the verified context file's metadata: {}",
    ),
    (
        "无法读取目录条目: {}",
        "could not read the directory entry: {}",
    ),
    (
        "无法预备 restore staging 文件: {}",
        "could not prepare the restore staging file: {}",
    ),
    (
        "替代目标 {replacement_id} lifecycle={}，必须先恢复为 current",
        "replacement goal {replacement_id} has lifecycle={}; restore it to current first",
    ),
    (
        "替代目标 {replacement_id} 合约无效: {error}",
        "replacement goal {replacement_id} has an invalid contract: {error}",
    ),
    (
        "替代目标 {replacement_id} 必须是 current schema；legacy success 只能显式 archive",
        "replacement goal {replacement_id} must be current schema; a legacy success can only be archived explicitly",
    ),
    (
        "替代目标不存在: {id}",
        "replacement goal does not exist: {id}",
    ),
    (
        "替代目标不存在: {replacement_id}",
        "replacement goal does not exist: {replacement_id}",
    ),
    (
        "替代目标合约无效: {error}",
        "the replacement goal has an invalid contract: {error}",
    ),
    (
        "替代目标必须是未授权的 current/active current-schema goal",
        "the replacement goal must be an unauthorized current/active current-schema goal",
    ),
    (
        "未知历史 receipt policy；当前只支持 {RECEIPT_POLICY_V1}",
        "unknown historical receipt policy; only {RECEIPT_POLICY_V1} is supported",
    ),
    (
        "本次新增 restore 目标已被第三方修改，拒绝删除: {}",
        "a restore target this run added was modified by a third party; refusing to delete it: {}",
    ),
    (
        "没有可恢复的 checkpoint",
        "there is no restorable checkpoint",
    ),
    (
        "测试注入：第 {} 个 restore 文件发布失败 ({})",
        "test injection: restore file {} failed to publish ({})",
    ),
    (
        "目标 success receipt 未通过当前或历史完整性复核: {}。仅对应 rollout 前历史可显式使用 --migrate-unreceipted 或 --migrate-receipt-policy {RECEIPT_POLICY_V1}",
        "the goal's success receipts did not pass current or historical integrity review: {}. Only the corresponding pre-rollout history may explicitly use --migrate-unreceipted or --migrate-receipt-policy {RECEIPT_POLICY_V1}",
    ),
    (
        "目标 {id} lifecycle={}，不能关闭；先用 `goal current {id}` 恢复为 current",
        "goal {id} has lifecycle={} and cannot be closed; restore it with `goal current {id}` first",
    ),
    (
        "目标 {id} lifecycle={}，不能写入 receipt；先用 `goal current {id}` 恢复为 current",
        "goal {id} has lifecycle={} and cannot take receipts; restore it with `goal current {id}` first",
    ),
    (
        "目标 {id} lifecycle={}，不能追加证据；先用 `goal current {id}` 恢复为 current",
        "goal {id} has lifecycle={} and cannot take more evidence; restore it with `goal current {id}` first",
    ),
    (
        "目标 {id} 不是当前 schema，不能写入可验证 receipt；请新建目标",
        "goal {id} is not current schema and cannot take a verifiable receipt; create a new goal",
    ),
    (
        "目标 {id} 不是当前 schema，不能记录 plan receipt",
        "goal {id} is not current schema and cannot record a plan receipt",
    ),
    (
        "目标 {id} 已关闭为 success，不能再追加人工证据；请用 `goal validate` 写入带 receipt 的验证，或先 supersede/archive",
        "goal {id} is already closed as success and cannot take more manual evidence; write a receipt-backed validation with `goal validate`, or supersede/archive it first",
    ),
    (
        "目标 {id} 已关闭为 success，不能降级为 {status}；请用新的 baseline-bound goal supersede，或将该记录 archive",
        "goal {id} is already closed as success and cannot be downgraded to {status}; supersede it with a new baseline-bound goal, or archive the record",
    ),
    (
        "目标 {id} 已隔离为 untrusted history；隔离是单向降级，审计记录必须保留，不能用 migration 刷新为可信历史",
        "goal {id} is quarantined as untrusted history; the quarantine is a one-way downgrade, the audit record must be retained, and a migration must not refresh it into trusted history",
    ),
    ("目标不能 supersede 自己", "a goal cannot supersede itself"),
    (
        "目标包含多个 plan receipt；拒绝继续使用可拆分绕过的计划状态",
        "the goal holds several plan receipts; refusing to keep using a plan state that can be bypassed by splitting",
    ),
    (
        "目标合约无效，不能 supersede: {error}",
        "the goal contract is invalid and cannot be superseded: {error}",
    ),
    (
        "目标合约无效，不能归档: {error}",
        "the goal contract is invalid and cannot be archived: {error}",
    ),
    (
        "目标合约无效，不能迁移 historical policy: {error}",
        "the goal contract is invalid and cannot migrate a historical policy: {error}",
    ),
    (
        "目标合约无效，不能隔离 historical receipt: {error}",
        "the goal contract is invalid and cannot quarantine a historical receipt: {error}",
    ),
    (
        "目标已满足当前 receipt policy，不需要降级迁移",
        "the goal already satisfies the current receipt policy; no downgrade migration is needed",
    ),
    (
        "目标已经是 untrusted history quarantine，不能重复隔离",
        "the goal is already an untrusted history quarantine and cannot be quarantined again",
    ),
    (
        "目标缺少开工 baseline，不能扩展 plan",
        "the goal has no starting baseline and cannot extend its plan",
    ),
    (
        "目标缺少开工 baseline；请新建目标后在首次修改前执行 goal plan",
        "the goal has no starting baseline; create a new goal and run goal plan before the first edit",
    ),
    (
        "被转移目标 {predecessor_id} 必须是 current 非 success current-schema goal",
        "transferred goal {predecessor_id} must be a current, non-success, current-schema goal",
    ),
    (
        "隔离原因不能为空",
        "the quarantine reason must not be empty",
    ),
    (
        "隔离后的 lifecycle proof 无效: {error}",
        "the lifecycle proof is invalid after quarantine: {error}",
    ),
    (
        "隔离后的目标合约无效: {error}",
        "the goal contract is invalid after quarantine: {error}",
    ),
    (
        "验证证据说明不能为空",
        "the validation evidence note must not be empty",
    ),
    (
        "无法确定用户数据目录",
        "unable to determine the user data directory",
    ),
    (
        "在 PATH 上只有本进程无法启动的",
        "is on PATH only as a file this process cannot launch:",
    ),
    (
        "—— rayman 用 `Command::new` 直接创建进程，Windows 只会补 `.exe`，不解析 PATHEXT；请把真正的",
        "— rayman spawns with `Command::new`, and Windows only appends `.exe` there, never consulting PATHEXT; put the real",
    ),
    (
        "所在目录加进 PATH（或改用提供 .exe 的安装方式）",
        "directory on PATH (or install a variant that ships a .exe)",
    ),
    // 整模板键：前缀条目已把句尾的「验证」语义并进 "verifies it with"，模板里
    // 残留的「验证」再被单独翻译一次就成了 "verifies it with verification with"。
    // 键更长者优先替换，所以整模板键必须存在才能压住前缀条目。
    (
        "仓库源码产物: 未由 doctor 检查；交接/CI 由 `{}` 验证",
        "Repository source artifact: not checked by doctor; handoff/CI verifies it with `{}`",
    ),
    (
        "未激活且没有自动保存状态，无需停止",
        "is not activated and has no autosave state; nothing to stop",
    ),
    (
        "停止状态写入失败且计划任务重注册失败：",
        "stop state write failed and scheduled-task re-registration failed: ",
    ),
    ("自动停止失败", "auto-stop failed"),
    ("嵌套 Cargo manifest 超过", "nested Cargo manifests exceed"),
    (
        "个，已停止逐个解析；把它们纳入同一个 workspace（根 Cargo.toml 的",
        "mutually independent packages; per-manifest resolution stopped. Put them in one workspace (the root Cargo.toml's",
    ),
    (
        "），或把 fixture manifest 排除出索引",
        "), or exclude the fixture manifests from the index",
    ),
    (
        "路径不能以引号开头或结尾（激活合同按未加引号的标量写入）",
        "path must not start or end with a quote (the activation contract is written as an unquoted scalar)",
    ),
    // Whole-template pairs: composing these two lines out of fragments left
    // run-together English ("currentworkspacehas no可restore …"), because
    // fragment substitution has no word-boundary protection.
    (
        "当前工作区没有可恢复的 standard 快照；另有 {other} 份 recovery-only/partial 快照，用 `rayman checkpoint list` 查看。",
        "This workspace has no restorable standard snapshot; there are also {other} recovery-only/partial snapshots — inspect them with `rayman checkpoint list`.",
    ),
    (
        "当前工作区没有可恢复的 standard 快照；另有",
        "This workspace has no restorable standard snapshot; there are also",
    ),
    (
        "份 recovery-only/partial 快照，用",
        "recovery-only/partial snapshots — inspect them with",
    ),
    ("查看。", "."),
    (
        "状态锁正被另一个 rayman 进程占用",
        "the state lock is held by another rayman process",
    ),
    (
        "已隔离为 untrusted history；隔离是单向降级，审计记录必须保留，不能恢复为",
        "is quarantined as untrusted history; the quarantine is a one-way downgrade, the audit record must be retained, and it cannot be restored to",
    ),
    (
        "未激活，跳过快照",
        "is not activated; the snapshot was skipped",
    ),
    (
        "未激活，最终快照已跳过",
        "is not activated; the final snapshot was skipped",
    ),
    (
        "未激活：已停止自动保存并注销计划任务",
        "is not activated: autosave stopped and its scheduled task was unregistered",
    ),
    (
        "。最终快照已跳过；如需抢救快照，运行",
        ". The final snapshot was skipped; to salvage a snapshot, run",
    ),
    ("无法检查被跟踪文件", "unable to inspect tracked file"),
    (
        "（含 typed proof 义务）",
        " (including typed proof obligations) ",
    ),
    (
        "验证命令不允许 shell 控制符或命令替换",
        "validation command does not allow shell control operators or command substitution",
    ),
    (
        "请直接提供要执行的程序及参数",
        "provide the executable and its arguments directly",
    ),
    (
        "请提供单一可执行程序及参数",
        "provide one executable and its arguments",
    ),
    (
        "测试命令成功退出但没有可验证的",
        "test command succeeded but has no verifiable",
    ),
    ("成功退出但没有可验证的", "succeeded but has no verifiable"),
    (
        "成功退出但缺少可验证的终端汇总",
        "succeeded but is missing a verifiable terminal summary",
    ),
    (
        "必须证明同一源码快照上的零退出执行与有效输出摘要",
        "must prove a zero-exit execution and valid output digest on the same source snapshot",
    ),
    (
        "不能作为咨询或等待边界",
        "cannot serve as a consultation or wait boundary",
    ),
    (
        "不能作为当前成功证据",
        "cannot serve as current success evidence",
    ),
    ("不能作为完成证明", "cannot serve as completion proof"),
    (
        "避免把影响面建议误当作已验证事实",
        "avoid treating impact suggestions as verified facts",
    ),
    (
        "其它平台请用系统定时器周期调用",
        "on other platforms, use the system scheduler to invoke",
    ),
    (
        "恢复会用快照覆盖工作区里的同名文件",
        "restore will overwrite same-named workspace files with the checkpoint",
    ),
    (
        "请复核范围是否仍属于同一目标",
        "review whether the scope still belongs to the same goal",
    ),
    (
        "建议分阶段绑定责任和恢复点",
        "consider binding responsibility and recovery points in stages",
    ),
    (
        "必须是同 workspace、current-policy 且包含同命令 direct-authority 的有效 archived success",
        "must be a valid archived success from the same workspace and current policy with direct authority for the same command",
    ),
    (
        "authority gate 必须是受检的",
        "authority gate must be an inspected",
    ),
    (
        "或无路径选择器的全工作区 pytest",
        "or full-workspace pytest without path selectors",
    ),
    (
        "且不得使用缩小运行范围的选择器",
        "and must not narrow the run with any selector",
    ),
    (
        "验证脚本不在当前工作区的受检文件集合中",
        "validation script is outside the inspected file set of the current workspace",
    ),
    (
        "一条真正验证该变更的命令",
        "a command that actually validates this change",
    ),
    (
        "非代码需求必须显式使用",
        "non-code requirements must explicitly use",
    ),
    (
        "代码构建/测试命令不能声明为",
        "code build/test commands cannot be declared as",
    ),
    (
        "必须实际运行至少一个测试",
        "must actually run at least one test",
    ),
    (
        "测试验证命令包含非执行模式",
        "test validation command contains non-execution mode",
    ),
    (
        "没有可验证的 passed>0 汇总",
        "has no verifiable passed>0 summary",
    ),
    ("没有收集任何测试", "collected no tests"),
    ("没有列出任何测试", "listed no tests"),
    ("拒绝混合/伪造输出", "refusing mixed or forged output"),
    (
        "之类的查询不是证据",
        "queries of this kind are not evidence",
    ),
    (
        "验证命令缺少可执行程序",
        "validation command is missing an executable",
    ),
    ("验证命令不能为空", "validation command cannot be empty"),
    (
        "验证命令不能启动 shell",
        "validation command cannot launch a shell",
    ),
    (
        "验证命令包含未闭合的引号",
        "validation command contains an unclosed quote",
    ),
    ("验证命令不覆盖", "validation command does not cover"),
    ("需要同一条当前成功", "requires the same current successful"),
    ("必须提供至少一个", "must provide at least one"),
    ("必须声明至少一个", "must declare at least one"),
    ("必须绑定实际", "must bind the actual"),
    (
        "需要保留未完成工作时使用",
        "to preserve unfinished work, use",
    ),
    ("需要", "requires"),
    ("出现在 stderr", "appears on stderr"),
    ("来源不可区分", "source is indistinguishable"),
    ("独立 test list proof", "independent test-list proof"),
    ("独立 list/collect proof", "independent list/collect proof"),
    ("独立 collect proof", "independent collect proof"),
    ("独立 list proof", "independent list proof"),
    ("独立路径参数", "independent path argument"),
    ("暂无独立", "does not yet support independent"),
    ("可用", "available"),
    ("脚本", "script"),
    ("命令", "command"),
    ("汇总", "summary"),
    ("证明", "proof"),
    ("声明", "declare"),
    ("生成", "generate"),
    ("收集", "collect"),
    ("列出", "list"),
    ("受检", "inspected"),
    ("选择器", "selector"),
    ("非执行模式", "non-execution mode"),
    ("测试", "test"),
    ("程序及参数", "program and arguments"),
    (
        "控制符或命令替换",
        "control operators or command substitution",
    ),
    ("引号", "quote"),
    ("执行程序", "executable"),
    ("疑似过时文件名后缀", "suspected stale filename suffix"),
    ("疑似过时命名标记", "suspected stale naming marker"),
    (
        "资产扫描无法读取文件元数据",
        "asset scan unable to read file metadata",
    ),
    ("资产扫描无法读取文件", "asset scan unable to read file"),
    (
        "注册成功后回滚计划任务失败",
        "scheduled-task rollback failed after successful registration",
    ),
    (
        "自动保存失败状态也未能写入",
        "autosave failure state also could not be written",
    ),
    ("注销计划任务失败", "scheduled-task unregistration failed"),
    ("注册计划任务失败", "scheduled-task registration failed"),
    ("无自动保存状态", "no autosave state"),
    (
        "也没有已注册的计划任务",
        "and no scheduled task is registered",
    ),
    (
        "也未注销计划任务",
        "and did not unregister the scheduled task",
    ),
    ("检测到全部目标均为", "detected that all goals are"),
    ("任务仍可能在运行", "the task may still be running"),
    ("任务未找到", "task not found"),
    ("找不到指定的文件", "the specified file was not found"),
    ("找不到指定的路径", "the specified path was not found"),
    ("未知 proof kind", "unknown proof kind"),
    ("指定的任务不存在", "the specified task does not exist"),
    ("找不到任务", "task not found"),
    ("已存初始快照", "saved initial checkpoint"),
    (
        "已存最后一次快照并停止自动保存",
        "saved the final checkpoint and stopped autosave",
    ),
    ("已存快照", "saved checkpoint"),
    ("最近一次触发", "latest trigger"),
    ("连续失败", "consecutive failures"),
    ("完成后自动停止", "stop automatically after completion"),
    ("尚无", "none yet"),
    ("最近一次", "latest"),
    ("最近错误", "latest error"),
    ("回滚", "rollback"),
    ("重新注册", "re-register"),
    ("检测到", "detected"),
    ("均为", "are all"),
    ("最终", "final"),
    ("停止状态已写入", "stop state was written"),
    ("停止状态写入失败", "stop-state write failed"),
    ("但最终", "but the final"),
    (
        "但停止状态写入失败且重新注册失败",
        "but stop-state write and re-registration failed",
    ),
    ("但", "but"),
    ("且", "and"),
    ("也没有", "and has no"),
    ("也未", "also did not"),
    ("尚", "not yet"),
    ("已存", "saved"),
    ("注册", "register"),
    ("注销", "unregister"),
    ("任务", "task"),
    ("触发", "trigger"),
    ("轮换", "rotate"),
    ("份更旧的", "older"),
    ("供取证", "for forensics"),
    (
        "不会替代最近完整快照",
        "will not replace the latest complete checkpoint",
    ),
    (
        "保存后完整性验证失败",
        "post-save integrity validation failed",
    ),
    ("保存不完整", "save is incomplete"),
    ("已轮换掉", "rotated out"),
    ("快照轮换失败", "checkpoint rotation failed"),
    (
        "发现不属于工作区根的候选文件",
        "found a candidate file outside the workspace root",
    ),
    (
        "无法创建 checkpoint 暂存目录",
        "unable to create checkpoint staging directory",
    ),
    (
        "无法创建 checkpoint 树目录",
        "unable to create checkpoint tree directory",
    ),
    ("暂存目录", "staging directory"),
    ("树目录", "tree directory"),
    ("已保存 recovery-only", "saved recovery-only"),
    (
        "它不会成为默认 latest 或完成证据",
        "it will not become the default latest or completion evidence",
    ),
    ("缺或损坏", "missing or corrupt"),
    ("确认请加", "add this flag to confirm"),
    ("已从快照", "restored from checkpoint"),
    ("已验证", "verified"),
    ("恢复", "restore"),
    ("同名", "same-named"),
    ("默认", "default"),
    ("损坏", "corrupt"),
    (
        "原子写入目标没有父目录",
        "atomic-write destination has no parent directory",
    ),
    (
        "原子复制目标没有父目录",
        "atomic-copy destination has no parent directory",
    ),
    (
        "无法创建原子复制临时文件",
        "unable to create atomic-copy temporary file",
    ),
    ("无法复制到临时文件", "unable to copy to the temporary file"),
    (
        "无法同步原子复制临时文件",
        "unable to sync the atomic-copy temporary file",
    ),
    (
        "无法校验原子复制临时文件",
        "unable to verify the atomic-copy temporary file",
    ),
    ("无法写入临时文件", "unable to write the temporary file"),
    ("临时文件", "temporary file"),
    ("原子复制", "atomic copy"),
    ("原子写入", "atomic write"),
    ("同步", "sync"),
    ("校验", "verify"),
    ("调用方", "caller"),
    ("规划检查", "plan inspection"),
    (
        "实际变更未被当前",
        "actual changes were not declared by the current",
    ),
    ("实际变更超出", "actual changes exceed"),
    ("实际变更", "actual changes"),
    ("缺少绑定最终源码", "is missing a binding to final source"),
    ("缺少开工", "is missing the starting"),
    ("不能作为", "cannot serve as"),
    ("请用新的", "use a new"),
    (
        "或将已完成记录显式 archive",
        "or explicitly archive the completed record",
    ),
    ("记录不能生成当前", "record cannot produce a current"),
    (
        "请显式 archive 历史 success",
        "explicitly archive the historical success",
    ),
    ("或新建", "or create a new"),
    ("无法在缺少", "unable to validate without"),
    ("时验证", "during validation"),
    ("仍为", "remains"),
    (
        "没有绑定当前工作区的成功",
        "has no successful receipt bound to the current workspace",
    ),
    ("非代码变更可忽略", "non-code changes may omit this"),
    ("不能保留", "cannot retain"),
    ("不能设置", "cannot set"),
    ("必须记录非空", "must record non-empty"),
    ("不支持的", "unsupported"),
    ("当前只接受", "currently accepts only"),
    ("请迁移或重新创建目标", "migrate or recreate the goal"),
    ("或标题为空", "or title is empty"),
    ("包含空的", "contains an empty"),
    ("包含重复", "contains duplicate"),
    ("尚未关闭", "is not closed"),
    ("不能携带", "cannot carry"),
    ("结构或摘要无效", "structure or digest is invalid"),
    (
        "结构或源码绑定无效",
        "structure or source binding is invalid",
    ),
    ("包含非法摘要", "contains an illegal digest"),
    (
        "使用了无效的历史迁移",
        "uses an invalid historical migration",
    ),
    ("使用了无效的", "uses an invalid"),
    (
        "缺少受控迁移标记",
        "is missing a controlled-migration marker",
    ),
    ("历史化时的", "at archival time"),
    (
        "来自不同 workspace identity",
        "comes from a different workspace identity",
    ),
    ("已过期", "is stale"),
    (
        "或专用迁移形态无效",
        "or dedicated migration shape is invalid",
    ),
    ("被转移目标不存在", "transferred goal does not exist"),
    ("被转移目标", "transferred goal"),
    ("与被转移目标", "and the transferred goal"),
    ("的精确并集不一致", "exact union is inconsistent"),
    (
        "或带有效 proof 的 archived success",
        "or an archived success with valid proof",
    ),
    ("必须先", "must first"),
    ("尚未 gate-ready", "is not gate-ready"),
    ("未完整转移到", "was not fully transferred to"),
    ("已失效", "is no longer valid"),
    ("被替代", "superseded"),
    ("迁移", "migration"),
    ("摘要", "digest"),
    ("有效", "valid"),
    ("标题", "title"),
    ("空", "empty"),
    ("非法", "illegal"),
    ("显式", "explicit"),
    ("历史", "historical"),
    ("父子图包含环", "parent-child graph contains a cycle"),
    ("父节点不存在", "parent node does not exist"),
    ("图无效", "graph is invalid"),
    ("缺少完成时间", "is missing completion time"),
    ("缺少进度收据", "is missing a progress receipt"),
    ("进度收据引用无效", "progress receipt reference is invalid"),
    ("含完成态字段", "contains completion-only fields"),
    ("标记无效", "marker is invalid"),
    ("含关闭态字段", "contains closed-only fields"),
    ("关闭证明无效", "close proof is invalid"),
    ("发生源码漂移", "changed source"),
    ("计划包含", "plan contains"),
    ("个路径但没有", "paths but no"),
    ("个路径但尚无", "paths but no"),
    ("计划已扩展", "plan was extended"),
    ("且标题不能为空", "and title cannot be empty"),
    ("目标可以增加", "goals may add"),
    ("目标可以记录", "goals may record"),
    ("不能追加", "cannot append"),
    (
        "写入前源码快照发生漂移",
        "source snapshot drifted before write",
    ),
    ("目标可以完成", "goals may complete"),
    ("目标可以打开", "goals may open"),
    ("目标可以关闭", "goals may close"),
    ("不接受", "does not accept"),
    ("检测到源码漂移", "detected source drift"),
    (
        "关闭检查期间源码快照发生漂移",
        "source snapshot drifted during close inspection",
    ),
    ("关闭被拒绝", "close was refused"),
    ("父", "parent"),
    ("进度收据", "progress receipt"),
    ("责任", "responsibility"),
    ("恢复点", "recovery point"),
    ("范围", "scope"),
    ("增加", "add"),
    ("追加", "append"),
    ("打开", "open"),
    ("关闭", "close"),
    ("人工", "human"),
    ("后台继续必须绑定", "background continuation must bind"),
    ("并同时记录非空", "and also record non-empty"),
    (
        "不能伪装成人工/外部边界",
        "cannot masquerade as a human/external boundary",
    ),
    ("不能包含空字符串", "cannot contain an empty string"),
    (
        "缺少完整 solution package",
        "is missing a complete solution package",
    ),
    ("咨询或等待边界", "consultation or wait boundary"),
    ("合同无效", "contract is invalid"),
    ("重复 work package id", "duplicate work-package id"),
    ("重复", "duplicate"),
    ("字段", "field"),
    ("顶层标量", "top-level scalars"),
    ("分隔符", "separator"),
    (
        "行包含不受支持的缩进",
        "line contains unsupported indentation",
    ),
    ("行包含未知字段", "line contains an unknown field"),
    ("行包含重复字段", "line contains a duplicate field"),
    (
        "行包含未闭合或不匹配的引号",
        "line contains an unclosed or mismatched quote",
    ),
    ("行缺少值", "line is missing a value"),
    ("行缺少", "line is missing"),
    ("受管状态存在", "managed state exists"),
    ("但缺少显式", "but is missing explicit"),
    (
        "激活合同只接受顶层标量",
        "activation contract accepts only top-level scalars",
    ),
    ("激活合同", "activation contract"),
    ("不是普通非链接文件", "is not a regular non-linked file"),
    ("必须是普通非链接文件", "must be a regular non-linked file"),
    ("当前内容不一致", "current contents are inconsistent"),
    ("工作区未显式激活", "workspace is not explicitly activated"),
    ("状态不会自动激活", "state does not automatically activate"),
    ("路径不能包含换行", "path cannot contain a newline"),
    (
        "写入后的工作区激活合同仍无效",
        "workspace activation contract remains invalid after writing",
    ),
    ("允许的状态项", "allowed state item"),
    ("未知的允许状态项", "unknown allowed state item"),
    ("不安全或无效", "unsafe or invalid"),
    (
        "受管状态包含退役条目或遍历错误",
        "managed state contains retired entries or traversal errors",
    ),
    (
        "目标目录含不可安全读取的记录",
        "goal directory contains records that cannot be read safely",
    ),
    (
        "不安全的受管状态相对路径",
        "unsafe managed-state relative path",
    ),
    (
        "受管状态路径不是普通文件",
        "managed-state path is not a regular file",
    ),
    ("拓扑未获", "topology lacks"),
    ("权威确认", "authoritative confirmation"),
    ("项目地图中没有文件", "project map has no file"),
    (
        "不在已验证上下文索引中",
        "is absent from the verified context index",
    ),
    ("上下文索引", "context index"),
    ("模块", "module"),
    ("直接依赖", "direct dependencies"),
    ("交接/CI 必须运行", "handoff/CI must run"),
    ("交接", "handoff"),
    ("工作区门禁使用", "workspace gate uses"),
    ("任务交付使用", "task delivery uses"),
    ("状态卫生使用", "state hygiene uses"),
    ("更新内容索引", "refresh the content index"),
    ("验证任务", "validate the task"),
    ("已退役且", "is retired and"),
    ("不维护", "does not maintain"),
    ("工作", "work"),
    ("要求绑定唯一", "requires binding the unique"),
    ("要求绑定", "requires binding"),
    ("但当前没有", "but there is no current"),
    ("但当前有", "but there are currently"),
    ("要求当前稳定", "requires current stable"),
    ("必须完成验证并", "must complete validation and"),
    ("已执行并记录", "executed and recorded"),
    ("的可验证", "verifiable"),
    ("已归档", "archived"),
    ("已获", "obtained"),
    ("已由", "was"),
    ("取代", "replaced by"),
    ("已恢复为", "restored as"),
    ("次修改了工作区内容", "modified workspace contents"),
    ("次修改了工作区", "modified the workspace"),
    ("以证明稳定固定点", "to prove a stable fixed point"),
    ("重复执行只用于", "repeated execution is only for"),
    ("请同时传", "also pass"),
    ("请显式传", "explicitly pass"),
    ("请显式", "explicitly"),
    ("读写探针", "read/write probe"),
    ("探针通过", "probe passed"),
    ("写探针失败", "write probe failed"),
    ("读探针失败", "read probe failed"),
    ("探针内容不一致", "probe contents differ"),
    ("清理探针失败", "probe cleanup failed"),
    ("子目录", "subdirectory"),
    ("临时条目类型", "temporary-entry type"),
    ("临时条目", "temporary entry"),
    ("临时目录", "temporary directory"),
    (
        "拒绝遍历链接/reparse",
        "refusing to traverse linked/reparse",
    ),
    ("不支持", "unsupported"),
    ("遍历不完整", "traversal is incomplete"),
    ("不会被跟随", "will not be followed"),
    ("链接/reparse", "linked/reparse"),
    ("非链接", "non-linked"),
    ("行", "line"),
    ("值", "value"),
    ("缩进", "indentation"),
    ("集合", "set"),
    ("内容", "content"),
    ("卫生", "hygiene"),
    ("门禁", "gate"),
    ("可执行", "executable"),
    ("修改", "modify"),
    ("稳定固定点", "stable fixed point"),
    ("同一条", "the same"),
    ("没有", "has no"),
    ("提供", "provide"),
    ("同时", "simultaneously"),
    ("保持", "keep"),
    ("仍", "still"),
    ("请用", "use"),
    ("是否在", "is in"),
    ("根", "root"),
    ("候选", "candidate"),
    ("更旧", "older"),
    ("最近完整", "latest complete"),
    ("完整", "complete"),
    ("初始", "initial"),
    ("确认", "confirm"),
    ("探针", "probe"),
    ("平台", "platform"),
    (
        "系统定时器周期调用",
        "invoke periodically through the system scheduler",
    ),
    ("工作区遍历不完整", "workspace traversal is incomplete"),
    (
        "拒绝把不完整结果当作完整文件集",
        "refusing to treat incomplete results as a complete file set",
    ),
    (
        "自动计划任务目前仅支持 Windows",
        "automatic scheduled tasks are currently supported only on Windows",
    ),
    (
        "自动保存状态损坏或不可读取",
        "autosave state is corrupt or unreadable",
    ),
    (
        "完成要求同包且绑定当前源码快照的",
        "completion requires a same-package receipt bound to the current source snapshot",
    ),
    (
        "完成写入前源码快照发生漂移",
        "source snapshot drifted before completion was recorded",
    ),
    (
        "验证脚本必须是工作区内的普通",
        "validation script must be a regular workspace-local",
    ),
    (
        "只能携带一个不可拆分的聚合",
        "may carry only one indivisible aggregate",
    ),
    (
        "只接受普通工作区相对文件路径",
        "accepts only regular workspace-relative file paths",
    ),
    (
        "共享 quality policy 的父目录",
        "the shared quality policy parent directory",
    ),
    (
        "共享 quality policy 必须是工作区内普通文件",
        "shared quality policy must be a regular workspace-local file",
    ),
    (
        "共享 quality policy 逃逸工作区或不在精确 policy 目录",
        "shared quality policy escapes the workspace or is outside the exact policy directory",
    ),
    (
        "原子复制临时文件完整性不匹配",
        "atomic-copy temporary file integrity mismatch",
    ),
    (
        "原子复制父目录不安全或不存在",
        "atomic-copy parent directory is unsafe or missing",
    ),
    (
        "原子复制发布前父目录不安全",
        "atomic-copy parent directory became unsafe before publish",
    ),
    (
        "原子写入父目录不安全或不存在",
        "atomic-write parent directory is unsafe or missing",
    ),
    (
        "原子发布前父目录不安全",
        "parent directory became unsafe before atomic publish",
    ),
    (
        "长任务存在证据悬崖",
        "long-running task has an evidence cliff",
    ),
    (
        "无效或未绑定当前源码",
        "invalid or not bound to the current source",
    ),
    (
        "未证明重复稳定执行或摘要无效",
        "stable repeated execution is unproven or the digest is invalid",
    ),
    (
        "未显式绑定被替代目标",
        "does not explicitly bind the superseded goal",
    ),
    ("未规范化或未绑定", "is not normalized or bound"),
    ("计划任务已注册", "scheduled task registered"),
    ("计划任务未注册", "scheduled task not registered"),
    ("遗留计划任务已注销", "legacy scheduled task unregistered"),
    (
        "状态已尝试回滚但计划任务重注册失败",
        "state rollback was attempted but scheduled-task registration failed",
    ),
    (
        "状态写入失败且回滚失败",
        "state write and rollback both failed",
    ),
    (
        "只允许字母、数字、下划线和连字符",
        "only letters, digits, underscores, and hyphens are allowed",
    ),
    ("只允许字母", "only letters are allowed"),
    ("下划线和连字符", "underscores and hyphens"),
    (
        "连续 {MAX_TEMP_NAME_ATTEMPTS} 个名称已存在",
        "{MAX_TEMP_NAME_ATTEMPTS} consecutive names already exist",
    ),
    (
        "连续 {MAX_NAME_ATTEMPTS} 个名称已存在",
        "{MAX_NAME_ATTEMPTS} consecutive names already exist",
    ),
    (
        "无法为原子复制创建独占临时文件",
        "unable to create an exclusive atomic-copy temporary file",
    ),
    (
        "无法为原子写入创建独占临时文件",
        "unable to create an exclusive atomic-write temporary file",
    ),
    (
        "无法为计划任务 XML 创建独占临时文件",
        "unable to create an exclusive scheduled-task XML temporary file",
    ),
    (
        "无法读取原子复制源元数据",
        "unable to read atomic-copy source metadata",
    ),
    (
        "无法读取原子复制源",
        "unable to read the atomic-copy source",
    ),
    (
        "无法原子替换复制目标",
        "unable to atomically replace the copy destination",
    ),
    ("无法原子替换文件", "unable to atomically replace the file"),
    ("无法保留复制权限", "unable to preserve copied permissions"),
    ("无法同步父目录", "unable to sync the parent directory"),
    (
        "无法独占创建计划任务 XML",
        "unable to exclusively create scheduled-task XML",
    ),
    ("无法写入计划任务 XML", "unable to write scheduled-task XML"),
    (
        "无法复查计划任务 XML",
        "unable to recheck scheduled-task XML",
    ),
    (
        "无法启动 schtasks 查询 autosave 计划任务",
        "unable to start schtasks to query the autosave task",
    ),
    (
        "无法读取工作区激活合同",
        "unable to read the workspace activation contract",
    ),
    (
        "无法读取受管状态目录",
        "unable to read the managed state directory",
    ),
    (
        "无法读取受管状态文件",
        "unable to read the managed state file",
    ),
    (
        "无法读取受管状态根",
        "unable to read the managed state root",
    ),
    (
        "无法规范化受管状态目录",
        "unable to canonicalize the managed state directory",
    ),
    (
        "无法创建受管状态目录",
        "unable to create the managed state directory",
    ),
    (
        "无法读取目标状态目录条目",
        "unable to read a goal-state directory entry",
    ),
    (
        "无法遍历目标状态目录",
        "unable to traverse the goal-state directory",
    ),
    (
        "无法读取目标状态目录",
        "unable to read the goal-state directory",
    ),
    ("无法读取状态目录", "unable to read the state directory"),
    (
        "无法规范化工作区根",
        "unable to canonicalize the workspace root",
    ),
    (
        "无法计算工作区内容指纹",
        "unable to compute the workspace content fingerprint",
    ),
    (
        "无法计算 goal 实际变更集",
        "unable to compute the goal's actual change set",
    ),
    (
        "无法读取允许的状态文件",
        "unable to read an allowed state file",
    ),
    (
        "无法安全读取受管状态",
        "unable to safely read managed state",
    ),
    ("无法读取当前目录", "unable to read the current directory"),
    ("无法读取被转移目标", "unable to read the transferred goal"),
    (
        "无法执行验证程序",
        "unable to execute the validation program",
    ),
    (
        "无法执行 cargo metadata",
        "unable to execute cargo metadata",
    ),
    (
        "无法解析 cargo metadata JSON",
        "unable to parse cargo metadata JSON",
    ),
    ("无法序列化输出", "unable to serialize output"),
    ("无法序列化 JSON", "unable to serialize JSON"),
    ("无法确定文件类型", "unable to determine the file type"),
    ("无法验证共享", "unable to verify shared"),
    ("无法检查共享", "unable to inspect shared"),
    (
        "无法创建受管临时目录",
        "unable to create the managed temp directory",
    ),
    ("无法释放", "unable to release"),
    (
        "无法清理临时目录",
        "unable to clean the temporary directory",
    ),
    ("无法哈希", "unable to hash"),
    ("无法规范化", "unable to canonicalize"),
    ("无法复算", "unable to recompute"),
    ("无法复制", "unable to copy"),
    ("无法提交", "unable to commit"),
    ("无法创建", "unable to create"),
    ("无法读取", "unable to read"),
    ("无法打开", "unable to open"),
    ("无法检查", "unable to inspect"),
    ("无法复查", "unable to recheck"),
    ("无法取得", "unable to acquire"),
    ("无法解析", "unable to parse"),
    ("无法执行", "unable to execute"),
    ("无法写入", "unable to write"),
    ("无法", "unable to "),
    ("等待 autosave 独占锁超过", "autosave lock wait exceeded"),
    ("等待 checkpoint 锁超过", "checkpoint lock wait exceeded"),
    ("等待锁超过", "lock wait exceeded"),
    ("拒绝链接/reparse", "refusing linked/reparse"),
    (
        "不能是链接/reparse/非常规文件",
        "cannot be linked/reparse/non-regular",
    ),
    ("不能是链接", "cannot be linked"),
    ("不是安全普通文件", "is not a safe regular file"),
    ("不是普通文件", "is not a regular file"),
    ("不是目录", "is not a directory"),
    (
        "写入后被替换或截断",
        "was replaced or truncated after writing",
    ),
    (
        "源文件在 checkpoint 复制期间发生变化",
        "source file changed during checkpoint copy",
    ),
    (
        "复制后的文件与源文件完整性不一致",
        "copied-file integrity differs from the source",
    ),
    (
        "目录在枚举后消失",
        "directory disappeared after enumeration",
    ),
    ("文件在枚举后消失", "file disappeared after enumeration"),
    ("越出允许路径", "escapes the allowed path"),
    ("逃逸工作区", "escapes the workspace"),
    ("拒绝覆盖", "refusing to overwrite"),
    ("暂存目录已存在", "staging directory already exists"),
    ("个名称已存在", "names already exist"),
    ("只读", "read-only"),
    ("非常规文件", "non-regular file"),
    ("符号链接", "symbolic link"),
    ("来源", "source"),
    ("授权", "authority"),
    ("验证命令第", "validation command run "),
    ("次运行前", " before run"),
    ("次失败", " run failed"),
    ("不会写入", "will not record"),
    ("源码快照已过期", "source snapshot is stale"),
    ("源码快照发生漂移", "source snapshot drifted"),
    ("漂移", "drifted"),
    ("不匹配", "does not match"),
    ("不一致", "is inconsistent"),
    ("无效", "is invalid"),
    ("不存在", "does not exist"),
    ("不可读取", "is unreadable"),
    ("不可区分", "is indistinguishable"),
    ("不可用", "is unavailable"),
    ("已退役", "is retired"),
    ("未完成或缺少", "is incomplete or missing"),
    ("未处于", "is not in"),
    ("未被", "was not"),
    ("未设置", "is not set"),
    ("未记录", "was not recorded"),
    ("未注册", "not registered"),
    ("未证明", "is unproven"),
    ("未完成", "incomplete"),
    ("待完成", "pending"),
    ("待办", "to-do"),
    ("缺少", "is missing"),
    ("不能为空", "cannot be empty"),
    ("都不能为空", "cannot both be empty"),
    ("不能是空字符串", "cannot be an empty string"),
    ("必须精确为", "must equal exactly"),
    ("必须记录", "must record"),
    ("必须在", "must be within"),
    ("必须为", "must be"),
    ("必须是", "must be"),
    ("必须", "must"),
    ("只有", "only"),
    ("只允许", "allows only"),
    ("只能显式", "can only explicitly"),
    ("只接受", "accepts only"),
    ("至少提供一个变更路径", "provide at least one changed path"),
    ("至少需要一个", "requires at least one"),
    ("使用", "use"),
    ("先审阅", "review first"),
    ("运行中", "running"),
    ("运行", "run"),
    ("已完成", "completed"),
    ("已记录", "recorded"),
    ("已创建", "created"),
    ("已关闭", "closed"),
    ("已保存", "saved"),
    ("已停止", "stopped"),
    ("已注册", "registered"),
    ("已注销", "unregistered"),
    ("已存在", "already exists"),
    ("失败", "failed"),
    ("成功", "success"),
    ("未知时间", "unknown time"),
    ("未知", "unknown"),
    ("当前", "current"),
    ("绑定的", "bound"),
    ("绑定", "bind"),
    ("目标", "goal"),
    ("需求", "requirement"),
    ("证据", "evidence"),
    ("合约", "contract"),
    ("计划任务", "scheduled task"),
    ("自动保存", "autosave"),
    ("快照", "checkpoint"),
    ("工作区", "workspace"),
    ("受管状态", "managed state"),
    ("状态文件", "state file"),
    ("状态", "state"),
    ("文件清单", "file list"),
    ("文件", "file"),
    ("目录", "directory"),
    ("路径", "path"),
    ("元数据", "metadata"),
    ("项目地图", "project map"),
    ("项目拓扑", "project topology"),
    ("直接依赖方", "direct dependents"),
    ("依赖方", "dependents"),
    ("依赖", "dependencies"),
    ("风险", "risk"),
    ("错误", "error"),
    ("范围内", "range"),
    ("秒", "seconds"),
    ("分钟", "minutes"),
    ("次", "times"),
    ("个文件", "files"),
    ("个错误", "errors"),
    ("个失败", "failures"),
    ("个", ""),
    ("第", "run "),
    ("自身", "itself"),
    ("全部", "all"),
    ("每", "every "),
    ("外部边界", "external boundary"),
    ("中", " in "),
    ("与", " and "),
    ("或", " or "),
    ("的", " "),
    ("为", " as "),
    ("出现", "appears"),
    ("指向", "points to"),
    ("包含", "contains"),
    ("完成", "complete"),
    ("删除", "removed"),
    ("保留", "kept"),
    ("覆盖", "overwrite"),
    ("检查", "inspect"),
    ("写入", "write"),
    ("读取", "read"),
    ("创建", "create"),
    ("解析", "parse"),
    ("计算", "compute"),
    ("规范化", "canonicalize"),
    ("验证", "verify"),
    ("清理", "clean"),
    ("释放", "release"),
    ("启动", "start"),
    ("停止", "stop"),
    ("拒绝", "refuse"),
    ("安全", "safe"),
    ("普通", "regular"),
    ("真实", "real"),
    ("独占锁", "exclusive lock"),
    ("锁", "lock"),
    ("计划", "plan"),
    ("文本", "text"),
    ("项", "items"),
    ("数字", "digits"),
    ("下划线", "underscores"),
    ("连字符", "hyphens"),
    ("；", "; "),
    ("：", ": "),
    ("，", ", "),
    ("。", "."),
    ("（", " ("),
    ("）", ")"),
    ("？", "?"),
];

const MESSAGE_FRAGMENT_CATALOG: &[(&str, &str)] = &[
    (
        "无法写入 checkpoint 根目录（默认在用户目录）",
        "cannot write the checkpoint root (defaults to the user profile)",
    ),
    (
        "指定工作区内目录，或以主机权限重试",
        "to pick a directory inside the workspace, or retry with host permission",
    ),
    (
        "写入被拒或探测失败（权限或 ACL）",
        "write denied or probe failed (permission or ACL)",
    ),
    (
        "状态目录不存在，未探测",
        "state directory absent, not probed",
    ),
    ("受限沙箱下用", "under a restricted sandbox use"),
    ("状态写探针", "state-write probe"),
    ("激活元数据写探针", "activation-metadata write probe"),
    (
        "原授权元数据 staging 已验证，激活文件未变",
        "authorization-metadata staging verified; activation unchanged",
    ),
    (
        "无激活合同或平台不支持，未探测",
        "activation absent or platform unsupported, not probed",
    ),
    (
        "workspace 激活合同结构上可 rebind",
        "workspace activation contract is structurally rebindable",
    ),
    (
        "且当前 activation metadata staging 探针已就绪",
        "and the current activation-metadata staging probe is ready",
    ),
    ("可写", "writable"),
    (
        "运行 `rayman context refresh`",
        "run `rayman context refresh`",
    ),
    (
        "未启用自动保存。运行 `rayman autosave start` 开启。",
        "Autosave is disabled. Run `rayman autosave start` to enable it.",
    ),
    (
        "`scripts/verify-release-contract.ps1 -RequireSourceFresh` 验证",
        "verification with `scripts/verify-release-contract.ps1 -RequireSourceFresh`",
    ),
    (
        "查询 autosave 计划任务失败，不能把未知状态当作未注册",
        "failed to query the autosave scheduled task; unknown state cannot be treated as unregistered",
    ),
    ("项目地图中没有文件", "project map has no file"),
    (
        "找不到可验证的 checkpoint",
        "no verifiable checkpoint was found",
    ),
    (
        "无法定位当前 rayman 二进制",
        "unable to locate the running rayman binary",
    ),
    ("新建目标", "Create a goal"),
    ("must 需求（可重复）", "Must requirement (repeatable)"),
    ("should 需求（可重复）", "Should requirement (repeatable)"),
    ("列出目标", "List goals"),
    ("查看单个目标", "Show one goal"),
    (
        "紧凑显示需求、计划、工作包和收据计数，不输出完整 baseline",
        "Compact requirement, plan, package, and receipt counts without the full baseline",
    ),
    ("管理分层 work package", "Manage hierarchical work packages"),
    (
        "管理源码绑定的并发 lane 台账",
        "Manage source-bound concurrency lanes",
    ),
    (
        "执行阶段检查并记录非权威 progress receipt",
        "Run a stage check and record a non-authoritative progress receipt",
    ),
    (
        "记录尚未被机器验证的进展说明并标记需求完成（evidence-only completion，不能支撑门禁主张）",
        "Record unverified evidence-only progress; it cannot support a gate claim",
    ),
    (
        "本次证据涉及的变更文件；会记录 map impact 快照（可重复）",
        "Changed file covered by this evidence (repeatable; records map impact)",
    ),
    (
        "声称已运行并通过的验证命令；无 receipt，不能支撑 standard/release 主张（可重复）",
        "Claimed validation command without a receipt; cannot support standard/release (repeatable)",
    ),
    (
        "实际执行一条验证命令并把 exit code、输出摘要和工作区指纹写成 receipt",
        "Execute validation and record exit code, output hashes, and workspace fingerprint",
    ),
    (
        "本次验证覆盖的变更文件；会记录 map impact 快照（可重复）",
        "Changed file covered by validation (repeatable; records map impact)",
    ),
    (
        "明确声明这是非代码需求；与 --changed 互斥",
        "Declare a non-code requirement; conflicts with --changed",
    ),
    (
        "作为单一程序 + argv 直接执行；拒绝 shell 控制符，非零退出不会写入 receipt",
        "Execute one direct program plus argv; reject shell control and nonzero receipts",
    ),
    (
        "关闭目标（success 要求每个 must 需求带 `goal validate` 写入的当前 receipt；仅有证据只能关成 partial/blocked）",
        "Close a goal; success requires current goal-validate receipts for every must",
    ),
    (
        "将历史目标显式归档；保留 JSON，但不再参与 readiness",
        "Archive a historical goal while retaining JSON outside readiness",
    ),
    (
        "以 archived authority 的同一 gate 在当前源码重跑，为精确 must 转移授权",
        "Rerun an archived authority gate to authorize exact must transfer",
    ),
    (
        "每个待替代的 current 非 success goal；可重复",
        "Current non-success goal to supersede (repeatable)",
    ),
    (
        "同 workspace 上带 direct stable authority 的 archived success",
        "Archived success with direct stable authority in the same workspace",
    ),
    (
        "标记旧目标已由另一个 current 目标取代",
        "Mark a goal superseded by another current goal",
    ),
    (
        "不带 id 时列出 current 目标；带 id 时把该目标恢复为 current",
        "List current goals or restore the specified goal to current",
    ),
    (
        "新增一个 package；父节点必须已存在",
        "Add a package whose parent already exists",
    ),
    (
        "用同包且绑定当前源码快照的 progress receipt 完成 package",
        "Complete a package from a same-package source-bound progress receipt",
    ),
    (
        "在当前源码 baseline 上打开一个 lane",
        "Open a lane on the current source baseline",
    ),
    (
        "计算 lane 期间的源码差量并按 mode/allowlist 机械验收",
        "Close a lane by mechanically checking delta against mode/allowlist",
    ),
    (
        "报告 v2 允许状态、退役目录和递归 temp 指标",
        "Report allowed v2 state, retired entries, and recursive temp metrics",
    ),
    (
        "发现退役状态或遍历错误时以非零退出",
        "Exit nonzero on retired state or traversal errors",
    ),
    (
        "在托管临时根下创建具名子目录",
        "Create a named directory under managed temp",
    ),
    (
        "创建可探测、可归因且源码排除的 pytest 临时租约",
        "Create a probed, attributable, source-excluded pytest lease",
    ),
    (
        "重新探测现有 pytest lease 的路径与读写能力",
        "Re-probe paths and write access for an existing pytest lease",
    ),
    (
        "按 manifest 精确释放一个 pytest lease",
        "Release exactly one manifest-owned pytest lease",
    ),
    ("清理整个托管临时根", "Clean the managed temp root"),
    (
        "已安装身份不一致时以非零退出；源码新鲜度须用 verify-release-contract.ps1 -RequireSourceFresh",
        "Exit nonzero on installed identity mismatch; use verify-release-contract.ps1 -RequireSourceFresh for source freshness",
    ),
    (
        "开工：存一次初始快照并注册计划任务（幂等，每次开工跑一遍即可）",
        "Start autosave: create an initial checkpoint and idempotently register the scheduled task",
    ),
    (
        "自动保存间隔（分钟，默认 30）",
        "Autosave interval in minutes (default 30)",
    ),
    (
        "保留最近 N 个快照（默认 3）",
        "Keep the latest N checkpoints (default 3)",
    ),
    (
        "关闭“完成后自动停止”（默认开启：所有目标关闭且无待完成项时自动收尾）",
        "Disable automatic stop after all goals close and pending work is empty",
    ),
    (
        "快照根目录（默认用户级）",
        "Checkpoint root (user-level by default)",
    ),
    (
        "计划任务触发时跑：存一次快照，必要时自动收尾（一般不手动调用）",
        "Scheduled autosave tick; creates a checkpoint and stops when appropriate",
    ),
    (
        "目标工作区（计划任务会传绝对路径；缺省则从当前目录向上找）",
        "Target workspace (absolute for scheduled tasks; otherwise discovered from cwd)",
    ),
    (
        "全部完成或出错时调用：存最后一次快照并注销计划任务",
        "Save the final checkpoint and unregister autosave",
    ),
    (
        "收尾状态（success / error / ...；默认 success）",
        "Final status (success/error/...; default success)",
    ),
    ("显示自动保存状态", "Show autosave status"),
    (
        "stat-only 新鲜度检查（不重建）",
        "Stat-only freshness check (does not rebuild)",
    ),
    (
        "刷新索引（只重算变更文件）",
        "Refresh the index and rehash changed files",
    ),
    (
        "从当前 context 索引重建项目地图",
        "Rebuild the project map from the current context index",
    ),
    (
        "输出项目规模、模块、符号、依赖和风险摘要",
        "Show project size, modules, symbols, dependencies, and risk summary",
    ),
    (
        "查看单个文件的模块、符号、依赖、测试和风险",
        "Show modules, symbols, dependencies, tests, and risks for one file",
    ),
    ("按名称查找符号", "Find symbols by name"),
    (
        "查看 Cargo package / path-dependency 拓扑",
        "Show Cargo package and path-dependency topology",
    ),
    (
        "分析某个文件变更会影响的依赖方、测试和建议验证命令",
        "Analyze dependents, tests, and recommended validation for a changed file",
    ),
    (
        "聚合多个变更路径，生成大型变更的文件分组、风险和验证计划",
        "Group multiple change paths into risks and a validation plan",
    ),
    (
        "计划触碰的文件路径（可重复）",
        "Planned file path (repeatable)",
    ),
    (
        "计划存在阻塞项时退出 1",
        "Exit 1 when the plan has blockers",
    ),
    (
        "汇总项目可维护性质量信号；--check 会在 error 级问题上非零退出",
        "Summarize maintainability signals; --check exits nonzero on errors",
    ),
    (
        "质量策略：standard 低误报；strict 会读取可选质量策略配置",
        "Quality profile: standard is low-noise; strict loads optional policy",
    ),
    (
        "error 级质量问题存在时退出 1；warning 只报告不阻断",
        "Exit 1 for error-level quality findings; warnings remain advisory",
    ),
    (
        "检查强度：默认 standard；quick 仅基础快照；release 为工作区 strict-quality，不是安装发布验证",
        "Check profile: standard default, quick basic snapshot, release workspace strict-quality only",
    ),
    (
        "将就绪结果绑定到一个精确目标",
        "Bind readiness to one exact goal",
    ),
    (
        "未传 --goal 时要求恰好一个 current 目标",
        "Require exactly one current goal when --goal is omitted",
    ),
    (
        "检查前在同一进程刷新上下文",
        "Refresh context in-process before checking",
    ),
    (
        "Codex 生命周期钩子，防止 Owner Mode 过早交接",
        "Codex lifecycle hook integration",
    ),
    (
        "激活、重绑、停用或检查工作区契约",
        "Activate, rebind, deactivate, or inspect the workspace contract",
    ),
    (
        "快照根目录（默认用户级：Windows 为 %LOCALAPPDATA%\\Rayman\\checkpoints）",
        "Checkpoint root (user-level by default; %LOCALAPPDATA%\\Rayman\\checkpoints on Windows)",
    ),
    (
        "激活无效时仍保存 recovery-only 快照；不会成为默认 latest 或完成证据",
        "Save a recovery-only checkpoint despite invalid activation; never default latest or completion evidence",
    ),
    ("列出已有快照", "List checkpoints"),
    (
        "恢复快照到工作区（默认最近；会覆盖同名文件）",
        "Restore a checkpoint (latest by default; overwrites matching files)",
    ),
    (
        "快照 id 或 \"latest\"（默认最近）",
        "Checkpoint ID or \"latest\" (latest by default)",
    ),
    (
        "确认覆盖工作区文件（恢复是破坏性操作，必须显式确认）",
        "Confirm overwriting workspace files during destructive restore",
    ),
    (
        "显式允许恢复 recovery-only 快照；当前激活仍必须已经修复",
        "Allow recovery-only restore after activation has been repaired",
    ),
    (
        "验证指定或最近完整快照的 manifest、路径和逐文件 hash，不写入工作区",
        "Verify manifest, paths, and file hashes without writing the workspace",
    ),
    (
        "快照 id 或 \"latest\"（默认最近完整快照）",
        "Checkpoint ID or \"latest\" (latest complete by default)",
    ),
    ("显示最近一次快照的状态", "Show latest checkpoint status"),
    (
        "RaymanCodingSkill v2：多语言的上下文索引 / 目标 / 检查 / 恢复工作流",
        "RaymanCodingSkill v2: multilingual context / goal / check / recovery workflow",
    ),
    (
        "工作区上下文索引（内容 hash 证明；map/check 会拒绝未验证内容）",
        "Content-hashed workspace context index; map/check reject unverified content",
    ),
    (
        "最小目标契约与待完成项续接",
        "Goal contracts and resumable pending work",
    ),
    ("待完成项", "Pending work"),
    (
        "一次性工作区就绪检查（默认 standard；release 仅代表 strict-quality，不代表已安装发布）",
        "Workspace readiness check (standard by default; release is strict-quality, not installation)",
    ),
    (
        "顺序刷新上下文并确认指定目标仍可继续实施",
        "Refresh context and prepare the specified goal",
    ),
    (
        "顺序刷新上下文并执行绑定指定目标的完成门禁",
        "Refresh context and run the specified goal completion gate",
    ),
    (
        "项目地图与变更影响分析（依赖当前 context 索引）",
        "Project map and change-impact analysis (requires the context index)",
    ),
    (
        "只读的过时资产与未完成标记扫描",
        "Read-only stale asset and work-in-progress scan",
    ),
    ("托管临时目录", "Managed temporary directory"),
    (
        "只读审计受管状态、退役状态与临时空间，不自动删除任何文件",
        "Read-only managed/retired state and temp audit; never deletes files",
    ),
    (
        "工作树快照：整树本地拷贝，便于断电/切换 AI 工具后恢复",
        "Workspace checkpoints for crash and client-switch recovery",
    ),
    (
        "自动快照生命周期：开工注册 Windows 计划任务定时保存，完成/出错时存最后一次并停止",
        "Autosave lifecycle with Windows scheduling and final snapshot",
    ),
    (
        "检查已安装二进制、PATH 与工作区 skill 的身份契约；不证明源码新鲜度",
        "Inspect installed binary/PATH/workspace skill identity; not source freshness",
    ),
    (
        "界面语言：auto 按环境/系统区域选择；也可用 RAYMAN_LANG / UI language",
        "UI language: auto follows environment/OS locale; RAYMAN_LANG is supported",
    ),
    ("输出格式", "Output format"),
    (
        "保存当前工作树快照；默认不删除任何旧恢复点",
        "Save a checkpoint; preserve every existing recovery point by default",
    ),
    (
        "显式确认保存后只保留最近 N 个完整快照；省略则不裁剪",
        "Explicitly retain only the latest N complete checkpoints after saving",
    ),
    (
        "显式裁剪已验证的完整快照；不会把损坏快照当作可删除候选",
        "Explicitly prune verified complete checkpoints; corrupt snapshots are retained",
    ),
    (
        "保留最近 N 个完整快照（至少 1）",
        "Keep the latest N complete checkpoints (at least 1)",
    ),
    (
        "确认删除旧恢复点",
        "Confirm deletion of old recovery points",
    ),
    ("建议:", "Recommendation:"),
    ("工作区就绪检查", "Workspace readiness check"),
    ("上下文:", "Context:"),
    ("资产:", "Assets:"),
    ("质量:", "Quality:"),
    ("过时候选", "stale candidates"),
    ("未完成标记", "work-in-progress markers"),
    ("提示，不阻塞", "advisory, non-blocking"),
    ("共 ", "total "),
    (" 个文件", " files"),
    ("复用 ", "reused "),
    ("重算 ", "rehashed "),
    ("移除 ", "removed "),
    (
        "已保留但不参与当前 readiness",
        "is retained but does not participate in current readiness",
    ),
    (
        "仍为 active；用 goal validate 记录实际验证后必须 goal close",
        "is still active; record real validation with goal validate, then run goal close",
    ),
    (" 的 must 需求 ", " must requirement "),
    ("仍未完成", "is still open"),
    ("目标不存在", "goal does not exist"),
    ("上下文索引不是 ready", "context index is not ready"),
    ("当前:", "current:"),
    ("先运行", "run"),
    ("已保存快照", "Checkpoint saved"),
    ("个文件", "files"),
    ("跳过", "skipped"),
    ("锁定/无权限", "locked/permission denied"),
    (
        "按显式 retention policy 清理旧快照",
        "old checkpoints pruned by explicit retention policy:",
    ),
    (
        "未裁剪任何旧恢复点",
        "no existing recovery point was pruned",
    ),
    ("位置:", "Location:"),
    (
        "已按显式 retention policy 保留最近",
        "Explicit retention policy kept the latest",
    ),
    ("个完整快照，删除", "complete checkpoints and removed"),
    (
        "checkpoint prune 会删除旧恢复点；传 --yes 显式确认",
        "checkpoint prune deletes old recovery points; pass --yes to confirm",
    ),
    (
        "状态锁不是安全普通文件",
        "state lock is not a safe regular file",
    ),
    ("无法检查状态锁", "unable to inspect state lock"),
    (
        "无法打开状态锁（权限或 ACL 拒绝）",
        "unable to open state lock (permission or ACL denied)",
    ),
    ("无法复查状态锁", "unable to recheck state lock"),
    (
        "状态锁被替换为非普通文件",
        "state lock was replaced by a non-regular file",
    ),
    (
        "状态正在被另一个 rayman 进程修改",
        "state is being modified by another rayman process",
    ),
    ("等待锁超过", "lock wait exceeded"),
    (
        "无法取得状态独占锁（权限或 ACL 拒绝）",
        "unable to acquire exclusive state lock (permission or ACL denied)",
    ),
    ("秒", "seconds"),
];

/// Fragment translation is a last resort: it rewrites an already-formatted line
/// that matched no authored template, so it cannot tell framework text from a
/// goal title, requirement or path the user wrote.
///
/// Han-word-boundary protection alone was not enough — it only saved fragments
/// glued to other ideographs, so `延迟 秒 精度` still became `延迟 seconds 精度`
/// and `计时器(秒)` became `计时器(seconds)`. The line is now rewritten only when
/// **every** ideograph in it is accounted for by the catalog. Any leftover Han is
/// text this build does not author, which means it is user content, and a
/// partly-translated line is worth far less than an intact one.
fn localize_known_fragments(mut line: String, language: ActiveLanguage) -> String {
    if language != ActiveLanguage::En || !line_han_is_fully_known(&line) {
        return line;
    }
    for &(chinese, english) in MESSAGE_FRAGMENT_CATALOG {
        line = replace_fragment_outside_han_words(&line, chinese, english);
    }
    line
}

/// Every key this rewriter can actually apply, longest first, so removing one
/// never strands part of a longer entry.
///
/// It must list exactly the catalog [`localize_known_fragments`] rewrites with
/// — no more. Counting coverage from all three catalogs while rewriting from
/// only one declared a line "fully known" that the rewriter could translate
/// just part of, which is how a user goal title made of common words
/// (`先运行 测试`) came out as the half-translated hybrid `run 测试` — the exact
/// data corruption the full-line gate exists to prevent.
fn sorted_fragment_keys() -> &'static [&'static str] {
    static KEYS: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    KEYS.get_or_init(|| {
        let mut keys = MESSAGE_FRAGMENT_CATALOG
            .iter()
            .map(|&(chinese, _)| chinese)
            .filter(|chinese| chinese.chars().any(is_han))
            .collect::<Vec<_>>();
        keys.sort_by(|left, right| {
            right
                .chars()
                .count()
                .cmp(&left.chars().count())
                .then(left.cmp(right))
        });
        keys
    })
}

fn line_han_is_fully_known(line: &str) -> bool {
    if !line.chars().any(is_han) {
        return true;
    }
    let mut remaining = line.to_string();
    for key in sorted_fragment_keys() {
        if remaining.contains(key) {
            remaining = remaining.replace(key, " ");
            if !remaining.chars().any(is_han) {
                return true;
            }
        }
    }
    !remaining.chars().any(is_han)
}

/// 翻译已知固定片段，但**不触碰被表意文字夹在词中间的出现**。localize 是对已格式化
/// 整行的事后重写，无法区分框架固定文案与用户动态内容（goal 标题、需求、路径）。
/// 单字/短片段如「秒」会同时出现在固定文案和用户标题「秒表功能」里；对整行盲替换会把
/// 后者改成「seconds表功能」（数据损坏）。这里要求片段紧邻的前后字符都不是表意文字，
/// 即它是一个 Han 词边界上的完整片段，而非嵌在更长 Han 串中间——宁可漏翻固定文案，
/// 也不改动用户内容。残留边界情形：某个动态值本身恰好整词等于一个片段，仍会被翻译。
fn replace_fragment_outside_han_words(line: &str, chinese: &str, english: &str) -> String {
    let mut result = String::with_capacity(line.len());
    let mut rest = line;
    let mut consumed = 0usize;
    while let Some(position) = rest.find(chinese) {
        // The lookbehind must come from the original line: `rest` has already
        // been advanced past the previous match, so a second occurrence sitting
        // immediately after the first would otherwise see an empty prefix and
        // be treated as unembedded.
        let absolute = consumed + position;
        let before = line[..absolute].chars().next_back();
        let after_index = position + chinese.len();
        let after = rest[after_index..].chars().next();
        let embedded_in_han_word = before.is_some_and(is_han) || after.is_some_and(is_han);
        result.push_str(&rest[..position]);
        result.push_str(if embedded_in_han_word {
            chinese
        } else {
            english
        });
        rest = &rest[after_index..];
        consumed += after_index;
    }
    result.push_str(rest);
    result
}

fn is_han(character: char) -> bool {
    matches!(character as u32, 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff)
}

macro_rules! println {
    () => {
        std::println!()
    };
    ($($argument:tt)*) => {{
        let rendered = format!($($argument)*);
        std::println!("{}", crate::i18n::localize_line(rendered));
    }};
}

macro_rules! eprintln {
    () => {
        std::eprintln!()
    };
    ($($argument:tt)*) => {{
        let rendered = format!($($argument)*);
        std::eprintln!("{}", crate::i18n::localize_line(rendered));
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_catalog_has_complete_unique_bilingual_entries() {
        let mut chinese = std::collections::BTreeSet::new();
        let mut english = std::collections::BTreeSet::new();
        for (zh_cn, en) in MESSAGE_PREFIX_CATALOG {
            assert!(!zh_cn.trim().is_empty());
            assert!(!en.trim().is_empty());
            assert!(chinese.insert(*zh_cn), "duplicate zh-CN catalog entry");
            assert!(english.insert(*en), "duplicate en catalog entry");
        }
        for (index, (zh_cn, en)) in MESSAGE_PREFIX_CATALOG.iter().enumerate() {
            for (later_zh, later_en) in MESSAGE_PREFIX_CATALOG.iter().skip(index + 1) {
                assert!(
                    !later_zh.starts_with(zh_cn),
                    "longer zh-CN prefix must come first"
                );
                assert!(
                    !later_en.starts_with(en),
                    "longer en prefix must come first"
                );
            }
        }
    }

    #[test]
    fn message_catalog_is_complete_and_placeholder_compatible() {
        assert_eq!(CATALOG.len(), MessageId::Count as usize);
        for (index, entry) in CATALOG.iter().enumerate() {
            assert_eq!(entry.id as usize, index);
            assert!(!entry.zh_cn.trim().is_empty());
            assert!(!entry.en.trim().is_empty());
            assert_eq!(
                entry.zh_cn.matches("{}").count(),
                entry.en.matches("{}").count(),
                "{:?}",
                entry.id
            );
        }
    }

    #[test]
    fn typed_catalog_messages_localize_templates_but_preserve_dynamic_unicode() {
        let dynamic_goal = "中文目标🙂".to_string();
        assert_eq!(
            message_for(
                MessageId::GoalCreated,
                &[dynamic_goal.clone(), "2".into()],
                ActiveLanguage::En,
            ),
            "Goal 中文目标🙂 created (2 requirements)"
        );
        assert_eq!(
            message_for(
                MessageId::CheckpointStatus,
                &["cp_1".into(), "Complete".into(), "2026-07-24".into()],
                ActiveLanguage::En,
            ),
            "Latest complete checkpoint: cp_1 (Complete, saved at 2026-07-24)"
        );
    }

    #[test]
    fn locale_parser_prefers_chinese_for_all_zh_variants() {
        assert_eq!(
            language_from_locale("zh_CN.UTF-8"),
            Some(ActiveLanguage::ZhCn)
        );
        assert_eq!(language_from_locale("zh-TW"), Some(ActiveLanguage::ZhCn));
        assert_eq!(
            language_from_locale("en_US.UTF-8"),
            Some(ActiveLanguage::En)
        );
    }

    #[test]
    fn known_fragments_do_not_corrupt_dynamic_han_content() {
        // 回归：MESSAGE_FRAGMENT_CATALOG 含单字 key「秒」→"seconds"。此前 localize_known_fragments
        // 对整行盲替换，把用户 goal 标题/需求里的「秒」翻掉，损坏动态内容。
        assert_eq!(
            localize_line_for(
                "goal_x [current/active] 计时器 秒表功能".into(),
                ActiveLanguage::En,
                false,
            ),
            "goal_x [current/active] 计时器 秒表功能"
        );
        assert_eq!(
            localize_line_for(
                "  req_1 [must/open] 支持启动 秒级精度".into(),
                ActiveLanguage::En,
                false,
            ),
            "  req_1 [must/open] 支持启动 秒级精度"
        );
        // Han-word-boundary protection alone missed every case where the
        // neighbour was punctuation, a digit or a space, which is most real
        // titles. A line carrying Han this build does not author is left alone.
        for title in [
            "goal_x [current/active] 延迟 秒 精度",
            "goal_x [current/active] 计时器(秒)",
            "goal_x [current/active] 5秒 超时",
        ] {
            assert_eq!(
                localize_line_for(title.into(), ActiveLanguage::En, false),
                title,
                "user content must survive the en locale intact"
            );
        }
        // Known and accepted residual: a dynamic value whose ideographs are
        // *exactly* a catalog fragment is indistinguishable from framework text
        // on an already-formatted line, so it is still translated.
        assert_eq!(
            localize_line_for(
                "goal_x [current/active] 秒".into(),
                ActiveLanguage::En,
                false
            ),
            "goal_x [current/active] seconds"
        );
    }

    #[test]
    fn known_fragments_still_translate_framework_text() {
        // Han 词边界规则不能过度失效：边界干净的固定文案仍需在 En 下翻译。
        assert_eq!(
            localize_line_for(
                "未启用自动保存。运行 `rayman autosave start` 开启。".into(),
                ActiveLanguage::En,
                false,
            ),
            "Autosave is disabled. Run `rayman autosave start` to enable it."
        );
        // A line whose ideographs are *entirely* catalog fragments is framework
        // text, so it still gets rewritten.
        assert_eq!(
            localize_line_for("等待锁超过 3 秒".into(), ActiveLanguage::En, false),
            "lock wait exceeded 3 seconds"
        );
        // Production messages that embed 秒 are authored templates, so they are
        // translated through the template path with their dynamic values intact.
        assert_eq!(
            localize_line_for(
                "等待 autosave 独占锁超过 2.5 秒".into(),
                ActiveLanguage::En,
                false
            ),
            "autosave lock wait exceeded 2.5 seconds"
        );
    }

    #[test]
    fn prefix_translation_preserves_dynamic_unicode_values() {
        assert_eq!(
            localize_line_for("文件: 中文目录/项目🙂.rs".into(), ActiveLanguage::En, false,),
            "File: 中文目录/项目🙂.rs"
        );
        assert_eq!(
            localize_line_for(
                "  Source error: 中文内容🙂".into(),
                ActiveLanguage::ZhCn,
                false,
            ),
            "  源码错误: 中文内容🙂"
        );
    }

    fn contains_han(text: &str) -> bool {
        text.chars().any(|character| {
            matches!(character as u32, 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff)
        })
    }

    #[test]
    fn composed_authored_gap_lists_localize_every_segment() {
        let localized = localize_line_for(
            "目标 success receipt 未通过当前或历史完整性复核: 实际变更超出 plan: outside.txt; high-priority plan 缺少绑定最终源码 fingerprint 的 review receipt。仅对应 rollout 前历史可显式使用 --migrate-unreceipted 或 --migrate-receipt-policy receipt_integrity_v1"
                .into(),
            ActiveLanguage::En,
            false,
        );
        assert!(localized.contains("outside.txt"), "{localized}");
        assert!(localized.contains("high-priority plan"), "{localized}");
        assert!(!contains_han(&localized), "{localized}");
    }

    #[derive(Debug)]
    struct SourceLiteral {
        line: usize,
        text: String,
    }

    fn rust_string_literals(source: &str) -> Vec<SourceLiteral> {
        let bytes = source.as_bytes();
        let mut literals = Vec::new();
        let mut index = 0;
        let mut line = 1;
        let mut block_comment_depth = 0_u32;

        while index < bytes.len() {
            if block_comment_depth > 0 {
                if bytes[index..].starts_with(b"/*") {
                    block_comment_depth += 1;
                    index += 2;
                } else if bytes[index..].starts_with(b"*/") {
                    block_comment_depth -= 1;
                    index += 2;
                } else {
                    if bytes[index] == b'\n' {
                        line += 1;
                    }
                    index += 1;
                }
                continue;
            }

            if bytes[index..].starts_with(b"//") {
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
                continue;
            }
            if bytes[index..].starts_with(b"/*") {
                block_comment_depth = 1;
                index += 2;
                continue;
            }

            if bytes[index] == b'\'' {
                let mut character_end = index + 1;
                if character_end < bytes.len() && bytes[character_end] == b'\\' {
                    character_end = (character_end + 2).min(bytes.len());
                } else if character_end < bytes.len() {
                    let width = source[character_end..]
                        .chars()
                        .next()
                        .map(char::len_utf8)
                        .unwrap_or(0);
                    character_end = (character_end + width).min(bytes.len());
                }
                if character_end < bytes.len() && bytes[character_end] == b'\'' {
                    index = character_end + 1;
                    continue;
                }
            }

            let raw_start = if bytes[index] == b'r' {
                Some(index)
            } else if bytes[index..].starts_with(b"br") {
                Some(index + 1)
            } else {
                None
            };
            if let Some(raw_start) = raw_start {
                let mut marker = raw_start + 1;
                while marker < bytes.len() && bytes[marker] == b'#' {
                    marker += 1;
                }
                if marker < bytes.len() && bytes[marker] == b'"' {
                    let hashes = marker - raw_start - 1;
                    let content_start = marker + 1;
                    let literal_line = line;
                    index = content_start;
                    while index < bytes.len() {
                        if bytes[index] == b'\n' {
                            line += 1;
                        }
                        if bytes[index] == b'"'
                            && bytes[index + 1..]
                                .get(..hashes)
                                .is_some_and(|suffix| suffix.iter().all(|byte| *byte == b'#'))
                        {
                            let text = source[content_start..index].to_string();
                            literals.push(SourceLiteral {
                                line: literal_line,
                                text,
                            });
                            index += hashes + 1;
                            break;
                        }
                        index += 1;
                    }
                    continue;
                }
            }

            let quote = if bytes[index] == b'"' {
                Some(index)
            } else if bytes[index..].starts_with(b"b\"") {
                Some(index + 1)
            } else {
                None
            };
            if let Some(quote) = quote {
                let literal_line = line;
                let content_start = quote + 1;
                index = content_start;
                while index < bytes.len() {
                    if bytes[index] == b'\n' {
                        line += 1;
                    }
                    if bytes[index] == b'\\' {
                        index = (index + 2).min(bytes.len());
                        continue;
                    }
                    if bytes[index] == b'"' {
                        let text = source[content_start..index].to_string();
                        literals.push(SourceLiteral {
                            line: literal_line,
                            text,
                        });
                        index += 1;
                        break;
                    }
                    index += 1;
                }
                continue;
            }

            if bytes[index] == b'\n' {
                line += 1;
            }
            index += 1;
        }
        literals
    }

    /// The production half of a source file: everything before its test module.
    ///
    /// Splitting on the first `#[cfg(test)]` truncated at a test-only `use`
    /// (goal.rs:19 is one), leaving ~98% of several files unscanned — which is
    /// why both coverage gates passed while `goal show`/`goal close` printed
    /// untranslated Chinese under `--language en`. Only a `#[cfg(test)] mod`
    /// ends the production region; an attribute on any other item does not.
    fn production_source(source: &str) -> &str {
        const MARKER: &str = "#[cfg(test)]";
        let mut offset = 0usize;
        while let Some(found) = source[offset..].find(MARKER) {
            let at = offset + found;
            let rest = source[at + MARKER.len()..].trim_start();
            if rest.starts_with("mod ") {
                return &source[..at];
            }
            offset = at + MARKER.len();
        }
        source
    }

    #[test]
    fn production_source_stops_at_the_test_module_not_at_a_test_only_use() {
        let source = "#[cfg(test)]\nuse crate::x;\nconst A: &str = \"keep\";\n#[cfg(test)]\nmod tests {\n    const B: &str = \"drop\";\n}\n";
        let production = production_source(source);
        assert!(production.contains("keep"), "{production}");
        assert!(!production.contains("drop"), "{production}");
        assert_eq!(production_source("fn plain() {}\n"), "fn plain() {}\n");
    }

    fn production_rs_files(directory: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(directory).expect("read production source directory") {
            let entry = entry.expect("read production source entry");
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().and_then(|value| value.to_str()) == Some("tests") {
                    continue;
                }
                production_rs_files(&path, files);
            } else if path.extension().and_then(|value| value.to_str()) == Some("rs")
                && path.file_name().and_then(|value| value.to_str()) != Some("i18n.rs")
                && path.file_name().and_then(|value| value.to_str()) != Some("tests.rs")
            {
                files.push(path);
            }
        }
    }

    #[test]
    fn production_source_discovery_excludes_nested_test_modules() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("src");
        std::fs::create_dir_all(source.join("tests")).unwrap();
        std::fs::write(source.join("production.rs"), "fn production() {}\n").unwrap();
        std::fs::write(source.join("tests/workflow.rs"), "fn test_only() {}\n").unwrap();

        let mut files = Vec::new();
        production_rs_files(&source, &mut files);

        assert_eq!(files, vec![source.join("production.rs")]);
    }

    #[test]
    fn every_authored_human_message_is_covered_by_the_english_catalog() {
        let source_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        production_rs_files(&source_root, &mut files);
        files.sort();
        let mut uncovered = Vec::new();

        for path in files {
            let source = std::fs::read_to_string(&path).expect("read production Rust source");
            let production = production_source(&source);
            let lines = production.lines().collect::<Vec<_>>();
            for literal in rust_string_literals(production) {
                if !contains_han(&literal.text) {
                    continue;
                }
                let annotated = [
                    literal.line.saturating_sub(2),
                    literal.line.saturating_sub(1),
                ]
                .into_iter()
                .filter_map(|index| lines.get(index))
                .any(|line| line.contains("i18n-json-stable"));
                if annotated {
                    continue;
                }
                let localized = translate_authored_template(&literal.text);
                if contains_han(&localized) {
                    uncovered.push(format!(
                        "{}:{}: {:?} -> {:?}",
                        path.strip_prefix(&source_root).unwrap_or(&path).display(),
                        literal.line,
                        literal.text,
                        localized
                    ));
                }
            }
        }

        assert!(
            uncovered.is_empty(),
            "authored human-visible strings missing English catalog coverage:\n{}",
            uncovered.join("\n")
        );
    }
    #[test]
    fn every_authored_template_localizes_without_mutating_dynamic_unicode() {
        for template in AUTHORED_MESSAGE_TEMPLATES {
            let parsed = parse_format_template(template).expect("parse generated message template");
            let captures = parsed
                .placeholders
                .iter()
                .enumerate()
                .map(|(index, _)| format!("DYNAMIC_{index}_目标失败文件_中文🙂"))
                .collect::<Vec<_>>();
            let rendered =
                render_translated_template(template, &captures).expect("render source template");
            // `localize_line_for` splits indentation off before matching, so the
            // production input for an indented template is the trimmed line.
            let localized = localize_authored_message(rendered.trim_start(), ActiveLanguage::En)
                .unwrap_or_else(|| {
                    panic!("generated template did not match its output: {template}")
                });
            let mut authored_only = localized.clone();
            for capture in &captures {
                assert!(
                    localized.contains(capture),
                    "dynamic content changed for {template:?}: {localized:?}"
                );
                authored_only = authored_only.replace(capture, "");
            }
            assert!(
                !contains_han(&authored_only),
                "localized static content still contains Han text for {template:?}: {localized:?}"
            );
        }
    }

    fn normalized_catalog_template(text: &str) -> String {
        text.replace('\r', "\\r")
            .replace('\n', "\\n")
            .replace('\t', "\\t")
    }

    #[test]
    fn runtime_authored_catalog_matches_all_production_source_templates() {
        let source_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        production_rs_files(&source_root, &mut files);
        files.sort();
        let mut scanned = std::collections::BTreeSet::new();
        for path in files {
            let source = std::fs::read_to_string(&path).expect("read production Rust source");
            let production = production_source(&source);
            for literal in rust_string_literals(production) {
                if contains_han(&literal.text) {
                    scanned.insert(normalized_catalog_template(&literal.text));
                }
            }
        }
        let catalog = AUTHORED_MESSAGE_TEMPLATES
            .iter()
            .map(|template| normalized_catalog_template(template))
            .collect::<std::collections::BTreeSet<_>>();
        let missing = scanned.difference(&catalog).cloned().collect::<Vec<_>>();
        let stale = catalog.difference(&scanned).cloned().collect::<Vec<_>>();
        assert!(
            missing.is_empty() && stale.is_empty(),
            "runtime authored-message catalog drifted from production source\nmissing={missing:#?}\nstale={stale:#?}"
        );
    }

    /// The other two catalogs already assert this; `TEMPLATE_FRAGMENT_CATALOG`
    /// did not, so a repeated Chinese key silently shadowed a later entry —
    /// including one key mapped to two different English strings, one of which
    /// was therefore unreachable.
    #[test]
    fn template_fragment_catalog_has_no_shadowed_entries() {
        let mut chinese = std::collections::BTreeSet::new();
        for (zh_cn, en) in TEMPLATE_FRAGMENT_CATALOG {
            assert!(!zh_cn.trim().is_empty());
            // An empty English value is legitimate here: Chinese measure words
            // such as 个 simply disappear in English.
            assert!(
                !contains_han(en),
                "English template fragment contains Han text: {en}"
            );
            assert!(
                chinese.insert(*zh_cn),
                "duplicate Chinese template fragment: {zh_cn}"
            );
        }
    }

    #[test]
    fn fragment_catalog_is_unique_english_complete_and_preserves_unknown_unicode() {
        let mut chinese = std::collections::BTreeSet::new();
        let mut english = std::collections::BTreeSet::new();
        for (zh_cn, en) in MESSAGE_FRAGMENT_CATALOG {
            assert!(!zh_cn.trim().is_empty());
            assert!(!en.trim().is_empty());
            assert!(
                !contains_han(en),
                "English fragment contains Han text: {en}"
            );
            assert!(
                chinese.insert(*zh_cn),
                "duplicate Chinese fragment: {zh_cn}"
            );
            assert!(english.insert(*en), "duplicate English fragment: {en}");
        }
        for (index, (zh_cn, _)) in MESSAGE_FRAGMENT_CATALOG.iter().enumerate() {
            for (later_zh, _) in MESSAGE_FRAGMENT_CATALOG.iter().skip(index + 1) {
                assert!(
                    !later_zh.starts_with(zh_cn),
                    "longer Chinese fragment must precede its prefix: {zh_cn} -> {later_zh}"
                );
            }
        }

        let localized = localize_line_for(
            "  warning: goal goal_1 仍为 active；用 goal validate 记录实际验证后必须 goal close"
                .into(),
            ActiveLanguage::En,
            false,
        );
        assert!(!contains_han(&localized), "{localized}");
        let dynamic = localize_line_for(
            "warning: user title 中文目标🙂".into(),
            ActiveLanguage::En,
            false,
        );
        assert_eq!(dynamic, "warning: user title 中文目标🙂");
    }

    /// `println!` is overridden to re-localize every rendered line, so a typed
    /// catalog message is translated a second time on its way out. These two
    /// carry host config keys and command names an operator must type
    /// verbatim; a later fragment-catalog entry matching one of them would
    /// silently rewrite the instruction. Pin the tokens in both locales.
    #[test]
    fn host_patch_messages_survive_the_second_localization_pass() {
        const VERBATIM: &[&str] = &[
            "unelevated",
            "elevated",
            "apply_patch",
            "git apply",
            "[windows] sandbox",
        ];
        for language in [ActiveLanguage::ZhCn, ActiveLanguage::En] {
            for (id, arguments) in [
                (MessageId::HostPatchUnusable, vec!["unelevated".to_string()]),
                (MessageId::HostPatchFix, Vec::new()),
            ] {
                let rendered = message_for(id, &arguments, language);
                let round_tripped = localize_line_for(rendered.clone(), language, false);
                assert_eq!(
                    rendered, round_tripped,
                    "{id:?} changed on the second pass under {language:?}"
                );
                for token in VERBATIM {
                    if rendered.contains(token) {
                        assert!(
                            round_tripped.contains(token),
                            "{id:?} lost `{token}` under {language:?}: {round_tripped}"
                        );
                    }
                }
            }
        }
    }

    /// 同一个中文键不得在两个目录里映射到不同英文。
    ///
    /// `translate_authored_template` 把三个目录串起来按长度排序后逐个替换，
    /// 先命中的那条会吃掉全部出现，另一条永远不可达——「先运行」曾同时映射到
    /// "run first" 与 "run"，而没有任何测试能发现这种静默遮蔽。
    #[test]
    fn no_catalog_key_is_shadowed_by_a_different_translation() {
        use std::collections::BTreeMap;
        let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
        for &(chinese, english) in MESSAGE_FRAGMENT_CATALOG
            .iter()
            .chain(MESSAGE_PREFIX_CATALOG)
            .chain(TEMPLATE_FRAGMENT_CATALOG)
        {
            if let Some(previous) = seen.insert(chinese, english) {
                assert_eq!(
                    previous, english,
                    "跨目录重复键 {chinese:?} 映射到两个不同英文，后者永远不可达"
                );
            }
        }
    }

    /// 整行 authored 模板必须先于前缀匹配生效，且缩进不得让它匹配不上。
    /// 此前 doctor 的源码产物行走前缀路径，前缀英文已含 "verifies it with"，
    /// 行尾残留的「验证」再被片段目录翻一次，每次 en 运行都输出
    /// "verifies it with verification with"。
    #[test]
    fn an_indented_authored_line_translates_as_a_whole_instead_of_prefix_plus_fragments() {
        let line = "  仓库源码产物: 未由 doctor 检查；交接/CI 由 `scripts/verify-release-contract.ps1 -RequireSourceFresh` 验证";
        let localized = localize_line_for(line.into(), ActiveLanguage::En, false);
        assert!(
            localized.contains("verifies it with `scripts/verify-release-contract.ps1"),
            "{localized}"
        );
        assert!(!localized.contains("verification with"), "{localized}");
        assert!(!contains_han(&localized), "{localized}");
        assert!(localized.starts_with("  "), "缩进必须保留: {localized:?}");
    }

    /// 整行改写的覆盖判据只能用它真正会应用的目录。此前覆盖用三个目录统计、
    /// 改写只用一个，于是由常用词构成的用户 goal 标题被判为"框架文本"并被
    /// 改写一半（`先运行 测试` → `run 测试`）。
    #[test]
    fn a_user_title_made_of_common_words_is_never_partially_rewritten() {
        for title in ["先运行 测试", "秒表功能", "先运行 验证 再提交"] {
            let localized = localize_line_for(title.into(), ActiveLanguage::En, false);
            assert_eq!(localized, title, "用户内容不得被改写");
        }
    }

    #[test]
    fn json_output_is_never_localized() {
        assert_eq!(
            localize_line_for(
                r#"{"Error:":"File: 中文"}"#.into(),
                ActiveLanguage::ZhCn,
                true,
            ),
            r#"{"Error:":"File: 中文"}"#
        );
    }
}
