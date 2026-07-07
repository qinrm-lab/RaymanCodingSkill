# 自动快照（checkpoint / autosave）

在 **codex / claude / copilot 之间切换**、以及**断电/意外后恢复**时保护工作状态。全部由 `rayman` 二进制完成（无需 PowerShell 脚本），自动保存用 Windows 计划任务实现。

## 生命周期（推荐用法）

```powershell
rayman autosave start          # 开工：存一次初始快照 + 注册每 30 分钟的计划任务
# … 干活，期间每 30 分钟自动存一次 …
rayman autosave stop           # 全部完成：存最后一次快照 + 注销计划任务
rayman autosave stop --status error   # 出错收尾：同样存最后一次并停止
```

- **`start` 幂等**：每次开工跑一遍即可，会覆盖旧任务、重新存一次初始快照。适合“每次启动就自动注册”。
- **完成即自停**：默认开启 auto-stop——当**所有目标都已关闭且没有待完成项**（`rayman goal` / `rayman check` 的口径）时，下一次定时触发会存最后一次快照并**自动注销**计划任务，不用你记得手动停。用 `--no-auto-stop` 可关闭。
- **出错收尾**：程序化流程里出错时调用 `rayman autosave stop --status error`，同样存最后一次状态再停。
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

一次快照 = **当前工作树**（尊重 `.gitignore`，跳过 `target/`、`node_modules/` 等）+ **`.RaymanCodingSkill/` 任务状态**（目标/待办/上下文索引，不含易变的 `tmp/`）。

- 存到**用户级**目录，不进仓库：Windows 为 `%LOCALAPPDATA%\Rayman\checkpoints\<工作区名>-<哈希>\<时间戳>\`。
- 每次保存**滚动清理**旧的，默认保留最近 3 个（`--keep N`）。
- 保存是“先写暂存目录再原子改名”，存到一半断电不污染最新快照。
- **恢复是叠加式**：覆盖同名文件，不删除工作区里多出来的文件。

> 快照默认**不含被 `.gitignore` 忽略的文件**（`.RaymanCodingSkill/` 任务状态是特意加回来的例外）。重要代码请照常 `git commit`；快照是“切工具 / 断电”的第二道保险，不是版本控制替代。

## 手动快照命令

```powershell
rayman checkpoint save            # 存当前工作区，保留最近 3 个
rayman checkpoint save --keep 5
rayman checkpoint list            # 列出快照
rayman checkpoint status          # 最近一次
rayman checkpoint restore --yes         # 恢复最近快照（覆盖同名文件）
rayman checkpoint restore <id> --yes    # 恢复指定快照
```

## 跑 eval（选后端）

eval 是独立项目，用后端名直接跑（后端在 `evals/backends.json`）：

```powershell
cd evals
cargo run -- --backend yunyi --trials 3      # 或 --backend deepseek
cargo run -- --backend yunyi --task fix-failing-test   # 便宜的单任务烟测
```
