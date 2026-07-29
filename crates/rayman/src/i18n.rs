use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use clap::ValueEnum;
pub const AUTHORED_MESSAGE_TEMPLATES: &[&str] = &[
    "无法写入 checkpoint 根目录（默认在用户目录）: {}；受限沙箱下用 --dir 指定工作区内目录，或以主机权限重试",
    "  状态写探针: 写入被拒或探测失败（权限或 ACL）: {}",
    "  状态写探针: 可写；清理探针失败: {}",
    "  状态写探针: 状态目录不存在，未探测",
    "  状态写探针: 可写",
    "RaymanCodingSkill 工作区未显式激活（status={}）：运行 `rayman workspace activate --skill-file <canonical-SKILL.md> --yes`；历史 .RaymanCodingSkill 状态不会自动激活 skill",
    "finish 要求当前稳定 authority receipt；先运行 `rayman goal validate {goal_id} --req <req> --message <evidence> --command <project-gate> --changed <path> --authority --repeat 2`",
    "`rayman audit` 已退役；工作区门禁使用 `rayman check --profile standard`，任务交付使用 `rayman finish --goal <id>`，状态卫生使用 `rayman state audit --check`",
    "legacy goal {} 仍为 current（status={}）；legacy 记录不能生成当前 receipt，请显式 archive 历史 success，或新建 current-schema replacement 后 supersede",
    "authority gate 必须是受检的 check-repo/audit-repository/verify-release-contract 脚本、`cargo test --workspace|--all`，或无路径选择器的全工作区 pytest",
    "后台继续必须绑定 immediate human consultation，并同时记录非空 background-mechanism、background-authority-evidence 与 background-isolation-evidence",
    "human/external blocker 必须包含 attempts、evidence-path、minimum-input、recommended、alternative、risk、resume-command 与 auto-resume-condition",
    "current goal 缺少开工 baseline；不能作为当前成功证据，请用新的 baseline-bound goal supersede，或将已完成记录显式 archive",
    "已安装身份契约不一致：请使用仓库 release 二进制同步安装，并更新 .RaymanCodingSkill/workspace_skill.yaml 的 skill_sha256",
    "最终 checkpoint 失败，active 状态已尝试回滚但计划任务重注册失败：checkpoint={checkpoint_error}; register={register_error}",
    "未知 pending kind: {value}（可用: machine_actionable | human_input | external_wait | destructive_boundary | hard_gate | repair_exhausted）",
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
    "superseded_by archived 目标 {replacement_id} 是 untrusted legacy quarantine，不能作为完成证明",
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
    "`rayman workspace-skill` 已退役；使用 `rayman workspace status|inspect|activate|deactivate`",
    "共享 quality policy 的父目录必须是工作区内真实目录，不能是链接/reparse: {}",
    "已保存 recovery-only 快照 {} — {} 个文件；它不会成为默认 latest 或完成证据",
    "validation 必须提供至少一个 `--changed`；非代码需求必须显式使用 `--non-code`",
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
    "  质量: profile={} ready={} errors={} warnings={} covered_sources={}/{}",
    "状态正在被另一个 rayman 进程修改: {}；等待锁超过 {} 秒",
    "lifecycle-only replacement 合约、baseline 或专用迁移形态无效",
    "maintenance cycle rebind 未精确绑定 archived command 的 flag/value",
    "maintenance cycle rebind 路径不得经过 symlink/junction/reparse: {}",
    "只有 current-schema active/current 目标可以记录 progress receipt",
    "索引已刷新: 共 {} 个文件（复用 {}，重算 {}，移除 {}）",
    "goal {} package {} progress receipt {} 已记录（non-authoritative）",
    "historical goal {} lifecycle={} 已保留但不参与当前 readiness{}",
    "human owner 只允许 human_input/destructive_boundary/repair_exhausted",
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
    "replacement must 与被转移目标 must 的精确并集不一致",
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
    "非 success goal 的 must 未完整转移到 replacement: {}",
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
    "workspace_skill.yaml 必须是普通非链接文件: {}",
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
    "只有 active/current 目标可以完成 work package",
    "目标目录含不可安全读取的记录: {details}",
    "被转移目标 {id} 的合约或 lifecycle 已失效",
    "；已轮换掉 {rotated} 份更旧的 partial 快照",
    "Cargo manifest 不在已验证上下文索引中: {}",
    "lane ledger id、baseline 或 authority 标记无效",
    "lifecycle-only authority goal 缺少 lifecycle proof",
    "lifecycle_proof 使用了无效的 legacy quarantine",
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
    "canonical skill 必须是普通非链接文件: {}",
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
    "只有 active/current 目标可以打开 lane",
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
    "skill_file 不是普通非链接文件: {}",
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
    "无法读取工作区激活合同: {}",
    "状态锁不是安全普通文件: {}",
    " → 运行 `rayman context refresh`",
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
    "skill_file 不可读取 {}: {error}",
    "skill_file 路径不能包含换行",
    "只有 success goal 可以 archived",
    "无法哈希 skill_file {}: {error}",
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
    "，跳过 {}（锁定/无权限）",
    "  依赖: outgoing={} incoming={}",
    "closed lane {} 关闭证明无效",
    "writer lane {} 越出允许路径",
    "受管状态审计: clean={clean}",
    "无托管临时目录可清理。",
    "无法取得 checkpoint 独占锁",
    "无法提交 checkpoint: {} -> {}",
    "无法检查 workspace_skill.yaml",
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
    "  直接依赖: {}",
    "受管状态目录",
    "无待完成项。",
    "（未知时间）",
    "  上下文: {}{}",
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
            let localized_content = localize_authored_message(remainder_content, language)
                .unwrap_or_else(|| remainder_content.into());
            return localize_known_fragments(
                format!("{indentation}{target}{remainder_indent}{localized_content}"),
                language,
            );
        }
    }
    if let Some(localized) = localize_authored_message(content, language) {
        return format!("{indentation}{localized}");
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
    if language != ActiveLanguage::En || !contains_han_text(content) {
        return None;
    }
    let mut best: Option<(usize, String)> = None;
    for template in AUTHORED_MESSAGE_TEMPLATES {
        let Some(captures) = match_authored_template(template, content) else {
            continue;
        };
        let translated = translate_authored_template(template);
        if contains_han_text(&translated) {
            continue;
        }
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
        "legacy goal {} 仍为 current（status={}）；legacy 记录不能生成当前 receipt，请显式 archive 历史 success，或新建 current-schema replacement 后 supersede",
        "legacy goal {} remains current (status={}); legacy records cannot produce current receipts, so explicitly archive the historical success or create a current-schema replacement and then supersede it",
    ),
    (
        "只有 success goal 可以 archived",
        "only successful goals can be archived",
    ),
    (
        "superseded_by archived 目标 {replacement_id} 是 untrusted legacy quarantine，不能作为完成证明",
        "superseded_by archived goal {replacement_id} is an untrusted legacy quarantine and cannot prove completion",
    ),
    (
        "非 success goal 的 must 未完整转移到 replacement: {}",
        "the non-successful goal's must requirements were not fully transferred to the replacement: {}",
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
        "无法确定用户数据目录",
        "unable to determine the user data directory",
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
    ("默认", "default"),
    ("确认", "confirm"),
    ("同名", "same-named"),
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
        "状态正在被另一个 rayman 进程修改",
        "state is being modified by another rayman process",
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
    ("未完整转移到", "was not fully transferred to"),
    (
        "未显式绑定被替代目标",
        "does not explicitly bind the superseded goal",
    ),
    ("未规范化或未绑定", "is not normalized or bound"),
    ("来源不可区分", "source is not distinguishable"),
    ("计划任务已注册", "scheduled task registered"),
    ("计划任务未注册", "scheduled task not registered"),
    ("遗留计划任务已注销", "legacy scheduled task unregistered"),
    (
        "已存最后一次快照并停止自动保存",
        "saved the final checkpoint and stopped autosave",
    ),
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
        "无法校验原子复制临时文件",
        "unable to verify the atomic-copy temporary file",
    ),
    (
        "无法同步原子复制临时文件",
        "unable to sync the atomic-copy temporary file",
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
    (
        "无法复制到临时文件",
        "unable to copy into the temporary file",
    ),
    ("无法同步父目录", "unable to sync the parent directory"),
    ("无法写入临时文件", "unable to write the temporary file"),
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
    ("先运行", "run first"),
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
    ("直接依赖", "direct dependencies"),
    ("依赖方", "dependents"),
    ("依赖", "dependencies"),
    ("汇总", "summary"),
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
    ("最近一次", "latest"),
    ("最近错误", "latest error"),
    ("外部边界", "external boundary"),
    (
        "之类的查询不是证据",
        "queries of this kind are not evidence",
    ),
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
    ("恢复", "restore"),
    ("覆盖", "overwrite"),
    ("检查", "inspect"),
    ("写入", "write"),
    ("读取", "read"),
    ("创建", "create"),
    ("打开", "open"),
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
        "已安装身份契约不一致：请使用仓库 release 二进制同步安装，并更新 .RaymanCodingSkill/workspace_skill.yaml 的 skill_sha256",
        "Installed identity mismatch: install the repository release binary and update skill_sha256 in .RaymanCodingSkill/workspace_skill.yaml",
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
        "激活、停用或检查工作区契约",
        "Activate, deactivate, or inspect the workspace contract",
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

fn localize_known_fragments(mut line: String, language: ActiveLanguage) -> String {
    if language != ActiveLanguage::En {
        return line;
    }
    for &(chinese, english) in MESSAGE_FRAGMENT_CATALOG {
        line = replace_fragment_outside_han_words(&line, chinese, english);
    }
    line
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
    while let Some(position) = rest.find(chinese) {
        let before = rest[..position].chars().next_back();
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
        // 「秒」作为独立时间单位（前后非 Han）仍翻译。
        assert_eq!(
            localize_line_for("最近触发 3 秒 前".into(), ActiveLanguage::En, false),
            "最近触发 3 seconds 前"
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

    fn production_rs_files(directory: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(directory).expect("read production source directory") {
            let entry = entry.expect("read production source entry");
            let path = entry.path();
            if path.is_dir() {
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
    fn every_authored_human_message_is_covered_by_the_english_catalog() {
        let source_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        production_rs_files(&source_root, &mut files);
        files.sort();
        let mut uncovered = Vec::new();

        for path in files {
            let source = std::fs::read_to_string(&path).expect("read production Rust source");
            let production = source.split("#[cfg(test)]").next().unwrap_or(&source);
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
            let localized = localize_authored_message(&rendered, ActiveLanguage::En)
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
            let production = source.split("#[cfg(test)]").next().unwrap_or(&source);
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
