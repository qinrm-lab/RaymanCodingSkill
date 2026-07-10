//! 工作树自动快照的生命周期（Windows）：
//! - `start`：立刻存一次快照 + 注册一个每 N 分钟触发的计划任务（幂等，每次开工跑一遍即可）。
//! - `tick`：计划任务每次触发时跑；存一次快照；若开启了 auto-stop 且工作已完成，则存最后一次并自停。
//! - `stop`：存最后一次快照 + 注销计划任务（“全部完成”或“出错”时调用）。
//!
//! 计划任务用 Windows 内置 `schtasks` + 任务 XML 注册，XML 里开了 `StartWhenAvailable`，
//! 断电/关机错过的那次会在开机后补跑；另挂一个登录触发器，重启登录后自动接着跑。

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::goal::{GoalStore, PendingStore};
use crate::state_store::{self, display_path};
use crate::{checkpoint, workspace_root};

const STATE_RELATIVE: &str = ".RaymanCodingSkill/autosave.json";
const DEFAULT_INTERVAL_MIN: u64 = 30;

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
}

fn state_path(root: &Path) -> PathBuf {
    root.join(STATE_RELATIVE)
}

fn load_state(root: &Path) -> Option<AutosaveState> {
    state_store::read_json(&state_path(root)).ok().flatten()
}

fn save_state(root: &Path, state: &AutosaveState) -> Result<()> {
    state_store::write_json(&state_path(root), state)
}

/// 计划任务名：每个工作区一个，稳定且唯一。
pub fn task_name(root: &Path) -> String {
    format!("RaymanCheckpoint-{}", checkpoint::workspace_key(root))
}

