//! 工作树自动快照的生命周期（Windows）：
//! - `start`：立刻存一次快照 + 注册一个每 N 分钟触发的计划任务（幂等，每次开工跑一遍即可）。
//! - `tick`：计划任务每次触发时跑；存一次快照；若开启了 auto-stop 且工作已完成，则存最后一次并自停。
//! - `stop`：存最后一次快照 + 注销计划任务（“全部完成”或“出错”时调用）。
//!
//! 计划任务用 Windows 内置 `schtasks` + 任务 XML 注册，XML 里开了 `StartWhenAvailable`，
//! 断电/关机错过的那次会在开机后补跑；另挂一个登录触发器，重启登录后自动接着跑。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::file_io::is_link_or_reparse;
use crate::goal::{GoalLifecycle, GoalStore, PendingStore};
use crate::pathfmt::display_path;
use crate::state_paths;
use crate::{checkpoint, workspace_root};

const DEFAULT_INTERVAL_MIN: u64 = 30;
const AUTOSAVE_LOCK_NAME: &str = "autosave.lock";
const AUTOSAVE_LOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// Serialize the complete autosave lifecycle across processes.  Keeping the
/// lock file stable (rather than deleting it after unlock) prevents different
/// processes from locking different file identities after an unlink/recreate
/// race.
pub struct AutosaveLock {
    file: fs::File,
}

/// Take the autosave writers' lock from outside this module.
///
/// `checkpoint restore` republishes `autosave.json`, which is guarded by this
/// lock rather than by a per-file state lock, so it has to serialize against
/// tick/stop/status like every other writer.
pub fn acquire_lock(root: &Path) -> Result<AutosaveLock> {
    AutosaveLock::acquire(root)
}

impl AutosaveLock {
    fn acquire(root: &Path) -> Result<Self> {
        Self::acquire_with_timeout(root, AUTOSAVE_LOCK_TIMEOUT)
    }

    fn acquire_with_timeout(root: &Path, timeout: Duration) -> Result<Self> {
        let path = state_paths::managed_state_file(root, Path::new(AUTOSAVE_LOCK_NAME), true)?;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("无法打开 autosave 独占锁: {}", display_path(&path)))?;
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("无法复查 autosave 独占锁: {}", display_path(&path)))?;
        if is_link_or_reparse(&metadata) || !metadata.file_type().is_file() {
            bail!("autosave 独占锁不是安全普通文件: {}", display_path(&path));
        }
        if !file
            .metadata()
            .with_context(|| format!("无法读取 autosave 锁句柄: {}", display_path(&path)))?
            .file_type()
            .is_file()
        {
            bail!("autosave 锁句柄不是普通文件: {}", display_path(&path));
        }

        let started = Instant::now();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self { file }),
                Err(error) if is_lock_busy(&error) && started.elapsed() < timeout => {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(error) if is_lock_busy(&error) => {
                    bail!("等待 autosave 独占锁超过 {} 秒", timeout.as_secs_f64());
                }
                Err(error) => return Err(error).context("无法取得 autosave 独占锁"),
            }
        }
    }
}

impl Drop for AutosaveLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn is_lock_busy(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock || matches!(error.raw_os_error(), Some(32) | Some(33))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskRegistration {
    Present,
    Absent,
}

trait TaskScheduler: Send + Sync {
    fn register(&self, root: &Path, name: &str, interval_min: u64) -> Result<()>;
    fn unregister(&self, name: &str) -> Result<bool>;
    fn registration(&self, name: &str) -> Result<TaskRegistration>;
}

struct SystemTaskScheduler;

impl TaskScheduler for SystemTaskScheduler {
    fn register(&self, root: &Path, name: &str, interval_min: u64) -> Result<()> {
        register_task(root, name, interval_min)
    }

    fn unregister(&self, name: &str) -> Result<bool> {
        unregister_task(name)
    }

    fn registration(&self, name: &str) -> Result<TaskRegistration> {
        task_registration(name)
    }
}

/// 自动保存的持久状态（单一事实来源；tick/stop/status 都读它）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutosaveState {
    pub active: bool,
    pub interval_min: u64,
    pub keep: usize,
    #[serde(default)]
    pub dir: Option<String>,
    pub auto_stop: bool,
    pub task_name: String,
    pub started_at: String,
    #[serde(default)]
    pub last_tick_at: Option<String>,
    #[serde(default)]
    pub stopped_at: Option<String>,
    #[serde(default)]
    pub stop_status: Option<String>,
    /// 连续失败的 tick 数（成功一次即归零）。
    ///
    /// 计划任务的 stdout/stderr 没有人看：没有这个计数器，一个每 30 分钟失败一次
    /// 的自动保存看起来和正常运行一模一样，用户会一直以为快照在生成。
    #[serde(default)]
    pub consecutive_failures: u64,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub last_error_at: Option<String>,
}

fn state_path(root: &Path, create_parents: bool) -> Result<PathBuf> {
    state_paths::managed_state_file(root, Path::new("autosave.json"), create_parents)
}

/// 状态损坏不能被当成“未启用”：那会使 start/stop 覆写唯一的故障证据，或让 tick
/// 错误注销仍可能需要恢复的计划任务。
fn load_state(root: &Path) -> Result<Option<AutosaveState>> {
    crate::file_io::read_json(&state_path(root, false)?)
}

/// The checkpoint store autosave was configured to write into, if any.
///
/// Anything that reports "the latest checkpoint" has to consult this: autosave
/// honors `--dir`, and the workflow reference tells agents to point it at the
/// workspace in sandboxes, so reading only the default user-profile root
/// reports `none` for workspaces that are in fact being snapshotted.
pub fn configured_checkpoint_dir(root: &Path) -> Option<PathBuf> {
    load_state(root)
        .ok()
        .flatten()
        .and_then(|state| state.dir)
        .map(PathBuf::from)
}

fn save_state(root: &Path, state: &AutosaveState) -> Result<()> {
    crate::file_io::write_json(&state_path(root, true)?, state)
}

/// 计划任务名：每个工作区一个，稳定且唯一。
pub fn task_name(root: &Path) -> String {
    format!("RaymanCheckpoint-{}", checkpoint::workspace_key(root))
}

/// 工作是否“全部完成”：至少有一个 current 目标，且**存储里的每一个目标**
/// ——包括 archived/superseded 记录——都通过同一份 `goal_gate_verdict`，
/// 没有待完成项。退休记录并不被跳过：它们同样必须无 blocker，否则一条损坏的
/// 历史记录会让自动快照永远停不下来，这一点与只看 current 目标的直觉不同。
/// 没有任何 current 目标时返回 false（无从判断完成，交给显式 `stop`）。
/// 任何状态文件读不出来都按“未完成”处理：损坏的 active 目标被当成不存在
/// 会导致自动快照在工作进行中自停并注销。
pub fn work_is_complete(root: &Path) -> bool {
    let Ok((goals, issues)) = GoalStore::new(root).list_with_issues() else {
        return false;
    };
    if !issues.is_empty() {
        return false;
    }
    let Ok(fingerprint) = crate::goal::workspace_fingerprint(root) else {
        return false;
    };
    if !goals
        .iter()
        .any(|goal| goal.lifecycle == GoalLifecycle::Current)
    {
        return false;
    }
    // 与 `rayman check --profile standard` 共用同一份判定。这里曾经手工复刻同一套
    // 语义，结果两边独立漂移：整目标差量门禁只加到了 check 一侧，autosave 就会在
    // check 判定未就绪的状态下认定工作已完成并自停快照。
    if goals.iter().any(|goal| {
        !crate::goal::goal_gate_verdict(goal, &goals, root, Some(&fingerprint))
            .blockers
            .is_empty()
    }) {
        return false;
    }
    matches!(PendingStore::new(root).list(), Ok(items) if items.is_empty())
}

