# tools/ — Rayman 控制台

`rayman-menu.ps1` 是一个交互式菜单，方便在 **codex / claude / copilot 之间切换**、以及**断电/意外后恢复**时保护工作状态，并快速跑 eval。

## 打开菜单

```powershell
pwsh -File tools\rayman-menu.ps1
```

（或右键脚本 → “使用 PowerShell 运行”。）菜单里的工作区默认是你当前所在目录，可用 `[W]` 切换到任意项目。

## 快照（checkpoint）是什么

一次快照 = **当前工作树**（尊重 `.gitignore`，跳过 `target/`、`node_modules/` 等重目录）+ **`.RaymanCodingSkill/` 任务状态**（目标/待办/上下文索引，不含易变的 `tmp/`）。

- 存到**用户级**目录，不放进仓库：Windows 为 `%LOCALAPPDATA%\Rayman\checkpoints\<工作区名>-<哈希>\<时间戳>\`。每个工作区独立。
- 每次保存后**滚动清理**旧快照，默认保留最近 3 个（`--keep N` 可调）。
- 保存是“先写暂存目录再原子改名”，保存中途断电不会污染最新那个快照。
- **恢复是叠加式**：覆盖同名文件，但不会删除你工作区里多出来的文件。恢复属破坏性操作，命令行需要 `--yes`，菜单里会二次确认。

> 注意：快照默认**不含被 `.gitignore` 忽略的文件**（如 `.env`、本地缓存），`.RaymanCodingSkill/` 任务状态是特意加回来的例外。真正重要的代码请照常 `git commit`；快照是“切工具 / 断电”的第二道保险，不是版本控制的替代。

## 菜单项

| 选项 | 作用 |
|---|---|
| `[1]` 立即保存快照 | `rayman checkpoint save --keep 3` |
| `[2]` 查看快照列表 | `rayman checkpoint list` |
| `[3]` 恢复最近快照 | 二次确认后 `rayman checkpoint restore --yes` |
| `[4]` 安装/更新自动保存计划任务 | 注册 Windows 计划任务（下方详述） |
| `[5]` 卸载自动保存计划任务 | 注销该任务 |
| `[6]` 运行 eval | 从 `evals/backends.json` 列出后端（yunyi/deepseek…）供选择，再选 trials/任务 |
| `[W]` 切换工作区 | 改变要快照的项目路径 |

## 每 30 分钟自动保存（Windows 计划任务）

选 `[4]`，输入间隔（回车=30 分钟）。菜单会为**当前工作区**注册一个名为 `RaymanCheckpoint-<工作区名>` 的计划任务：

- 每 N 分钟跑一次 `rayman checkpoint save`，**无需一直开着窗口**。
- `StartWhenAvailable`：断电/关机错过的那次，开机后会补跑。
- 额外挂了一个“登录时”触发器，重启登录后自动接着跑。
- 不需要管理员权限（按当前用户注册）。

想改间隔或路径就再跑一次 `[4]`（`-Force` 覆盖同名任务）；不想要了用 `[5]` 卸载。也可以在 Windows「任务计划程序」里手动查看/停用。

多个项目想各自自动快照？在每个项目里用 `[W]` 切过去再 `[4]`，每个工作区一个独立任务。

## 直接用命令行（不走菜单）

```powershell
rayman checkpoint save            # 保存当前工作区，保留最近 3 个
rayman checkpoint save --keep 5   # 保留最近 5 个
rayman checkpoint list            # 列出快照
rayman checkpoint status          # 最近一次快照
rayman checkpoint restore --yes   # 恢复最近快照（覆盖同名文件）
rayman checkpoint restore <id> --yes   # 恢复指定快照
rayman checkpoint save --dir D:\my-ckpts   # 自定义快照根目录
```
