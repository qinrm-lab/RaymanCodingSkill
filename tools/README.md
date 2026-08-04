# 自动快照（checkpoint / autosave）

在 **codex / claude / copilot 之间切换**、以及**断电/意外后恢复**时保护工作状态。手动 checkpoint 由 `rayman` 二进制完成（无需 PowerShell 脚本）；自动保存的计划任务仅在 Windows 上由 Windows 计划任务实现。非 Windows 平台请使用系统定时器周期调用 `rayman checkpoint save`。

## 生命周期（推荐用法）

```powershell
rayman autosave start          # 开工：存一次初始快照 + 注册每 30 分钟的计划任务
# … 干活，期间每 30 分钟自动存一次 …
rayman autosave stop           # 全部完成：存最后一次快照 + 注销计划任务
rayman autosave stop --status error   # 出错收尾：同样存最后一次并停止
```

- **`start` 幂等**：每次开工跑一遍即可，会覆盖旧任务、重新存一次初始快照。适合“每次启动就自动注册”。
- **完成即自停**：默认开启 auto-stop——仅当至少有一个目标、**所有目标状态均为 `success`**、且没有待完成项时，下一次定时触发会存最后一次快照并**自动注销**计划任务。`active`、`partial`、`blocked`、目标/待办状态读取失败或根本没有目标，都会保持运行。用 `--no-auto-stop` 可关闭。
- **出错收尾**：程序化流程里出错时调用 `rayman autosave stop --status error`，尝试存最后一次快照后再停；若最终保存失败，会返回错误并保持 active 状态/计划任务，绝不伪造“已保存并停止”。
- `rayman autosave status` 看当前状态（是否运行中、间隔、上次触发、任务是否注册）。

### 参数

```powershell
rayman autosave start --interval 15 --keep 5      # 每 15 分钟，保留最近 5 个
rayman autosave start --dir D:\my-ckpts           # 自定义快照根目录
rayman autosave start --no-auto-stop              # 不自动停，需手动 stop
```

## 计划任务细节

`start` 用 Windows 内置 `schtasks` + 任务 XML 注册一个名为 `RaymanCheckpoint-<工作区名>-<哈希>` 的任务（每个工作区一个）：

- 每 N 分钟触发 `rayman autosave tick`，**无需一直开着窗口**。
- **`StartWhenAvailable`**：断电/关机错过的那次，开机后自动补跑。
- 另挂一个**登录触发器**，重启登录后自动接着跑。
- 按当前用户注册，**不需要管理员权限**；可在 Windows「任务计划程序」里查看。
- `tick` 是任务内部用的，一般不手动调；它会存一次快照，并在检测到 `stop`（或完成）后自注销，做到自愈。

## 快照是什么

一次快照 = **当前工作树**（尊重 `.gitignore`，跳过 `target/`、`node_modules/` 等）+ **v2 白名单任务状态**：goals、`pending.json`、context index/project map 和 `autosave.json`。它不会把整份 `.RaymanCodingSkill/` 当作备份源，因此不带入 `tmp/`、退役状态、评测运行物或其它未列入白名单的数据。