fn dir_override(state_dir: &Option<String>) -> Option<PathBuf> {
    state_dir.as_ref().map(PathBuf::from)
}

/// 一次生命周期动作的结果，供 CLI 打印。
pub struct ActionOutcome {
    pub message: String,
    pub state: Option<AutosaveState>,
}

/// 开工：存一次初始快照并注册计划任务。可重复调用（幂等重装）。
pub fn start(
    root: &Path,
    interval_min: u64,
    keep: usize,
    auto_stop: bool,
    dir: Option<&Path>,
) -> Result<ActionOutcome> {
    let _lock = AutosaveLock::acquire(root)?;
    start_with_scheduler(
        root,
        interval_min,
        keep,
        auto_stop,
        dir,
        &SystemTaskScheduler,
    )
}

fn start_with_scheduler(
    root: &Path,
    interval_min: u64,
    keep: usize,
    auto_stop: bool,
    dir: Option<&Path>,
    scheduler: &dyn TaskScheduler,
) -> Result<ActionOutcome> {
    let interval_min = interval_min.max(1);
    // 不覆盖损坏的 autosave.json；使用者需要先保全并修复它。
    let _ = load_state(root)?;
    let saved = checkpoint::save(root, dir, keep)?;

    let name = task_name(root);
    let state = AutosaveState {
        active: true,
        interval_min,
        keep,
        dir: dir.map(display_path),
        auto_stop,
        task_name: name.clone(),
        started_at: crate::timefmt::now_iso(),
        last_tick_at: None,
        stopped_at: None,
        stop_status: None,
        consecutive_failures: 0,
        last_error: None,
        last_error_at: None,
    };
    activate_state_with(
        root,
        &state,
        || scheduler.register(root, &name, interval_min),
        || {
            if scheduler.unregister(&name)? {
                Ok(())
            } else {
                bail!("注册成功后回滚计划任务失败：任务未找到")
            }
        },
    )?;

    Ok(ActionOutcome {
        message: format!(
            "已存初始快照 {}（{} 个文件）并注册计划任务 '{}'：每 {} 分钟自动快照{}。",
            saved.id,
            saved.file_count,
            name,
            interval_min,
            if auto_stop {
                "，完成后自动停止"
            } else {
                ""
            }
        ),
        state: Some(state),
    })
}

/// 计划任务每次触发：存一次快照；未激活则自注销；开启 auto-stop 且完成则存最后一次并自停。
pub fn tick(root: &Path) -> Result<ActionOutcome> {
    tick_entry(root, &SystemTaskScheduler)
}

/// 免顶层门禁的入口在拿锁之前必须先探测受管状态根：AutosaveLock::acquire 以
/// create_parents=true 打开锁文件，会在从未激活的目录里凭空创建
/// `.RaymanCodingSkill/`——那既是 workspace 标记（劫持子目录的 workspace-root
/// 解析），又会被后续命令误诊为 orphan_state。状态根不存在时不碰盘面。
fn tick_entry(root: &Path, scheduler: &dyn TaskScheduler) -> Result<ActionOutcome> {
    if state_paths::managed_state_root(root, false)?.is_none() {
        return tick_without_state(root, scheduler);
    }
    let _lock = AutosaveLock::acquire(root)?;
    tick_with_scheduler(root, scheduler)
}

/// 没有状态：不该有任务在跑，尽力注销遗留任务后退出，不创建任何状态。
fn tick_without_state(root: &Path, scheduler: &dyn TaskScheduler) -> Result<ActionOutcome> {
    let name = task_name(root);
    let removed = scheduler.unregister(&name)?;
    Ok(ActionOutcome {
        message: if removed {
            "无自动保存状态，遗留计划任务已注销。".into()
        } else {
            "无自动保存状态，也没有已注册的计划任务。".into()
        },
        state: None,
    })
}

fn tick_with_scheduler(root: &Path, scheduler: &dyn TaskScheduler) -> Result<ActionOutcome> {
    let Some(mut state) = load_state(root)? else {
        return tick_without_state(root, scheduler);
    };
    if !state.active {
        let removed = scheduler.unregister(&state.task_name)?;
        return Ok(ActionOutcome {
            message: if removed {
                "自动保存已停止，遗留计划任务已注销。".into()
            } else {
                "自动保存已停止，计划任务未注册。".into()
            },
            state: Some(state),
        });
    }

    // 激活检查在 tick 内部对真正要拍快照的 root 执行，而不是 main 的顶层门禁：
    // 顶层门禁会让 tick 在失败落盘之前 exit 1（违背下面"每次结果必须持久化"的
    // 契约），且它检查的是 cwd 工作区而非 --workspace 目标，会给激活破损的
    // 工作区铸造 standard 快照。失败按 tick 失败记账后原样返回。
    if let Err(error) = crate::workspace::require_active(root) {
        let now = crate::timefmt::now_iso();
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        state.last_error = Some(format!("workspace 未激活，跳过快照: {error:#}"));
        state.last_error_at = Some(now.clone());
        state.last_tick_at = Some(now);
        if let Err(persist_error) = save_state(root, &state) {
            return Err(error).context(format!("自动保存失败状态也未能写入: {persist_error:#}"));
        }
        return Err(error);
    }

    if state.auto_stop && work_is_complete(root) {
        // 与下面的普通保存同约：tick 的任何结局都必须落进 autosave.json，
        // 否则一个持续失败的自停（磁盘满、checkpoint 根被 ACL 拒绝）每 30
        // 分钟静默失败一次，status 却一直显示"运行中、零失败"。
        // finalize 内部已回滚 active 状态与计划任务注册，这里只补记账。
        if let Err(error) = finalize_with_scheduler(root, &mut state, "success (auto)", scheduler) {
            let now = crate::timefmt::now_iso();
            state.consecutive_failures = state.consecutive_failures.saturating_add(1);
            state.last_error = Some(format!("自动停止失败: {error:#}"));
            state.last_error_at = Some(now.clone());
            state.last_tick_at = Some(now);
            if let Err(persist_error) = save_state(root, &state) {
                return Err(error)
                    .context(format!("自动保存失败状态也未能写入: {persist_error:#}"));
            }
            return Err(error);
        }
        return Ok(ActionOutcome {
            message: "检测到全部目标均为 success：已存最后一次快照并停止自动保存。".into(),
            state: Some(state),
        });
    }

    // A tick runs from Task Scheduler, so its exit code and stderr go nowhere a
    // person will ever look.  Every outcome — success or failure — must therefore
    // be persisted into the state file that `autosave status` reads; otherwise a
    // workspace whose saves have failed for a week is indistinguishable from a
    // healthy one.
    let saved = match checkpoint::save(root, dir_override(&state.dir).as_deref(), state.keep) {
        Ok(saved) => saved,
        Err(error) => {
            let now = crate::timefmt::now_iso();
            state.consecutive_failures = state.consecutive_failures.saturating_add(1);
            state.last_error = Some(format!("{error:#}"));
            state.last_error_at = Some(now.clone());
            state.last_tick_at = Some(now);
            if let Err(persist_error) = save_state(root, &state) {
                return Err(error)
                    .context(format!("自动保存失败状态也未能写入: {persist_error:#}"));
            }
            return Err(error);
        }
    };
    state.last_tick_at = Some(crate::timefmt::now_iso());
    state.consecutive_failures = 0;
    state.last_error = None;
    state.last_error_at = None;
    save_state(root, &state)?;

    Ok(ActionOutcome {
        message: format!("已存快照 {}（{} 个文件）。", saved.id, saved.file_count),
        state: Some(state),
    })
}

