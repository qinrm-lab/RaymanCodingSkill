use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use rayman_core::rayman_reminder_exe_name;

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const REMINDER_IDLE_SECONDS: &str = "10";
const SUBAGENT_SCOPE: &str = "subagent";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReminderTrigger {
    GoalStopped,
    GoalClosed,
    SessionClosed,
}

impl ReminderTrigger {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::GoalStopped => "goal_stopped",
            Self::GoalClosed => "goal_closed",
            Self::SessionClosed => "session_closed",
        }
    }
}

#[must_use = "ReminderGuard must stay alive until the command exits"]
pub(crate) struct ReminderGuard {
    trigger: ReminderTrigger,
    fired: bool,
}

impl ReminderGuard {
    pub(crate) fn arm(trigger: ReminderTrigger) -> Self {
        Self {
            trigger,
            fired: false,
        }
    }

    pub(crate) fn fire_once(&mut self) {
        if self.fired {
            return;
        }
        self.fired = true;
        start(self.trigger);
    }
}

impl Drop for ReminderGuard {
    fn drop(&mut self) {
        self.fire_once();
    }
}

pub(crate) fn start(trigger: ReminderTrigger) {
    let _ = try_start(trigger);
}

fn try_start(trigger: ReminderTrigger) -> Result<()> {
    if reminder_disabled_by_environment() {
        return Ok(());
    }
    if let Some(path) = std::env::var_os("RAYMAN_REMINDER_LOG_PATH") {
        return append_trace(path, trigger);
    }
    let current_exe = std::env::current_exe().context("无法读取当前 rayman 可执行文件路径")?;
    let Some(reminder) = reminder_binary_path(&current_exe) else {
        return Ok(());
    };
    spawn_reminder(&reminder, trigger)?;
    Ok(())
}

fn reminder_disabled_by_environment() -> bool {
    std::env::var_os("RAYMAN_DISABLE_REMINDER").is_some()
        || env_is("RAYMAN_REMINDER_SCOPE", SUBAGENT_SCOPE)
        || env_is("RAYMAN_AGENT_ROLE", SUBAGENT_SCOPE)
        || env_is("RAYMAN_SUBAGENT", "1")
}

fn env_is(name: &str, expected: &str) -> bool {
    std::env::var(name)
        .map(|value| value.eq_ignore_ascii_case(expected))
        .unwrap_or(false)
}

fn append_trace(path: OsString, trigger: ReminderTrigger) -> Result<()> {
    let path = PathBuf::from(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(file, "{}", trigger.as_str())?;
    Ok(())
}

fn reminder_binary_path(current_exe: &Path) -> Option<PathBuf> {
    let exe_name = rayman_reminder_exe_name();
    let parent = current_exe.parent()?;
    let sibling = parent.join(exe_name);
    if sibling.exists() {
        return Some(sibling);
    }
    if parent.file_name().and_then(|name| name.to_str()) == Some("deps")
        && let Some(debug_dir) = parent.parent()
    {
        let debug_sibling = debug_dir.join(exe_name);
        if debug_sibling.exists() {
            return Some(debug_sibling);
        }
    }
    None
}

fn spawn_reminder(path: &Path, trigger: ReminderTrigger) -> Result<()> {
    let mut command = Command::new(path);
    command
        .arg("--trigger")
        .arg(trigger.as_str())
        .arg("--idle-seconds")
        .arg(REMINDER_IDLE_SECONDS)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    apply_background_flags(&mut command);
    command
        .spawn()
        .with_context(|| format!("无法后台启动提醒程序: {}", path.display()))?;
    Ok(())
}

#[cfg(windows)]
fn apply_background_flags(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn apply_background_flags(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_names_are_stable() {
        assert_eq!(ReminderTrigger::GoalStopped.as_str(), "goal_stopped");
        assert_eq!(ReminderTrigger::GoalClosed.as_str(), "goal_closed");
        assert_eq!(ReminderTrigger::SessionClosed.as_str(), "session_closed");
    }

    #[test]
    fn reminder_binary_path_checks_current_and_deps_siblings() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let exe_name = rayman_reminder_exe_name();
        let debug = temp.path().join("target").join("debug");
        let deps = debug.join("deps");
        std::fs::create_dir_all(&deps)?;
        let reminder = debug.join(exe_name);
        std::fs::write(&reminder, b"reminder")?;
        let current = deps.join(if cfg!(windows) {
            "rayman-test.exe"
        } else {
            "rayman-test"
        });

        assert_eq!(reminder_binary_path(&current), Some(reminder));
        Ok(())
    }
}