- 存到**用户级**目录，不进仓库：Windows 为 `%LOCALAPPDATA%\Rayman\checkpoints\<工作区名>-<哈希>\<时间戳>\`；其它平台优先为 `$XDG_DATA_HOME/Rayman/checkpoints/`，否则为 `$HOME/.local/share/Rayman/checkpoints/`。若这些变量不可用而有 `$USERPROFILE`，则退回 `$USERPROFILE/Rayman/checkpoints/`。`--dir` 会覆盖以上位置。
- save/restore/prune 共用跨进程锁，重叠的手工/计划任务不会互删正在写的 staging。手工 `checkpoint save` 与 `salvage-save` 默认不删除任何旧恢复点；只有显式 `save --keep N`、`prune --keep N --yes`，或 `autosave start --keep N` 写入的 retention policy 才能裁剪已验证快照。普通完整快照、`recovery_only` 和 `partial` 使用互不影响的池；`corrupt` 及 crash 遗留 staging 保留取证，自动 prune 不删除 staging。
- 保存是“先写暂存目录再原子改名”。复制、遍历或完整性校验失败会留下取证状态并以非零失败，绝不替换或污染最近的完整快照。
- `checkpoint list` 同时显示完整性状态和 `standard|recovery_only` purpose；`checkpoint status` / 默认 `latest` 只选择最近一次已验证的普通完整快照。
- `checkpoint verify [id|latest]` 只读复核 v3 manifest、路径、文件数、大小、权限和 SHA-256；显式 ID 可检查 recovery-only。
- **恢复是叠加式、journaled all-or-nothing、可幂等重跑**：全部源先 staging/校验，全部既有目标先备份，发布失败会逆序回滚；崩溃留下 transaction，由下一次 save/restore 恢复。不会删除工作区里多出来的文件。
- 激活损坏时可运行 `checkpoint salvage-save`。manifest 会记录激活来源并永久标为 `recovery_only`，不能冒充默认恢复点或完成证据。恢复它必须先修复激活，再显式给 ID、`--yes --allow-recovery-only`。

> 快照默认**不含被 `.gitignore` 忽略的文件**（`.RaymanCodingSkill/` 任务状态是特意加回来的例外）。重要代码请照常 `git commit`；快照是“切工具 / 断电”的第二道保险，不是版本控制替代。

## 手动快照命令

```powershell
rayman checkpoint save                         # 存当前工作区，不删除旧恢复点
rayman checkpoint save --keep 5                # 本次保存后显式保留最近 5 个
rayman checkpoint prune --keep 5 --yes          # 独立、显式裁剪
rayman checkpoint salvage-save   # 激活无效也可保存；只作 recovery-only
rayman checkpoint list            # 列出快照
rayman checkpoint status          # 最近一次已验证的完整快照
rayman checkpoint verify          # 只读验证最近完整快照
rayman checkpoint verify <id>     # 只读验证指定快照
rayman checkpoint restore --yes         # 恢复最近完整快照（覆盖同名文件）
rayman checkpoint restore <id> --yes    # 恢复指定的完整快照
rayman checkpoint restore <recovery-id> --yes --allow-recovery-only
```

## 状态审计与托管临时目录

```powershell
rayman state audit          # 只读列出 v2 允许状态、退役条目和递归 temp 指标
rayman state audit --check  # 退役状态、审计错误或遍历错误时非零；不会删除任何文件
rayman temp status          # 递归 files/dirs/bytes 与 traversal errors
rayman temp pytest-lease <label>  # 独立 basetemp/cache/TMP/pycache + 探针
rayman temp pytest-probe <id>
rayman temp pytest-release <id>   # 只释放 manifest 精确拥有的租约
```

`state audit` 只提供清理/迁移决策所需的证据。即使 `--check` 失败，它也不会删除、迁移或覆盖状态；先审阅输出并取得明确批准。`temp cleanup` 仍是唯一会删除状态的 temp 命令，且只删除 `.RaymanCodingSkill/tmp/`。

## 跑 eval（选后端）

eval 是独立项目。先从 `evals/backends.example.json` 复制出 `evals/backends.json` 并填好 key，再用后端名跑（示例配置里有 `deepseek` / `openrouter` / `ollama` / `relay-responses`）：

```powershell
cd evals
cargo run -- --backend deepseek --trials 3 --unsafe-host-exec
cargo run -- --backend deepseek --task fix-failing-test --unsafe-host-exec   # 便宜的单任务烟测
```

`--unsafe-host-exec` 是刻意的显式确认：真实后端生成的 shell 命令会直接在当前宿主机执行。仅在你接受风险的隔离环境中使用；这种运行永远不可比较（non-comparative），不能用于比较两组或作因果结论。CI 运行 `evals` 的 fmt、clippy、unit tests、依赖策略检查和离线 mock CLI smoke；smoke 会确认未传该参数的真实后端被拒绝，并验证 mock run 的不可变报告指针、seed、执行模式和 trial 数。它不配置真实后端密钥、不发出模型请求，也不把 mock 结果当作模型效果结论。真实 backend 运行前请先阅读 `evals/README.md` 并遵守本机隔离与凭据要求。

## 安装、升级与全仓审计

不要手工复制二进制或只跑 `cargo install`；它们不会同步 canonical skill，也没有回滚/身份验证。源码 checkout 的唯一安装/升级入口是：

```powershell
# 先从 $PROFILE/启动脚本删除持久 Function/Alias，再在同一 pwsh 7 环境确认无 shadow
./scripts/install-rayman.ps1 -Yes -AddToUserPath
```

安装器只替换目标目录中的 `rayman[.exe]` 与 `install-manifest.json` 的
`codex_skill_resources` 所列文件（当前为 `SKILL.md`、`AGENTS.md`、
`references/workflow-contract.md`），写入前要求 clean source-fresh 字节一致；同目录 staging、backup move、最终替换与 Windows user PATH 更新处于同一回滚事务。`-AddToUserPath` 会把目标放到 user segment 最前，再按真实未来顺序 `Machine PATH + User PATH` 验证，机器级旧 `rayman` 在前会直接阻断；不传该开关时，当前 PATH 必须已优先解析到目标，非 Windows 传该开关会明确失败。完整交接审计只有一条命令；它要求显式给出已经安装的 application 与已部署 canonical skill：

```powershell
./scripts/audit-repository.ps1 `
  -CliPath (Get-Command rayman -CommandType Application).Source `
  -SkillPath "$HOME/.codex/skills/raymancodingskill/SKILL.md"
```

此审计依次覆盖 root/evals fmt、Clippy、tests、deny，`cargo package`/`cargo install` smoke，当前 artifact 的 context + strict + release 自食验证，`state audit --check` 门禁与仅作报告的 `assets` 扫描，以及最终 clean-source/PATH/skill 身份。任一**门禁**失败都不能声明发布或安装完成；`assets` 只报告，不阻塞。