/// 显式停止（“全部完成”传 success，“出错”传 error 等）：存最后一次快照 + 注销任务。
pub fn stop(root: &Path, status: &str) -> Result<ActionOutcome> {
    stop_entry(root, status, &SystemTaskScheduler)
}

/// 见 [`tick_entry`]：状态根不存在时不得让锁创建它。
fn stop_entry(root: &Path, status: &str, scheduler: &dyn TaskScheduler) -> Result<ActionOutcome> {
    if state_paths::managed_state_root(root, false)?.is_none() {
        crate::workspace::require_active(root)?;
    }
    let _lock = AutosaveLock::acquire(root)?;
    stop_with_scheduler(root, status, scheduler)
}

fn stop_with_scheduler(
    root: &Path,
    status: &str,
    scheduler: &dyn TaskScheduler,
) -> Result<ActionOutcome> {
    let loaded = load_state(root)?;
    // 激活破损时 stop 仍必须能注销计划任务（否则 tick 永远失败下去、无路可停），
    // 但不能给破损工作区铸造 standard 最终快照：跳过快照、如实记账。注销失败
    // 则原样报错并保持 active 状态，与 finalize_state_with 的契约一致。
    // 没有既存状态时不得走这条路径——在未激活目录里 save_state 会凭空创建
    // 受管状态目录。
    if let Err(activation_error) = crate::workspace::require_active(root) {
        let Some(mut state) = loaded else {
            return Err(activation_error).context("workspace 未激活且没有自动保存状态，无需停止");
        };
        let was_active = state.active;
        let interval_min = state.interval_min;
        let rollback_name = state.task_name.clone();
        let removed = scheduler.unregister(&state.task_name)?;
        let now = crate::timefmt::now_iso();
        state.active = false;
        state.stopped_at = Some(now.clone());
        state.stop_status = Some(status.to_string());
        state.last_error = Some(format!(
            "workspace 未激活，最终快照已跳过: {activation_error:#}"
        ));
        state.last_error_at = Some(now);
        if let Err(persist_error) = save_state(root, &state) {
            // 与 finalize_state_with 同约：持久化失败时必须恢复计划任务注册，
            // 否则盘上 active=true 而任务已被注销，status 与下一次 stop 都在说谎。
            if was_active
                && let Err(register_error) = scheduler.register(root, &rollback_name, interval_min)
            {
                bail!(
                    "停止状态写入失败且计划任务重注册失败：state={persist_error}; register={register_error}"
                );
            }
            return Err(persist_error);
        }
        return Ok(ActionOutcome {
            message: if removed {
                format!(
                    "workspace 未激活：已停止自动保存并注销计划任务 '{}'。最终快照已跳过；如需抢救快照，运行 `rayman checkpoint salvage-save`。",
                    state.task_name
                )
            } else {
                format!(
                    "workspace 未激活：已停止自动保存；计划任务 '{}' 未注册。最终快照已跳过；如需抢救快照，运行 `rayman checkpoint salvage-save`。",
                    state.task_name
                )
            },
            state: Some(state),
        });
    }
    let mut state = match loaded {
        Some(state) => state,
        None => AutosaveState {
            active: false,
            interval_min: DEFAULT_INTERVAL_MIN,
            keep: checkpoint::DEFAULT_KEEP,
            dir: None,
            auto_stop: true,
            task_name: task_name(root),
            started_at: crate::timefmt::now_iso(),
            last_tick_at: None,
            stopped_at: None,
            stop_status: None,
            consecutive_failures: 0,
            last_error: None,
            last_error_at: None,
        },
    };
    let removed = finalize_with_scheduler(root, &mut state, status, scheduler)?;
    Ok(ActionOutcome {
        message: if removed {
            format!(
                "已存最后一次快照并停止自动保存（状态：{status}）。计划任务 '{}' 已注销。",
                state.task_name
            )
        } else {
            format!(
                "已存最后一次快照并停止自动保存（状态：{status}）。计划任务 '{}' 未注册。",
                state.task_name
            )
        },
        state: Some(state),
    })
}

/// 存最后一次快照，标记停止，注销任务。
fn finalize_with_scheduler(
    root: &Path,
    state: &mut AutosaveState,
    status: &str,
    scheduler: &dyn TaskScheduler,
) -> Result<bool> {
    let task_name = state.task_name.clone();
    let persist_rollback_name = task_name.clone();
    let checkpoint_rollback_name = task_name.clone();
    let interval_min = state.interval_min;
    finalize_with(
        root,
        state,
        status,
        move || scheduler.unregister(&task_name),
        || scheduler.register(root, &persist_rollback_name, interval_min),
        || scheduler.register(root, &checkpoint_rollback_name, interval_min),
    )
}

fn finalize_with<F, R, C>(
    root: &Path,
    state: &mut AutosaveState,
    status: &str,
    unregister: F,
    reregister: R,
    checkpoint_reregister: C,
) -> Result<bool>
where
    F: FnOnce() -> Result<bool>,
    R: FnOnce() -> Result<()>,
    C: FnOnce() -> Result<()>,
{
    // 保存失败（含 partial checkpoint）时保持 active 状态和计划任务，交给调用者处理；
    // 不能伪造“已最终保存并停止”的结果。
    let original = state.clone();
    checkpoint::save(root, dir_override(&state.dir).as_deref(), state.keep)?;
    let removed = finalize_state_with(
        state,
        status,
        unregister,
        |stopped| save_state(root, stopped),
        reregister,
    )?;
    if let Err(checkpoint_error) =
        checkpoint::save(root, dir_override(&state.dir).as_deref(), state.keep)
    {
        if original.active {
            if let Err(state_error) = save_state(root, &original) {
                bail!(
                    "停止状态已写入，但最终 checkpoint 失败且 active 状态回滚失败：checkpoint={checkpoint_error}; state={state_error}"
                );
            }
            if let Err(register_error) = checkpoint_reregister() {
                // Registration could not be restored.  Put the persisted state
                // back to stopped so it truthfully matches the absent scheduler.
                let stopped = state.clone();
                let _ = save_state(root, &stopped);
                bail!(
                    "最终 checkpoint 失败，active 状态已尝试回滚但计划任务重注册失败：checkpoint={checkpoint_error}; register={register_error}"
                );
            }
            *state = original;
        }
        return Err(checkpoint_error);
    }
    Ok(removed)
}

fn finalize_state_with<F, P, R>(
    state: &mut AutosaveState,
    status: &str,
    unregister: F,
    persist: P,
    reregister: R,
) -> Result<bool>
where
    F: FnOnce() -> Result<bool>,
    P: FnOnce(&AutosaveState) -> Result<()>,
    R: FnOnce() -> Result<()>,
{
    // 注销失败时也必须保持 persisted active 状态；否则任务仍在运行，而 status
    // 和 stop 输出却会谎报已经停止。
    //
    // 返回值同样重要：`unregister` 报告"本来就没注册"时，调用方不得输出
    // "计划任务已注销"——那是对用户说了一件没发生的事。
    let removed = unregister()?;
    let was_active = state.active;
    let mut stopped = state.clone();
    stopped.active = false;
    stopped.stopped_at = Some(crate::timefmt::now_iso());
    stopped.stop_status = Some(status.to_string());
    // `finalize_with` only reaches here after a checkpoint save succeeded, so a
    // stale failure streak from earlier ticks would now be misreporting.
    stopped.consecutive_failures = 0;
    stopped.last_error = None;
    stopped.last_error_at = None;
    if let Err(state_error) = persist(&stopped) {
        if was_active {
            if let Err(register_error) = reregister() {
                bail!(
                    "计划任务已注销，但停止状态写入失败且重新注册失败：state={state_error}; register={register_error}"
                );
            }
            bail!("停止状态写入失败；计划任务已重新注册，autosave 保持 active：{state_error}");
        }
        return Err(state_error);
    }
    *state = stopped;
    Ok(removed)
}