/// 工作是否“全部完成”：有目标、且没有仍处于 active 的目标、且没有待完成项。
/// 没有任何目标时返回 false（无从判断完成，交给显式 `stop`）。
/// 任何状态文件读不出来都按“未完成”处理：损坏的 active 目标被当成不存在
/// 会导致自动快照在工作进行中自停并注销。
pub fn work_is_complete(root: &Path) -> bool {
    let Ok((goals, issues)) = GoalStore::new(root).list_with_issues() else {
        return false;
    };
    if !issues.is_empty() || goals.is_empty() {
        return false;
    }
    if goals.iter().any(|goal| goal.status == "active") {
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
    let interval_min = interval_min.max(1);
    let saved = checkpoint::save(root, dir, keep)?;

    let name = task_name(root);
    let state = AutosaveState {
        active: true,
        interval_min,
        keep,
        dir: dir.map(display_path),
        auto_stop,
        task_name: name.clone(),
        started_at: state_store::now_iso(),
        last_tick_at: None,
        stopped_at: None,
        stop_status: None,
    };
    save_state(root, &state)?;
    register_task(root, &name, interval_min)?;

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
    let Some(mut state) = load_state(root) else {
        // 没有状态：不该有任务在跑，尽力注销后退出。
        let name = task_name(root);
        let _ = unregister_task(&name);
        return Ok(ActionOutcome {
            message: "无自动保存状态，已尝试注销计划任务。".into(),
            state: None,
        });
    };
    if !state.active {
        let _ = unregister_task(&state.task_name);
        return Ok(ActionOutcome {
            message: "自动保存已停止，已注销计划任务。".into(),
            state: Some(state),
        });
    }

    let saved = checkpoint::save(root, dir_override(&state.dir).as_deref(), state.keep)?;
    state.last_tick_at = Some(state_store::now_iso());
    save_state(root, &state)?;

    if state.auto_stop && work_is_complete(root) {
        finalize(root, &mut state, "success (auto)")?;
        return Ok(ActionOutcome {
            message: format!(
                "已存快照 {} 并检测到工作完成：存为最后一次，已停止自动保存。",
                saved.id
            ),
            state: Some(state),
        });
    }

    Ok(ActionOutcome {
        message: format!("已存快照 {}（{} 个文件）。", saved.id, saved.file_count),
        state: Some(state),
    })
}

/// 显式停止（“全部完成”传 success，“出错”传 error 等）：存最后一次快照 + 注销任务。
pub fn stop(root: &Path, status: &str) -> Result<ActionOutcome> {
    let mut state = load_state(root).unwrap_or_else(|| AutosaveState {
        active: false,
        interval_min: DEFAULT_INTERVAL_MIN,
        keep: checkpoint::DEFAULT_KEEP,
        dir: None,
        auto_stop: true,
        task_name: task_name(root),
        started_at: state_store::now_iso(),
        last_tick_at: None,
        stopped_at: None,
        stop_status: None,
    });
    finalize(root, &mut state, status)?;
    Ok(ActionOutcome {
        message: format!(
            "已存最后一次快照并停止自动保存（状态：{status}）。计划任务 '{}' 已注销。",
            state.task_name
        ),
        state: Some(state),
    })
}

/// 存最后一次快照，标记停止，注销任务。
fn finalize(root: &Path, state: &mut AutosaveState, status: &str) -> Result<()> {
    let _ = checkpoint::save(root, dir_override(&state.dir).as_deref(), state.keep)?;
    state.active = false;
    state.stopped_at = Some(state_store::now_iso());
    state.stop_status = Some(status.to_string());
    save_state(root, state)?;
    let _ = unregister_task(&state.task_name);
    Ok(())
}

/// 当前自动保存状态摘要。
pub fn status(root: &Path) -> ActionOutcome {
    match load_state(root) {
        None => ActionOutcome {
            message: "未启用自动保存。运行 `rayman autosave start` 开启。".into(),
            state: None,
        },
        Some(state) => {
            let registered = task_registered(&state.task_name);
            let last = state
                .last_tick_at
                .clone()
                .unwrap_or_else(|| "（尚无）".into());
            ActionOutcome {
                message: format!(
                    "自动保存：{}（每 {} 分钟，keep={}，auto_stop={}）\n  计划任务 '{}'：{}\n  最近一次触发：{}",
                    if state.active {
                        "运行中"
                    } else {
                        "已停止"
                    },
                    state.interval_min,
                    state.keep,
                    state.auto_stop,
                    state.task_name,
                    if registered { "已注册" } else { "未注册" },
                    last
                ),
                state: Some(state),
            }
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

#[cfg(windows)]
fn register_task(root: &Path, name: &str, interval_min: u64) -> Result<()> {
    let exe = std::env::current_exe()?;
    let xml = build_task_xml(&exe, root, name, interval_min);

    // schtasks /XML 期望 UTF-16LE 文件；用带 BOM 的 UTF-16LE 写出，兼容非 ASCII 路径。
    let xml_path = std::env::temp_dir().join(format!("rayman-task-{}.xml", std::process::id()));
    std::fs::write(&xml_path, utf16le_bom(&xml))?;

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
    let output = std::process::Command::new("schtasks")
        .args(["/Delete", "/TN", name, "/F"])
        .output()?;
    Ok(output.status.success())
}

#[cfg(windows)]
fn task_registered(name: &str) -> bool {
    std::process::Command::new("schtasks")
        .args(["/Query", "/TN", name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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
fn task_registered(_name: &str) -> bool {
    false
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

        goals
            .record_evidence(&g.id, "req_1", "src/x + test passed")
            .unwrap();
        goals.close(&g.id, "success").unwrap();
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

        // 直接写一个 active 状态（不触碰真实计划任务）。
        let state = AutosaveState {
            active: true,
            interval_min: 30,
            keep: 2,
            dir: Some(display_path(store.path())),
            auto_stop: true,
            task_name: task_name(root),
            started_at: state_store::now_iso(),
            last_tick_at: None,
            stopped_at: None,
            stop_status: None,
        };
        save_state(root, &state).unwrap();
        assert!(load_state(root).unwrap().active);

        // stop 会存最后一次快照并标记 inactive（unregister 在非注册状态下是 no-op）。
        let outcome = stop(root, "success").unwrap();
        let after = outcome.state.unwrap();
        assert!(!after.active);
        assert_eq!(after.stop_status.as_deref(), Some("success"));
        // 最后一次快照确实落盘。
        let snaps = checkpoint::list(root, Some(store.path())).unwrap();
        assert!(!snaps.is_empty());
    }
}