fn activate_state_with<F, R>(
    root: &Path,
    state: &AutosaveState,
    register: F,
    rollback: R,
) -> Result<()>
where
    F: FnOnce() -> Result<()>,
    R: FnOnce() -> Result<()>,
{
    // Registration precedes the active state write.  If registration fails,
    // the previous state remains untouched instead of persisting a phantom
    // active scheduler.
    register()?;
    if let Err(state_error) = save_state(root, state) {
        if let Err(rollback_error) = rollback() {
            bail!(
                "计划任务已注册，但 active 状态写入失败且回滚失败：state={state_error}; rollback={rollback_error}"
            );
        }
        return Err(state_error);
    }
    Ok(())
}

/// 当前自动保存状态摘要。
pub fn status(root: &Path) -> Result<ActionOutcome> {
    // 见 tick_entry：状态根不存在（从未激活）时不得让锁创建它；
    // 报激活错误与豁免顶层门禁之前的行为一致。
    if state_paths::managed_state_root(root, false)?.is_none() {
        crate::workspace::require_active(root)?;
    }
    let _lock = AutosaveLock::acquire(root)?;
    status_with_scheduler(root, &SystemTaskScheduler)
}

fn status_with_scheduler(root: &Path, scheduler: &dyn TaskScheduler) -> Result<ActionOutcome> {
    match load_state(root) {
        Err(error) => bail!("自动保存状态损坏或不可读取；未修改状态，也未注销计划任务：{error}"),
        Ok(None) => Ok(ActionOutcome {
            message: "未启用自动保存。运行 `rayman autosave start` 开启。".into(),
            state: None,
        }),
        Ok(Some(state)) => {
            let registered = scheduler.registration(&state.task_name)?;
            let last = state
                .last_tick_at
                .clone()
                .unwrap_or_else(|| "（尚无）".into());
            // 失败必须出现在这里：计划任务的失败输出无人可见，`status` 是用户
            // 唯一能发现"自动保存其实一直没成功"的地方。
            let failures = if state.consecutive_failures == 0 {
                String::new()
            } else {
                format!(
                    "\n  连续失败：{} 次（最近一次：{}）\n  最近错误：{}",
                    state.consecutive_failures,
                    state.last_error_at.as_deref().unwrap_or("（未知时间）"),
                    state.last_error.as_deref().unwrap_or("（未记录）")
                )
            };
            Ok(ActionOutcome {
                message: format!(
                    "自动保存：{}（每 {} 分钟，keep={}，auto_stop={}）\n  计划任务 '{}'：{}\n  最近一次触发：{}{}",
                    if state.active {
                        "运行中"
                    } else {
                        "已停止"
                    },
                    state.interval_min,
                    state.keep,
                    state.auto_stop,
                    state.task_name,
                    if registered == TaskRegistration::Present {
                        "已注册"
                    } else {
                        "未注册"
                    },
                    last,
                    failures
                ),
                state: Some(state),
            })
        }
    }
}

/// 供 CLI 决定 workspace：显式 `--workspace` 优先，否则从 cwd 向上找工作区根。
pub fn resolve_workspace(explicit: Option<&Path>) -> Result<PathBuf> {
    match explicit {
        Some(path) => Ok(path.canonicalize().unwrap_or_else(|_| path.to_path_buf())),
        None => workspace_root(),
    }
}

// ---------------- Windows 计划任务（schtasks + 任务 XML） ----------------

/// 把计划任务 XML 独占创建在受管临时目录里，并在交给 `schtasks` 之前复查落盘结果。
///
/// 原实现是 `fs::write(temp_dir().join(format!("rayman-task-{pid}.xml")), ..)`。
/// 那条路径完全可预测、写入会跟随符号链接、会截断既有文件，而 `schtasks` 读取它
/// 之前还有一个 TOCTOU 窗口——同用户攻击者可以把它换成自己的 XML，让计划任务以
/// 用户身份注册任意命令。全仓其余写路径都走 `create_new` + 父目录校验，这里也照办：
/// 目录经 `state_paths` 验证过且在工作区内，`create_new` 既不跟随链接也不截断。
#[cfg(any(windows, test))]
fn write_task_xml_exclusive(root: &Path, bytes: &[u8]) -> Result<PathBuf> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    const MAX_NAME_ATTEMPTS: usize = 32;
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let scratch = crate::temp::scratch_dir(root, "autosave")?;
    state_paths::ensure_real_directory(&scratch)?;
    for _ in 0..MAX_NAME_ATTEMPTS {
        let path = scratch.join(format!(
            "task-{}-{}.xml",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("无法独占创建计划任务 XML: {}", display_path(&path)));
            }
        };
        let written = file
            .write_all(bytes)
            .and_then(|()| file.sync_all())
            .with_context(|| format!("无法写入计划任务 XML: {}", display_path(&path)));
        drop(file);
        if let Err(error) = written {
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                let _ = fs::remove_file(&path);
                return Err(error)
                    .with_context(|| format!("无法复查计划任务 XML: {}", display_path(&path)));
            }
        };
        if is_link_or_reparse(&metadata)
            || !metadata.file_type().is_file()
            || metadata.len() != bytes.len() as u64
        {
            let _ = fs::remove_file(&path);
            bail!("计划任务 XML 写入后被替换或截断: {}", display_path(&path));
        }
        return Ok(path);
    }
    bail!("无法为计划任务 XML 创建独占临时文件（连续 {MAX_NAME_ATTEMPTS} 个名称已存在）")
}

#[cfg(windows)]
fn register_task(root: &Path, name: &str, interval_min: u64) -> Result<()> {
    let exe = std::env::current_exe()?;
    let xml = build_task_xml(&exe, root, name, interval_min);

    // schtasks /XML 期望 UTF-16LE 文件；用带 BOM 的 UTF-16LE 写出，兼容非 ASCII 路径。
    let xml_path = write_task_xml_exclusive(root, &utf16le_bom(&xml))?;

    let user = current_user();
    let output = std::process::Command::new("schtasks")
        .args([
            "/Create",
            "/TN",
            name,
            "/XML",
            &xml_path.to_string_lossy(),
            "/RU",
            &user,
            "/F",
        ])
        .output();
    let _ = std::fs::remove_file(&xml_path);

    let output = output?;
    if !output.status.success() {
        bail!(
            "注册计划任务失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(windows)]
fn unregister_task(name: &str) -> Result<bool> {
    match task_registration(name)? {
        TaskRegistration::Absent => return Ok(false),
        TaskRegistration::Present => {}
    }
    let output = std::process::Command::new("schtasks")
        .args(["/Delete", "/TN", name, "/F"])
        .output()?;
    if !output.status.success() {
        let detail = scheduler_output_text(&output.stdout, &output.stderr);
        if scheduler_reports_not_found(output.status.code(), &detail) {
            return Ok(false);
        }
        bail!("注销计划任务失败（任务仍可能在运行）：{detail}");
    }
    Ok(true)
}

#[cfg(windows)]
fn task_registration(name: &str) -> Result<TaskRegistration> {
    let output = std::process::Command::new("schtasks")
        .args(["/Query", "/TN", name])
        .output()
        .context("无法启动 schtasks 查询 autosave 计划任务")?;
    if output.status.success() {
        return Ok(TaskRegistration::Present);
    }
    let detail = scheduler_output_text(&output.stdout, &output.stderr);
    if scheduler_reports_not_found(output.status.code(), &detail) {
        return Ok(TaskRegistration::Absent);
    }
    bail!("查询 autosave 计划任务失败，不能把未知状态当作未注册：{detail}")
}

fn scheduler_output_text(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    match (stdout.is_empty(), stderr.is_empty()) {
        (false, false) => format!("{stdout}\n{stderr}"),
        (false, true) => stdout,
        (true, false) => stderr,
        (true, true) => "schtasks returned no diagnostic output".into(),
    }
}

fn scheduler_reports_not_found(exit_code: Option<i32>, detail: &str) -> bool {
    // HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND/ERROR_PATH_NOT_FOUND), their raw values,
    // and the stable English/Chinese schtasks diagnostics.  Generic exit 1 is
    // deliberately insufficient because access denied and service failures use
    // the same process exit code.
    if matches!(
        exit_code,
        Some(2) | Some(3) | Some(-2_147_024_894) | Some(-2_147_024_893)
    ) {
        return true;
    }
    let detail = detail.to_ascii_lowercase();
    [
        "the system cannot find the file specified",
        "the system cannot find the path specified",
        "cannot find the task",
        "找不到指定的文件",
        "找不到指定的路径",
        "找不到任务",
        "指定的任务不存在",
    ]
    .iter()
    .any(|marker| detail.contains(marker))
}

#[cfg(windows)]
fn current_user() -> String {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
    match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => format!("{domain}\\{user}"),
        _ => user,
    }
}

// 非 Windows 平台仅为可编译；主力是 Windows。
#[cfg(not(windows))]
fn register_task(_root: &Path, _name: &str, _interval_min: u64) -> Result<()> {
    bail!(
        "自动计划任务目前仅支持 Windows；其它平台请用系统定时器周期调用 `rayman checkpoint save`。"
    );
}

#[cfg(not(windows))]
fn unregister_task(_name: &str) -> Result<bool> {
    Ok(false)
}

#[cfg(not(windows))]
fn task_registration(_name: &str) -> Result<TaskRegistration> {
    Ok(TaskRegistration::Absent)
}

/// 生成 Windows 任务计划 XML（可测：断言含关键字段）。
pub fn build_task_xml(exe: &Path, root: &Path, name: &str, interval_min: u64) -> String {
    let start_boundary = local_now_naive();
    let exe_disp = display_path(exe);
    let ws_disp = display_path(root);
    let user = xml_user();
    let args = format!("autosave tick --workspace \"{}\"", ws_disp);

    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Rayman working-tree autosnapshot for {ws}</Description>
    <URI>\{name}</URI>
  </RegistrationInfo>
  <Triggers>
    <TimeTrigger>
      <StartBoundary>{start}</StartBoundary>
      <Enabled>true</Enabled>
      <Repetition>
        <Interval>PT{interval}M</Interval>
        <Duration>P3650D</Duration>
        <StopAtDurationEnd>false</StopAtDurationEnd>
      </Repetition>
    </TimeTrigger>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>true</StartWhenAvailable>
    <Enabled>true</Enabled>
    <ExecutionTimeLimit>PT10M</ExecutionTimeLimit>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
      <Arguments>{args}</Arguments>
      <WorkingDirectory>{ws}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#,
        ws = xml_escape(&ws_disp),
        name = xml_escape(name),
        start = start_boundary,
        interval = interval_min,
        user = xml_escape(&user),
        exe = xml_escape(&exe_disp),
        args = xml_escape(&args),
    )
}

#[cfg(windows)]
fn xml_user() -> String {
    current_user()
}

#[cfg(not(windows))]
fn xml_user() -> String {
    std::env::var("USER").unwrap_or_else(|_| "user".into())
}

fn local_now_naive() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
}

fn utf16le_bom(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    struct AbsentScheduler;

    impl TaskScheduler for AbsentScheduler {
        fn register(&self, _root: &Path, _name: &str, _interval_min: u64) -> Result<()> {
            Ok(())
        }

        fn unregister(&self, _name: &str) -> Result<bool> {
            Ok(false)
        }

        fn registration(&self, _name: &str) -> Result<TaskRegistration> {
            Ok(TaskRegistration::Absent)
        }
    }

    fn record_non_code_success(goals: &GoalStore, root: &Path, goal: &crate::goal::Goal) {
        let command = "echo validation-ok";
        let fingerprint = crate::goal::workspace_fingerprint(root).unwrap();
        let contract_sha256 = goals.validation_contract_hash(&goal.id, "req_1").unwrap();
        goals
            .record_validation_receipt(
                &goal.id,
                "req_1",
                crate::goal::ValidationReceiptSubmission {
                    evidence: "non-code validation passed".into(),
                    command: command.into(),
                    receipt: crate::goal::ValidationReceipt {
                        exit_code: 0,
                        cwd: root.display().to_string(),
                        workspace_fingerprint_before: fingerprint.clone(),
                        workspace_fingerprint_after: fingerprint,
                        stdout_sha256: "a".repeat(64),
                        stderr_sha256: "b".repeat(64),
                        invocation_sha256: crate::goal::validation_invocation_sha256_scoped(
                            command,
                            &[],
                            true,
                        ),
                        passed_tests: None,
                        listed_tests: None,
                        ignored_tests: None,
                        list_stdout_sha256: None,
                        list_stderr_sha256: None,
                        contract_sha256,
                    },
                    impacts: Vec::new(),
                    non_code: true,
                },
            )
            .unwrap();
        goals.close(&goal.id, "success").unwrap();
    }

    #[test]
    fn work_is_complete_needs_goals_none_active_no_pending() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // 无目标 → 未完成。
        assert!(!work_is_complete(root));

        let goals = GoalStore::new(root);
        let g = goals.start("t", &[("do".into(), true)]).unwrap();
        // 有 active 目标 → 未完成。
        assert!(!work_is_complete(root));

        record_non_code_success(&goals, root, &g);
        // 全部关闭、无 pending → 完成。
        assert!(work_is_complete(root));

        // 加一个 pending → 又变未完成。
        PendingStore::new(root).add("leftover", "todo").unwrap();
        assert!(!work_is_complete(root));
    }

    #[test]
    fn task_name_is_stable_and_safe() {
        let dir = tempfile::tempdir().unwrap();
        let a = task_name(dir.path());
        let b = task_name(dir.path());
        assert_eq!(a, b);
        assert!(a.starts_with("RaymanCheckpoint-"));
        assert!(
            a.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[test]
    fn task_xml_contains_key_fields() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let exe = Path::new("C:/tools/rayman.exe");
        let xml = build_task_xml(exe, root, "RaymanCheckpoint-demo", 15);
        assert!(xml.contains("<Interval>PT15M</Interval>"));
        assert!(xml.contains("<StartWhenAvailable>true</StartWhenAvailable>"));
        assert!(xml.contains("autosave tick --workspace"));
        assert!(xml.contains("rayman.exe"));
        assert!(xml.contains("<LogonTrigger>"));
    }

    #[test]
    fn xml_escape_handles_ampersand_and_angles() {
        assert_eq!(xml_escape("a & b <c>"), "a &amp; b &lt;c&gt;");
    }

    #[test]
    fn state_roundtrips_and_stop_marks_inactive() {
        let ws = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let root = ws.path();
        touch(&root.join("src/main.rs"), "fn main() {}");
        activate(root);

        // 直接写一个 active 状态（不触碰真实计划任务）。
        let state = AutosaveState {
            active: true,
            interval_min: 30,
            keep: 2,
            dir: Some(display_path(store.path())),
            auto_stop: true,
            task_name: task_name(root),
            started_at: crate::timefmt::now_iso(),
            last_tick_at: None,
            stopped_at: None,
            stop_status: None,
            consecutive_failures: 0,
            last_error: None,
            last_error_at: None,
        };
        save_state(root, &state).unwrap();
        assert!(load_state(root).unwrap().unwrap().active);

        // stop 会存最后一次快照并标记 inactive（unregister 在非注册状态下是 no-op）。
        let outcome = stop_with_scheduler(root, "success", &AbsentScheduler).unwrap();
        let after = outcome.state.unwrap();
        assert!(!after.active);
        assert_eq!(after.stop_status.as_deref(), Some("success"));
        // 最后一次快照确实落盘。
        let snaps = checkpoint::list(root, Some(store.path())).unwrap();
        assert!(!snaps.is_empty());
        let latest = checkpoint::latest(root, Some(store.path()))
            .unwrap()
            .unwrap();
        let snapshot_state: AutosaveState = serde_json::from_str(
            &fs::read_to_string(
                latest
                    .path
                    .join("tree")
                    .join(".RaymanCodingSkill/autosave.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(
            !snapshot_state.active,
            "the final checkpoint must restore a stopped state"
        );

        let mut stale_active = after.clone();
        stale_active.active = true;
        stale_active.stopped_at = None;
        stale_active.stop_status = None;
        save_state(root, &stale_active).unwrap();
        checkpoint::restore(root, Some(store.path()), Some(&latest.id)).unwrap();
        assert!(!load_state(root).unwrap().unwrap().active);
    }

    /// autosave 的"工作已完成"与 `check --profile standard` 必须永远同判。
    ///
    /// 这两处曾各有一份手写的门禁语义，往其中一处加规则就会产生漂移——真实发生过：
    /// 整目标差量门禁只加到了 check 一侧，autosave 于是会在 check 判定未就绪时
    /// 自停快照。此测试锁死二者共用同一份判定，任何一侧单独演进都会失败。
    #[test]
    fn work_is_complete_agrees_with_the_standard_gate_on_undeclared_drift() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("a.txt"), "a0").unwrap();
        std::fs::write(root.join("b.txt"), "b0").unwrap();
        let goals = GoalStore::new(root);
        let goal = goals.start("drift", &[("ship".into(), true)]).unwrap();
        goals
            .record_plan(
                &goal.id,
                crate::goal::PlanReceiptSubmission {
                    changed_paths: vec!["a.txt".into(), "b.txt".into()],
                    review_priority: "normal".into(),
                    impacted_paths: vec!["a.txt".into(), "b.txt".into()],
                    recommended_checks: Vec::new(),
                },
            )
            .unwrap();

        std::fs::write(root.join("a.txt"), "a1").unwrap();
        // 未建模生态的验证命令不能是自证无关的探针（`echo`/`--version` 之类），
        // 所以这里用一条真实的测试命令；本测试关心的是门禁判定而非命令本身。
        let command = "make test";
        let impacts = vec![crate::goal::ImpactEvidence {
            changed_path: "a.txt".into(),
            package: None,
            manifest_path: None,
            direct_dependencies: Vec::new(),
            direct_dependents: Vec::new(),
            candidate_tests: Vec::new(),
            recommended_checks: Vec::new(),
            recommendation_basis: "test".into(),
            recorded_at: crate::timefmt::now_iso(),
        }];
        let scopes = crate::goal::validation_scopes_for_impacts(&impacts);
        let fingerprint = crate::goal::workspace_fingerprint(root).unwrap();
        goals
            .record_validation_receipt(
                &goal.id,
                "req_1",
                crate::goal::ValidationReceiptSubmission {
                    evidence: "validated a.txt".into(),
                    command: command.into(),
                    receipt: crate::goal::ValidationReceipt {
                        exit_code: 0,
                        cwd: root.display().to_string(),
                        workspace_fingerprint_before: fingerprint.clone(),
                        workspace_fingerprint_after: fingerprint,
                        stdout_sha256: "a".repeat(64),
                        stderr_sha256: "b".repeat(64),
                        invocation_sha256: crate::goal::validation_invocation_sha256_scoped(
                            command, &scopes, false,
                        ),
                        passed_tests: None,
                        listed_tests: None,
                        ignored_tests: None,
                        list_stdout_sha256: None,
                        list_stderr_sha256: None,
                        contract_sha256: crate::goal::validation_contract_sha256(&goal, "req_1")
                            .unwrap(),
                    },
                    impacts,
                    non_code: false,
                },
            )
            .unwrap();
        goals.close(&goal.id, "success").unwrap();
        assert!(work_is_complete(root), "closed success must be complete");

        // b.txt 在 plan 之内但其实际改动从未被任何 receipt 声明。
        std::fs::write(root.join("b.txt"), "b1-undeclared").unwrap();
        let (all, _) = goals.list_with_issues().unwrap();
        let drifted = crate::goal::workspace_fingerprint(root).unwrap();
        let gate_blocked = all.iter().any(|g| {
            !crate::goal::goal_gate_verdict(g, &all, root, Some(&drifted))
                .blockers
                .is_empty()
        });
        assert!(gate_blocked, "standard gate must block undeclared drift");
        assert!(
            !work_is_complete(root),
            "autosave must not call it complete while the standard gate blocks"
        );
    }

    #[test]
    fn work_is_complete_rejects_partial_blocked_and_unknown_statuses() {
        for status in ["partial", "blocked"] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let goals = GoalStore::new(root);
            let goal = goals.start("t", &[("do".into(), true)]).unwrap();
            if status == "blocked" {
                PendingStore::new(root)
                    .add_structured(crate::goal::PendingSubmission {
                        title: "human boundary".into(),
                        detail: "test boundary".into(),
                        goal_id: Some(goal.id.clone()),
                        owner: crate::goal::PendingOwner::Human,
                        kind: crate::goal::PendingKind::HumanInput,
                        attempts: vec!["attempted local path".into()],
                        evidence_paths: vec!["evidence.txt".into()],
                        minimum_input: Some("choose".into()),
                        recommended_action: Some("choose A".into()),
                        alternatives: vec!["choose B".into()],
                        risk: Some("tradeoff".into()),
                        resume_command: Some("rayman prepare --goal test".into()),
                        auto_resume_condition: Some("choice recorded".into()),
                        consultation_timing: crate::goal::ConsultationTiming::Deferred,
                        background_mechanism: None,
                        background_authority_evidence: None,
                        background_isolation_evidence: None,
                    })
                    .unwrap();
            }
            goals.close(&goal.id, status).unwrap();
            assert!(!work_is_complete(root), "{status} must not auto-stop");
        }

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let goals = GoalStore::new(root);
        let goal = goals.start("t", &[("do".into(), true)]).unwrap();
        let path = root
            .join(".RaymanCodingSkill/goals")
            .join(format!("{}.json", goal.id));
        let mut raw: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        raw["status"] = serde_json::Value::String("mystery".into());
        fs::write(&path, serde_json::to_string(&raw).unwrap()).unwrap();
        assert!(!work_is_complete(root), "unknown status must not auto-stop");
    }

    #[test]
    fn work_is_complete_rejects_a_semantically_invalid_success_goal() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let goals = GoalStore::new(root);
        let goal = goals.start("t", &[("do".into(), true)]).unwrap();
        let path = root
            .join(".RaymanCodingSkill/goals")
            .join(format!("{}.json", goal.id));
        let mut raw: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        raw["schema_version"] = serde_json::json!(crate::goal::GOAL_SCHEMA_VERSION + 1);
        raw["status"] = serde_json::Value::String("success".into());
        fs::write(&path, serde_json::to_string(&raw).unwrap()).unwrap();

        assert!(
            !work_is_complete(root),
            "an invalid current-schema goal must not auto-stop"
        );
    }

    #[test]
    fn work_is_complete_rejects_legacy_success_without_a_current_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let goals_dir = root.join(".RaymanCodingSkill/goals");
        fs::create_dir_all(&goals_dir).unwrap();
        let legacy = serde_json::json!({
            "id": "legacy_success",
            "status": "success",
            "contract": {
                "goal": "historical completion",
                "requirements": [{
                    "id": "req_1",
                    "text": "completed historically",
                    "priority": "must",
                    "status": "satisfied",
                    "evidence": "historical evidence"
                }]
            }
        });
        fs::write(
            goals_dir.join("legacy_success.json"),
            serde_json::to_string(&legacy).unwrap(),
        )
        .unwrap();

        assert!(
            !work_is_complete(root),
            "legacy attestation cannot trigger current standard-ready auto-stop"
        );
    }

    #[test]
    fn work_is_complete_ignores_reasoned_history_but_requires_fresh_current_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/lib.rs"), "pub fn answer() -> i32 { 42 }\n");
        let goals = GoalStore::new(root);
        let historical = goals
            .start("historical", &[("old work".into(), true)])
            .unwrap();
        record_non_code_success(&goals, root, &historical);
        goals
            .archive(&historical.id, "older delivery", false)
            .unwrap();

        let current = goals
            .start("current", &[("validate now".into(), true)])
            .unwrap();
        record_non_code_success(&goals, root, &current);
        assert!(work_is_complete(root));

        touch(&root.join("src/lib.rs"), "pub fn answer() -> i32 { 43 }\n");
        assert!(
            !work_is_complete(root),
            "a source change must make the current receipt stale"
        );
    }

    #[test]
    fn activation_failure_never_persists_a_phantom_active_state() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let state = AutosaveState {
            active: true,
            interval_min: 30,
            keep: 2,
            dir: None,
            auto_stop: true,
            task_name: task_name(root),
            started_at: crate::timefmt::now_iso(),
            last_tick_at: None,
            stopped_at: None,
            stop_status: None,
            consecutive_failures: 0,
            last_error: None,
            last_error_at: None,
        };

        let result = activate_state_with(
            root,
            &state,
            || bail!("synthetic registration failure"),
            || Ok(()),
        );
        assert!(result.is_err());
        assert!(load_state(root).unwrap().is_none());
    }

    #[test]
    fn unregister_failure_keeps_persisted_state_active_and_returns_error() {
        let ws = tempfile::tempdir().unwrap();
        let snapshots = tempfile::tempdir().unwrap();
        let root = ws.path();
        touch(&root.join("src/lib.rs"), "pub fn answer() -> i32 { 42 }\n");
        let mut state = AutosaveState {
            active: true,
            interval_min: 30,
            keep: 2,
            dir: Some(display_path(snapshots.path())),
            auto_stop: true,
            task_name: task_name(root),
            started_at: crate::timefmt::now_iso(),
            last_tick_at: None,
            stopped_at: None,
            stop_status: None,
            consecutive_failures: 0,
            last_error: None,
            last_error_at: None,
        };
        save_state(root, &state).unwrap();

        let result = finalize_with(
            root,
            &mut state,
            "success",
            || bail!("synthetic unregister failure"),
            || Ok(()),
            || Ok(()),
        );
        assert!(result.is_err());
        let persisted = load_state(root).unwrap().unwrap();
        assert!(persisted.active);
        assert!(persisted.stopped_at.is_none());
        assert!(persisted.stop_status.is_none());
    }

    #[test]
    fn state_write_failure_after_unregister_reregisters_and_stays_active() {
        use std::cell::Cell;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut state = AutosaveState {
            active: true,
            interval_min: 30,
            keep: 2,
            dir: None,
            auto_stop: true,
            task_name: task_name(root),
            started_at: crate::timefmt::now_iso(),
            last_tick_at: None,
            stopped_at: None,
            stop_status: None,
            consecutive_failures: 0,
            last_error: None,
            last_error_at: None,
        };
        save_state(root, &state).unwrap();
        let reregistered = Cell::new(false);

        let result = finalize_state_with(
            &mut state,
            "success",
            || Ok(false),
            |_| bail!("synthetic state write failure"),
            || {
                reregistered.set(true);
                Ok(())
            },
        );
        assert!(result.is_err());
        assert!(reregistered.get());
        assert!(state.active);
        assert!(load_state(root).unwrap().unwrap().active);
    }

    /// tick/stop 现在在内部检查目标工作区的激活状态（激活破损按失败记账/跳过
    /// 最终快照），所以走真实 tick/stop 路径的测试必须先激活工作区，否则测试
    /// 的本意（如 partial 快照上限）会被激活门禁静默架空。
    fn activate(root: &Path) {
        let skill = root.join("SKILL.md");
        fs::write(&skill, "test skill\n").unwrap();
        crate::workspace::activate(root, &skill).unwrap();
    }

    fn active_state(root: &Path, store: Option<&Path>) -> AutosaveState {
        AutosaveState {
            active: true,
            interval_min: 30,
            keep: 2,
            dir: store.map(display_path),
            auto_stop: false,
            task_name: task_name(root),
            started_at: crate::timefmt::now_iso(),
            last_tick_at: None,
            stopped_at: None,
            stop_status: None,
            consecutive_failures: 0,
            last_error: None,
            last_error_at: None,
        }
    }

    /// 一个反复失败的 autosave 曾经完全静默：tick 的输出无人可见，partial 快照又
    /// 无上限累积（约 48 份/天），而 `status` 依旧显示"运行中"。失败必须计数、
    /// 必须出现在 status 里，partial 也必须有上限。
    #[test]
    fn repeated_tick_failures_are_counted_capped_and_surfaced_in_status() {
        let ws = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let root = ws.path();
        touch(&root.join("src/main.rs"), "fn main() {}");
        activate(root);
        save_state(root, &active_state(root, Some(store.path()))).unwrap();

        // goals 应为目录；同名普通文件让此后每次 checkpoint::save 都失败。
        touch(&root.join(".RaymanCodingSkill/goals"), "not a directory");

        let attempts = checkpoint::MAX_PARTIAL_SNAPSHOTS + 2;
        for _ in 0..attempts {
            assert!(tick_with_scheduler(root, &AbsentScheduler).is_err());
        }

        let persisted = load_state(root).unwrap().unwrap();
        assert_eq!(persisted.consecutive_failures as usize, attempts);
        assert!(persisted.last_error.is_some());
        assert!(persisted.last_error_at.is_some());

        let partials = checkpoint::list(root, Some(store.path()))
            .unwrap()
            .iter()
            .filter(|snapshot| snapshot.status == checkpoint::SnapshotStatus::Partial)
            .count();
        assert!(
            partials <= checkpoint::MAX_PARTIAL_SNAPSHOTS,
            "失败的 autosave 循环不得无界堆积 partial 快照（{attempts} 次后实得 {partials} 份）"
        );

        let message = status_with_scheduler(root, &AbsentScheduler)
            .unwrap()
            .message;
        assert!(
            message.contains(&format!("连续失败：{attempts} 次")),
            "status 必须让用户看见自动保存其实一直没成功: {message}"
        );
    }

    #[test]
    fn a_successful_tick_clears_a_previous_failure_streak() {
        let ws = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let root = ws.path();
        touch(&root.join("src/main.rs"), "fn main() {}");
        activate(root);
        let mut state = active_state(root, Some(store.path()));
        state.consecutive_failures = 7;
        state.last_error = Some("previous failure".into());
        state.last_error_at = Some(crate::timefmt::now_iso());
        save_state(root, &state).unwrap();

        tick_with_scheduler(root, &AbsentScheduler).unwrap();

        let persisted = load_state(root).unwrap().unwrap();
        assert_eq!(persisted.consecutive_failures, 0);
        assert!(persisted.last_error.is_none());
        assert!(persisted.last_error_at.is_none());
        let message = status_with_scheduler(root, &AbsentScheduler)
            .unwrap()
            .message;
        assert!(!message.contains("连续失败"), "{message}");
    }

    /// 免顶层门禁的入口不得在从未激活的目录里凭空创建 `.RaymanCodingSkill/`：
    /// 那既是 workspace 标记（劫持子目录的 workspace-root 解析），又会被后续
    /// 命令误诊为 orphan_state。status/stop 报激活错误，tick 只做遗留任务清理。
    #[test]
    fn exempted_entrypoints_do_not_mint_state_in_unactivated_dirs() {
        let ws = tempfile::tempdir().unwrap();
        let root = ws.path();

        assert!(status(root).is_err());
        assert!(stop_entry(root, "error", &AbsentScheduler).is_err());
        let outcome = tick_entry(root, &AbsentScheduler).unwrap();
        assert!(outcome.state.is_none());
        assert!(
            !root.join(".RaymanCodingSkill").exists(),
            "免门禁入口不得创建受管状态目录"
        );
    }

    /// 激活破损（SKILL.md 漂移、CLI 升级）时，tick 必须仍然把失败写进
    /// autosave.json——计划任务的输出无人可见，`autosave status` 是唯一的
    /// 发现渠道；同时不得给破损工作区铸造 standard 快照。
    #[test]
    fn tick_persists_failure_when_activation_is_broken() {
        let ws = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let root = ws.path();
        touch(&root.join("src/main.rs"), "fn main() {}");
        activate(root);
        save_state(root, &active_state(root, Some(store.path()))).unwrap();
        fs::write(root.join("SKILL.md"), "drifted content\n").unwrap();

        assert!(tick_with_scheduler(root, &AbsentScheduler).is_err());
        let persisted = load_state(root).unwrap().unwrap();
        assert_eq!(persisted.consecutive_failures, 1);
        assert!(
            persisted
                .last_error
                .as_deref()
                .unwrap_or_default()
                .contains("未激活"),
            "{:?}",
            persisted.last_error
        );
        assert!(persisted.last_error_at.is_some());
        assert!(
            checkpoint::list(root, Some(store.path()))
                .unwrap()
                .is_empty(),
            "激活破损的工作区不得铸造 standard 快照"
        );
        let message = status_with_scheduler(root, &AbsentScheduler)
            .unwrap()
            .message;
        assert!(message.contains("连续失败"), "{message}");
    }

    /// 激活破损时 stop 仍必须能注销计划任务并落盘停止状态（否则 tick 永远
    /// 失败下去、无路可停），但最终快照跳过。
    #[test]
    fn stop_on_broken_activation_unregisters_without_minting_a_snapshot() {
        let ws = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let root = ws.path();
        touch(&root.join("src/main.rs"), "fn main() {}");
        activate(root);
        save_state(root, &active_state(root, Some(store.path()))).unwrap();
        fs::write(root.join("SKILL.md"), "drifted content\n").unwrap();

        let outcome = stop_with_scheduler(root, "error", &AbsentScheduler).unwrap();
        let after = outcome.state.unwrap();
        assert!(!after.active);
        assert_eq!(after.stop_status.as_deref(), Some("error"));
        assert!(
            after
                .last_error
                .as_deref()
                .unwrap_or_default()
                .contains("最终快照已跳过"),
            "{:?}",
            after.last_error
        );
        assert!(!load_state(root).unwrap().unwrap().active);
        assert!(
            checkpoint::list(root, Some(store.path()))
                .unwrap()
                .is_empty(),
            "激活破损的工作区不得铸造 standard 最终快照"
        );
    }

    /// 计划任务 XML 必须独占创建在受管临时目录里，而不是 PID 可预测的 `%TEMP%`
    /// 裸写——那条路径会跟随符号链接、可截断，且与 schtasks 读取之间有 TOCTOU 窗口。
    #[test]
    fn task_xml_is_created_exclusively_under_the_managed_temp_root() {
        let ws = tempfile::tempdir().unwrap();
        let root = ws.path();
        let bytes = utf16le_bom("<Task/>");

        let first = write_task_xml_exclusive(root, &bytes).unwrap();
        let expected_parent = root
            .canonicalize()
            .unwrap()
            .join(".RaymanCodingSkill")
            .join("tmp")
            .join("autosave");
        assert_eq!(first.parent().unwrap(), expected_parent);
        assert_eq!(fs::read(&first).unwrap(), bytes);
        assert!(
            !fs::symlink_metadata(&first)
                .unwrap()
                .file_type()
                .is_symlink(),
            "写入不得落在链接上"
        );

        // 独占创建：绝不复用/截断一个已经存在的名字。
        let second = write_task_xml_exclusive(root, &bytes).unwrap();
        assert_ne!(second, first);
        assert_eq!(fs::read(&first).unwrap(), bytes);
    }

    #[cfg(unix)]
    #[test]
    fn task_xml_refuses_a_symlinked_managed_temp_root() {
        use std::os::unix::fs::symlink;

        let ws = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = ws.path();
        fs::create_dir_all(root.join(".RaymanCodingSkill")).unwrap();
        symlink(outside.path(), root.join(".RaymanCodingSkill/tmp")).unwrap();

        assert!(write_task_xml_exclusive(root, b"payload").is_err());
        assert_eq!(
            fs::read_dir(outside.path()).unwrap().count(),
            0,
            "绝不把计划任务 XML 写到链接目标里"
        );
    }

    #[test]
    fn corrupt_autosave_state_is_never_overwritten_by_lifecycle_actions() {
        let ws = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let root = ws.path();
        touch(&root.join("src/main.rs"), "fn main() {}");
        let path = state_path(root, true).unwrap();
        touch(&path, "{ not json");

        assert!(start(root, 30, 1, true, Some(store.path())).is_err());
        assert!(tick(root).is_err());
        assert!(stop(root, "error").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");
        assert!(status(root).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn scheduler_not_found_classification_accepts_path_missing_but_not_generic_failures() {
        assert!(scheduler_reports_not_found(
            Some(1),
            "ERROR: The system cannot find the path specified."
        ));
        assert!(scheduler_reports_not_found(
            Some(1),
            "错误: 系统找不到指定的路径。"
        ));
        assert!(scheduler_reports_not_found(Some(3), ""));
        assert!(scheduler_reports_not_found(Some(-2_147_024_893), ""));
        assert!(!scheduler_reports_not_found(
            Some(1),
            "ERROR: Access is denied."
        ));
        assert!(!scheduler_reports_not_found(
            Some(1),
            "ERROR: The Task Scheduler service is not available."
        ));
    }
}
