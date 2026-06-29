use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::models::{AgentManager, RouteAttempt};
use crate::session::SessionManager;
use crate::{display_path, ensure_within, now_iso};

const TERMINAL_STATUSES: &[&str] = &["reconciled", "blocked", "policy_violation"];
const UNRESOLVED_STATUSES: &[&str] = &["conflict", "blocked", "policy_violation"];
const RESEARCH_ROLES: &[&str] = &[
    "planner",
    "scientist",
    "critic",
    "reflector",
    "arbiter",
    "safety_monitor",
];
const DEFAULT_EXPERIMENT_TIMEOUT_SECONDS: u64 = 120;
const MAX_EXPERIMENT_TIMEOUT_SECONDS: u64 = 120;
const MAX_TAIL_CHARS: usize = 4000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResearchAutonomyPolicy {
    pub can_run_experiments: bool,
    pub can_edit_files: bool,
    pub can_close_goals: bool,
}

impl Default for ResearchAutonomyPolicy {
    fn default() -> Self {
        Self {
            can_run_experiments: true,
            can_edit_files: false,
            can_close_goals: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResearchCommandPolicy {
    pub allow_network: bool,
    pub require_workspace_cwd: bool,
    pub reject_shell_operators: bool,
    pub diff_check_repo_tracked_files: bool,
    pub allowed: Vec<Vec<String>>,
}

impl Default for ResearchCommandPolicy {
    fn default() -> Self {
        Self {
            allow_network: false,
            require_workspace_cwd: true,
            reject_shell_operators: true,
            diff_check_repo_tracked_files: true,
            allowed: vec![
                words("rayman context status"),
                words("rayman context task"),
                words("rayman impact"),
                words("rayman regression plan"),
                words("rayman eval run"),
                words("rayman security audit"),
                words("cargo fmt --check"),
                words("cargo clippy --all-targets -- -D warnings"),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResearchConfig {
    pub enabled: bool,
    pub max_parallel_agents: usize,
    pub autonomy: ResearchAutonomyPolicy,
    pub command_policy: ResearchCommandPolicy,
}

impl Default for ResearchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_parallel_agents: 4,
            autonomy: ResearchAutonomyPolicy::default(),
            command_policy: ResearchCommandPolicy::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResearchSession {
    pub id: String,
    pub goal_id: Option<String>,
    pub question: String,
    pub status: String,
    pub current_stage: String,
    pub autonomy_policy: ResearchAutonomyPolicy,
    pub created_at: String,
    pub updated_at: String,
    pub hypotheses: Vec<ResearchHypothesis>,
    pub experiments: Vec<ResearchExperiment>,
    pub reflections: Vec<ResearchReflection>,
    pub findings: Vec<ResearchAgentFinding>,
    pub conflicts: Vec<ResearchConflict>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResearchHypothesis {
    pub id: String,
    pub claim: String,
    pub rationale: String,
    pub expected_observation: String,
    pub falsification_tests: Vec<String>,
    pub prior_confidence: f64,
    pub posterior_confidence: Option<f64>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExperimentCommand {
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResearchExperiment {
    pub id: String,
    pub hypothesis_id: Option<String>,
    pub purpose: String,
    pub command: ExperimentCommand,
    pub status: String,
    pub exit_code: Option<i32>,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub duration_ms: u128,
    pub policy_violation: Option<String>,
    pub changed_files: Vec<String>,
    pub started_at: String,
    pub finished_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResearchReflection {
    pub id: String,
    pub experiment_id: Option<String>,
    pub expected: String,
    pub observed: String,
    pub mismatch: Option<String>,
    pub lesson: String,
    pub next_hypotheses: Vec<String>,
    pub quality_pattern_candidates: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResearchAgentFinding {
    pub id: String,
    pub role: String,
    pub status: String,
    pub model_ref: Option<String>,
    #[serde(default = "default_research_execution_mode")]
    pub execution_mode: String,
    #[serde(default)]
    pub duration_ms: u128,
    #[serde(default)]
    pub route_attempts: Vec<RouteAttempt>,
    #[serde(default)]
    pub error: Option<String>,
    pub prompt_hash: String,
    pub response_hash: String,
    pub summary: String,
    #[serde(default = "default_research_evidence_status")]
    pub evidence_status: String,
    pub evidence_refs: Vec<String>,
    pub confidence: f64,
    pub risk_level: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResearchConflict {
    pub id: String,
    pub status: String,
    pub severity: String,
    pub reason: String,
    pub evidence_refs: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResearchStats {
    pub total_sessions: u64,
    pub active_sessions: u64,
    pub reconciled_sessions: u64,
    pub conflicted_sessions: u64,
    pub blocked_sessions: u64,
    pub policy_violations: u64,
    pub experiments: u64,
    pub unresolved_conflicts: u64,
}

#[derive(Debug, Clone)]
pub struct ResearchManager {
    workspace: PathBuf,
    sessions_dir: PathBuf,
    config: ResearchConfig,
}

impl ResearchManager {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        let workspace = workspace
            .into()
            .canonicalize()
            .context("无法解析工作区路径")?;
        let sessions_dir = ensure_within(
            &workspace
                .join(".RaymanCodingSkill")
                .join("research")
                .join("sessions"),
            &workspace,
            "Research 状态目录必须位于工作区内",
        )?;
        let config = ResearchConfig::load(&workspace)?;
        Ok(Self {
            workspace,
            sessions_dir,
            config,
        })
    }

    pub fn config(&self) -> &ResearchConfig {
        &self.config
    }

    pub fn start(&self, question: &str, goal_id: Option<String>) -> Result<ResearchSession> {
        if !self.config.enabled {
            bail!("research_agents.enabled=false");
        }
        if self.config.autonomy.can_edit_files {
            bail!("scientist autonomy policy violation: can_edit_files must be false");
        }
        if self.config.autonomy.can_close_goals {
            bail!("scientist autonomy policy violation: can_close_goals must be false");
        }
        if question.trim().is_empty() {
            bail!("research question 不能为空");
        }
        let created_at = now_iso();
        let id = research_id(question, &created_at);
        let session = ResearchSession {
            id,
            goal_id,
            question: question.trim().to_string(),
            status: "active".into(),
            current_stage: "question".into(),
            autonomy_policy: self.config.autonomy.clone(),
            created_at: created_at.clone(),
            updated_at: created_at,
            hypotheses: Vec::new(),
            experiments: Vec::new(),
            reflections: Vec::new(),
            findings: Vec::new(),
            conflicts: Vec::new(),
        };
        self.write_session(&session)?;
        Ok(session)
    }

    pub fn run_once(
        &self,
        id: Option<&str>,
        agent: Option<&mut AgentManager>,
    ) -> Result<ResearchSession> {
        let mut session = self.resolve_session(id)?;
        if TERMINAL_STATUSES.contains(&session.status.as_str()) {
            return Ok(session);
        }
        if !session.autonomy_policy.can_run_experiments {
            session.status = "blocked".into();
            session.current_stage = "blocked".into();
            session.conflicts.push(ResearchConflict {
                id: format!("conflict_{}", session.conflicts.len() + 1),
                status: "open".into(),
                severity: "high".into(),
                reason: "scientist cannot run experiments under current autonomy policy".into(),
                evidence_refs: Vec::new(),
                created_at: now_iso(),
            });
            self.write_session(&session)?;
            self.add_conflict_pending(&session)?;
            return Ok(session);
        }

        let mut scientist_response = session
            .findings
            .iter()
            .rev()
            .find(|finding| finding.role == "scientist" && finding.status == "succeeded")
            .map(|finding| finding.summary.clone());
        for finding in self.role_findings(&session, agent)? {
            if finding.role == "scientist" && finding.status == "succeeded" {
                scientist_response = Some(finding.summary.clone());
            }
            session.findings.push(finding);
        }

        if session.hypotheses.is_empty() {
            session.hypotheses.push(default_hypothesis(
                &session.question,
                scientist_response.as_deref(),
            ));
        }
        session.current_stage = "hypothesis_set".into();

        let command = scientist_response
            .as_deref()
            .and_then(parse_scientist_command)
            .unwrap_or_else(|| default_experiment_command(&session.question));
        let experiment = self.run_experiment(
            format!("exp_{}", session.experiments.len() + 1),
            session
                .hypotheses
                .first()
                .map(|hypothesis| hypothesis.id.clone()),
            "scientist-requested whitelist experiment".into(),
            command,
        )?;
        let experiment_id = experiment.id.clone();
        let experiment_failed = experiment.status != "succeeded";
        let policy_violation = experiment.policy_violation.clone();
        let changed_files = experiment.changed_files.clone();
        session.experiments.push(experiment);
        session.current_stage = "evidence_captured".into();

        let reflection = self.reflect(&session, &experiment_id);
        session.reflections.push(reflection);
        session.current_stage = "reflection".into();

        if let Some(violation) = policy_violation {
            session.status = "policy_violation".into();
            session.current_stage = "policy_violation".into();
            session.conflicts.push(ResearchConflict {
                id: format!("conflict_{}", session.conflicts.len() + 1),
                status: "open".into(),
                severity: "critical".into(),
                reason: format!("experiment changed protected files: {violation}"),
                evidence_refs: changed_files,
                created_at: now_iso(),
            });
            self.add_conflict_pending(&session)?;
        } else if experiment_failed {
            session.status = "conflict".into();
            session.current_stage = "conflict".into();
            session.conflicts.push(ResearchConflict {
                id: format!("conflict_{}", session.conflicts.len() + 1),
                status: "open".into(),
                severity: "medium".into(),
                reason: "scientist experiment failed; primary AI must triage before success".into(),
                evidence_refs: vec![experiment_id],
                created_at: now_iso(),
            });
            self.add_conflict_pending(&session)?;
        } else {
            if let Some(hypothesis) = session.hypotheses.first_mut() {
                hypothesis.posterior_confidence =
                    Some((hypothesis.prior_confidence + 0.1).min(0.95));
                hypothesis.status = "supported_by_experiment".into();
            }
            session.status = "reconciled".into();
            session.current_stage = "reconciled".into();
        }
        session.updated_at = now_iso();
        self.write_session(&session)?;
        Ok(session)
    }

    pub fn reconcile(&self, id: Option<&str>) -> Result<ResearchSession> {
        let mut session = self.resolve_session(id)?;
        for conflict in &mut session.conflicts {
            if conflict.status == "open" && conflict.severity != "critical" {
                conflict.status = "acknowledged".into();
            }
        }
        if session
            .conflicts
            .iter()
            .any(|conflict| conflict.status == "open")
        {
            session.status = "blocked".into();
            session.current_stage = "blocked".into();
        } else if session.status != "policy_violation" {
            session.status = "reconciled".into();
            session.current_stage = "reconciled".into();
        }
        session.updated_at = now_iso();
        self.write_session(&session)?;
        Ok(session)
    }

    pub fn status(&self, id: Option<&str>) -> Result<Value> {
        if let Some(id) = id {
            return Ok(serde_json::to_value(self.read_session(id)?)?);
        }
        Ok(json!({
            "workspace_path": display_path(&self.workspace),
            "sessions_dir": display_path(&self.sessions_dir),
            "config": self.config,
            "stats": self.stats()?,
            "next_session": self.next_active_session()?.map(serde_json::to_value).transpose()?.unwrap_or(Value::Null),
        }))
    }

    pub fn report(&self, id: Option<&str>) -> Result<Value> {
        let session = self.resolve_session(id)?;
        Ok(json!({
            "session": session,
            "source_policy": "Research ledger is advisory evidence only. Scientist experiments may run whitelist commands but cannot edit files, approve validation, or close goals.",
            "required_actions": self.required_actions(&session),
        }))
    }

    pub fn stats(&self) -> Result<ResearchStats> {
        let mut stats = ResearchStats::default();
        for session in self.list_sessions()? {
            stats.total_sessions += 1;
            stats.experiments += session.experiments.len() as u64;
            stats.unresolved_conflicts += session
                .conflicts
                .iter()
                .filter(|conflict| conflict.status == "open")
                .count() as u64;
            match session.status.as_str() {
                "active" | "conflict" => stats.active_sessions += 1,
                "reconciled" => stats.reconciled_sessions += 1,
                "blocked" => stats.blocked_sessions += 1,
                "policy_violation" => {
                    stats.blocked_sessions += 1;
                    stats.policy_violations += 1;
                }
                _ => {}
            }
            if session.status == "conflict" {
                stats.conflicted_sessions += 1;
            }
        }
        Ok(stats)
    }

    pub fn unresolved_blockers(&self) -> Result<Vec<ResearchSession>> {
        Ok(self
            .list_sessions()?
            .into_iter()
            .filter(|session| {
                UNRESOLVED_STATUSES.contains(&session.status.as_str())
                    || session
                        .conflicts
                        .iter()
                        .any(|conflict| conflict.status == "open")
            })
            .collect())
    }

    fn role_findings(
        &self,
        session: &ResearchSession,
        agent: Option<&mut AgentManager>,
    ) -> Result<Vec<ResearchAgentFinding>> {
        let roles = RESEARCH_ROLES;
        let base_sequence = session.findings.len();
        let Some(agent) = agent else {
            return roles
                .iter()
                .enumerate()
                .map(|(index, role)| {
                    Self::role_finding(session, role, base_sequence + index + 1, None)
                })
                .collect();
        };
        let base_agent = agent.clone();
        run_research_role_jobs(
            roles,
            self.config.max_parallel_agents,
            |role_index, role| {
                let mut role_agent = base_agent.clone();
                Self::role_finding(
                    session,
                    role,
                    base_sequence + role_index + 1,
                    Some(&mut role_agent),
                )
            },
        )
    }

    fn role_finding(
        session: &ResearchSession,
        role: &str,
        sequence: usize,
        agent: Option<&mut AgentManager>,
    ) -> Result<ResearchAgentFinding> {
        let prompt = role_prompt(session, role);
        let prompt_hash = sha256_text(&prompt);
        let created_at = now_iso();
        let started = Instant::now();
        let mut route_attempts = Vec::new();
        let (status, model_ref, response, execution_mode, error) = if let Some(agent) = agent {
            match agent.primary_advisory(&prompt, Some(&format!("research_{role}"))) {
                Ok(text) => {
                    route_attempts = agent.last_route_attempts.clone();
                    (
                        "succeeded".to_string(),
                        agent
                            .last_route_attempts
                            .iter()
                            .rev()
                            .find(|attempt| attempt.status == "success")
                            .map(|attempt| attempt.model.clone()),
                        text,
                        "primary_route".to_string(),
                        None,
                    )
                }
                Err(error) => {
                    route_attempts = agent.last_route_attempts.clone();
                    let message = format!("{error:#}");
                    (
                        "failed".to_string(),
                        None,
                        format!("research role {role} primary advisory failed: {message}"),
                        "primary_route".to_string(),
                        Some(message),
                    )
                }
            }
        } else {
            (
                "synthesized".to_string(),
                None,
                default_role_summary(session, role),
                "local_synthesis".to_string(),
                None,
            )
        };
        Ok(ResearchAgentFinding {
            id: format!("finding_{role}_{sequence}"),
            role: role.to_string(),
            status,
            model_ref,
            execution_mode,
            duration_ms: started.elapsed().as_millis(),
            route_attempts,
            error,
            prompt_hash,
            response_hash: sha256_text(&response),
            summary: response,
            evidence_status: "advisory".into(),
            evidence_refs: Vec::new(),
            confidence: if role == "safety_monitor" { 0.9 } else { 0.7 },
            risk_level: if role == "critic" || role == "safety_monitor" {
                "medium".into()
            } else {
                "low".into()
            },
            created_at,
        })
    }

    fn run_experiment(
        &self,
        id: String,
        hypothesis_id: Option<String>,
        purpose: String,
        command: ExperimentCommand,
    ) -> Result<ResearchExperiment> {
        let started_at = now_iso();
        let started = Instant::now();
        let mut policy_violation = None;
        let mut changed_files = Vec::new();
        let validation = self.validate_command(&command);
        if let Err(error) = validation {
            let message = error.to_string();
            return Ok(ResearchExperiment {
                id,
                hypothesis_id,
                purpose,
                command,
                status: "blocked".into(),
                exit_code: None,
                stdout_tail: String::new(),
                stderr_tail: message.clone(),
                duration_ms: started.elapsed().as_millis(),
                policy_violation: Some(message),
                changed_files,
                started_at,
                finished_at: now_iso(),
            });
        }
        let before = if self.config.command_policy.diff_check_repo_tracked_files {
            Some(snapshot_workspace_files(&self.workspace)?)
        } else {
            None
        };
        let cwd = self.command_cwd(&command)?;
        let mut process = Command::new(&command.argv[0]);
        process.args(&command.argv[1..]).current_dir(cwd);
        let mut child = process
            .spawn()
            .with_context(|| format!("无法启动 scientist 实验命令: {}", command.argv.join(" ")))?;
        let timeout = Duration::from_secs(bounded_experiment_timeout(command.timeout_seconds));
        let timed_out = loop {
            if child.try_wait()?.is_some() {
                break false;
            }
            if started.elapsed() > timeout {
                let _ = child.kill();
                break true;
            }
            thread::sleep(Duration::from_millis(25));
        };
        let output = child
            .wait_with_output()
            .context("无法读取 scientist 实验输出")?;
        if let Some(before) = before {
            let after = snapshot_workspace_files(&self.workspace)?;
            changed_files = changed_snapshot_paths(&before, &after);
            if !changed_files.is_empty() {
                policy_violation = Some(format!(
                    "protected workspace files changed: {}",
                    changed_files.join(", ")
                ));
            }
        }
        let exit_code = if timed_out {
            None
        } else {
            output.status.code()
        };
        let status = if policy_violation.is_some() {
            "policy_violation"
        } else if timed_out {
            "failed"
        } else if output.status.success() {
            "succeeded"
        } else {
            "failed"
        };
        Ok(ResearchExperiment {
            id,
            hypothesis_id,
            purpose,
            command,
            status: status.into(),
            exit_code,
            stdout_tail: tail(&String::from_utf8_lossy(&output.stdout)),
            stderr_tail: if timed_out {
                "experiment command timed out".into()
            } else {
                tail(&String::from_utf8_lossy(&output.stderr))
            },
            duration_ms: started.elapsed().as_millis(),
            policy_violation,
            changed_files,
            started_at,
            finished_at: now_iso(),
        })
    }

    fn validate_command(&self, command: &ExperimentCommand) -> Result<()> {
        if command.argv.is_empty() || command.argv[0].trim().is_empty() {
            bail!("scientist experiment command argv 不能为空");
        }
        if !self.config.command_policy.require_workspace_cwd {
            bail!("scientist experiment workspace cwd boundary cannot be disabled");
        }
        if self.config.autonomy.can_edit_files {
            bail!("scientist cannot run experiments when can_edit_files=true");
        }
        if self.config.autonomy.can_close_goals {
            bail!("scientist cannot close goals");
        }
        if self.config.command_policy.reject_shell_operators {
            for arg in &command.argv {
                if arg
                    .chars()
                    .any(|ch| matches!(ch, '|' | '&' | ';' | '<' | '>' | '`'))
                {
                    bail!("scientist experiment argv contains shell operator: {arg}");
                }
            }
        }
        if !self
            .config
            .command_policy
            .allowed
            .iter()
            .any(|allowed| argv_starts_with(&command.argv, allowed))
        {
            bail!(
                "scientist experiment command is not whitelisted: {}",
                command.argv.join(" ")
            );
        }
        Ok(())
    }

    fn command_cwd(&self, command: &ExperimentCommand) -> Result<PathBuf> {
        let cwd = command
            .cwd
            .as_ref()
            .map(|cwd| self.workspace.join(cwd))
            .unwrap_or_else(|| self.workspace.clone());
        if !cwd.exists() {
            bail!("scientist 实验 cwd 不存在: {}", cwd.display());
        }
        ensure_within(&cwd, &self.workspace, "scientist 实验 cwd 必须位于工作区内")
    }

    fn reflect(&self, session: &ResearchSession, experiment_id: &str) -> ResearchReflection {
        let experiment = session
            .experiments
            .iter()
            .find(|experiment| experiment.id == experiment_id);
        let expected = session
            .hypotheses
            .first()
            .map(|hypothesis| hypothesis.expected_observation.clone())
            .unwrap_or_else(|| "experiment produces bounded evidence".into());
        let observed = experiment
            .map(|experiment| {
                format!(
                    "status={}, exit_code={:?}, stdout_tail_len={}, stderr_tail_len={}",
                    experiment.status,
                    experiment.exit_code,
                    experiment.stdout_tail.len(),
                    experiment.stderr_tail.len()
                )
            })
            .unwrap_or_else(|| "experiment missing".into());
        ResearchReflection {
            id: format!("reflection_{}", session.reflections.len() + 1),
            experiment_id: Some(experiment_id.to_string()),
            expected,
            observed: observed.clone(),
            mismatch: experiment
                .filter(|experiment| experiment.status != "succeeded")
                .map(|experiment| format!("experiment did not succeed: {}", experiment.status)),
            lesson: if observed.contains("status=succeeded") {
                "whitelist experiment produced bounded evidence; primary AI must still validate current files before completion".into()
            } else {
                "experiment result needs primary-AI triage before any success claim".into()
            },
            next_hypotheses: Vec::new(),
            quality_pattern_candidates: Vec::new(),
            created_at: now_iso(),
        }
    }

    fn required_actions(&self, session: &ResearchSession) -> Vec<String> {
        let mut actions = vec![
            "Treat research output as advisory evidence only.".into(),
            "Primary AI must validate current files and command output before delivery.".into(),
        ];
        if UNRESOLVED_STATUSES.contains(&session.status.as_str()) {
            actions.push("Resolve research conflicts before goal/session success.".into());
        }
        if session.status == "policy_violation" {
            actions
                .push("Investigate protected-file changes caused by scientist experiment.".into());
        }
        actions
    }

    fn add_conflict_pending(&self, session: &ResearchSession) -> Result<()> {
        SessionManager::new(self.workspace.clone())?.add_pending(
            &format!("resolve research session {}", session.id),
            "Research agent conflict or policy violation blocks successful delivery.",
            "workflow",
            "research",
            "must",
            json!({
                "research_session_id": session.id,
                "status": session.status,
                "conflicts": session.conflicts,
            }),
        )?;
        Ok(())
    }

    fn resolve_session(&self, id: Option<&str>) -> Result<ResearchSession> {
        if let Some(id) = id {
            return self.read_session(id);
        }
        self.next_active_session()?
            .context("没有 active/conflict research session；请先运行 rayman research start")
    }

    fn next_active_session(&self) -> Result<Option<ResearchSession>> {
        let mut sessions = self
            .list_sessions()?
            .into_iter()
            .filter(|session| !TERMINAL_STATUSES.contains(&session.status.as_str()))
            .collect::<Vec<_>>();
        sessions.sort_by_key(|session| session.created_at.clone());
        Ok(sessions.into_iter().next())
    }

    fn list_sessions(&self) -> Result<Vec<ResearchSession>> {
        if !self.sessions_dir.exists() {
            return Ok(Vec::new());
        }
        WalkDir::new(&self.sessions_dir)
            .min_depth(1)
            .max_depth(1)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            })
            .map(|entry| read_session_file(entry.path()))
            .collect()
    }

    fn read_session(&self, id: &str) -> Result<ResearchSession> {
        read_session_file(&self.session_path(id)?)
    }

    fn write_session(&self, session: &ResearchSession) -> Result<()> {
        let path = self.session_path(&session.id)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建 research 状态目录: {}", parent.display()))?;
        }
        fs::write(&path, serde_json::to_string_pretty(session)?)
            .with_context(|| format!("无法写入 research session: {}", path.display()))
    }

    fn session_path(&self, id: &str) -> Result<PathBuf> {
        if id.trim().is_empty() || id.contains(['/', '\\', ':']) {
            bail!("无效 research session id: {id}");
        }
        ensure_within(
            &self.sessions_dir.join(format!("{id}.json")),
            &self.workspace,
            "Research session 文件必须位于工作区内",
        )
    }
}

impl ResearchConfig {
    pub fn load(workspace: &Path) -> Result<Self> {
        let path = workspace.join("config").join("research_agents.yaml");
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("无法读取 research agent 配置: {}", path.display()))?;
        let root: Value = crate::yaml::from_str(&text)
            .with_context(|| format!("无法解析 research agent 配置: {}", path.display()))?;
        let Some(config) = root.get("research_agents") else {
            return Ok(Self::default());
        };
        let mut out = Self::default();
        out.enabled = config
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(out.enabled);
        out.max_parallel_agents = config
            .get("max_parallel_agents")
            .and_then(Value::as_u64)
            .unwrap_or(out.max_parallel_agents as u64) as usize;
        if let Some(scientist) = config.get("scientist") {
            out.autonomy.can_run_experiments = scientist
                .get("can_run_experiments")
                .and_then(Value::as_bool)
                .unwrap_or(out.autonomy.can_run_experiments);
            out.autonomy.can_edit_files = scientist
                .get("can_edit_files")
                .and_then(Value::as_bool)
                .unwrap_or(out.autonomy.can_edit_files);
            out.autonomy.can_close_goals = scientist
                .get("can_close_goals")
                .and_then(Value::as_bool)
                .unwrap_or(out.autonomy.can_close_goals);
        }
        if let Some(policy) = config.get("command_policy") {
            out.command_policy.allow_network = policy
                .get("allow_network")
                .and_then(Value::as_bool)
                .unwrap_or(out.command_policy.allow_network);
            out.command_policy.require_workspace_cwd = policy
                .get("require_workspace_cwd")
                .and_then(Value::as_bool)
                .unwrap_or(out.command_policy.require_workspace_cwd);
            out.command_policy.reject_shell_operators = policy
                .get("reject_shell_operators")
                .and_then(Value::as_bool)
                .unwrap_or(out.command_policy.reject_shell_operators);
            out.command_policy.diff_check_repo_tracked_files = policy
                .get("diff_check_repo_tracked_files")
                .and_then(Value::as_bool)
                .unwrap_or(out.command_policy.diff_check_repo_tracked_files);
            if let Some(allowed) = policy.get("allowed").and_then(Value::as_array) {
                out.command_policy.allowed = allowed
                    .iter()
                    .filter_map(|entry| {
                        entry.as_array().map(|items| {
                            items
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect::<Vec<_>>()
                        })
                    })
                    .filter(|argv| !argv.is_empty())
                    .collect();
            }
        }
        Ok(out)
    }
}

fn read_session_file(path: &Path) -> Result<ResearchSession> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("无法读取 research session: {}", path.display()))?;
    serde_json::from_str(&text)
        .with_context(|| format!("无法解析 research session: {}", path.display()))
}

fn role_prompt(session: &ResearchSession, role: &str) -> String {
    format!(
        "You are the {role} in RaymanCodingSkill research orchestration.\nQuestion: {}\nCurrent status: {}\nReturn strict JSON when possible with hypotheses, experiments, risks, evidence_status, evidence_refs, confidence, unknowns, blockers, and next_action. Use evidence_status=verified only for current workspace file, command output, or evidence artifact references; empty evidence_refs must be advisory or unknown even when confidence is high. Confidence is metadata, not proof. Scientist may request only whitelist commands and cannot edit files, approve validation, or close goals.",
        session.question, session.status
    )
}

fn default_research_evidence_status() -> String {
    "advisory".into()
}

fn default_role_summary(session: &ResearchSession, role: &str) -> String {
    match role {
        "scientist" => json!({
            "hypotheses": [{
                "claim": format!("The research question can be narrowed with local workspace evidence: {}", session.question),
                "expected_observation": "context command exits successfully and reports workspace state"
            }],
            "experiments": [{
                "argv": ["rayman", "context", "status"],
                "purpose": "establish current workspace context without editing files"
            }]
        })
        .to_string(),
        "critic" => "Check failed experiments, missing validation, obsolete assets, and unresolved conflicts before success.".into(),
        "reflector" => "Compare expected observations with command output; record mismatches as advisory lessons only.".into(),
        "arbiter" => "If agents disagree or experiments fail, create a conflict and require primary-AI triage.".into(),
        "safety_monitor" => "Scientist can run only argv whitelist commands, cannot edit files, and cannot close goals.".into(),
        _ => "Plan a bounded research round using current files and auditable local evidence.".into(),
    }
}

fn default_hypothesis(question: &str, scientist_response: Option<&str>) -> ResearchHypothesis {
    let parsed = scientist_response.and_then(extract_json);
    let claim = parsed
        .as_ref()
        .and_then(|value| value.get("hypotheses"))
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("claim"))
        .and_then(Value::as_str)
        .unwrap_or("The research question can be narrowed with local workspace evidence")
        .to_string();
    let expected = parsed
        .as_ref()
        .and_then(|value| value.get("hypotheses"))
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("expected_observation"))
        .and_then(Value::as_str)
        .unwrap_or("whitelist command exits successfully and provides bounded evidence")
        .to_string();
    ResearchHypothesis {
        id: "hyp_1".into(),
        claim,
        rationale: format!("Initial scientist hypothesis for: {question}"),
        expected_observation: expected,
        falsification_tests: vec![
            "experiment exits non-zero".into(),
            "policy violation is detected".into(),
        ],
        prior_confidence: 0.6,
        posterior_confidence: None,
        status: "proposed".into(),
    }
}

fn parse_scientist_command(text: &str) -> Option<ExperimentCommand> {
    let value = extract_json(text)?;
    let experiment = value
        .get("experiments")
        .and_then(Value::as_array)
        .and_then(|items| items.first())?;
    let argv = experiment
        .get("argv")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    if argv.is_empty() {
        return None;
    }
    Some(ExperimentCommand {
        argv,
        cwd: experiment
            .get("cwd")
            .and_then(Value::as_str)
            .map(str::to_string),
        timeout_seconds: bounded_experiment_timeout(
            experiment
                .get("timeout_seconds")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_EXPERIMENT_TIMEOUT_SECONDS),
        ),
    })
}

fn default_experiment_command(question: &str) -> ExperimentCommand {
    ExperimentCommand {
        argv: vec![
            "rayman".into(),
            "context".into(),
            "task".into(),
            question.to_string(),
        ],
        cwd: None,
        timeout_seconds: DEFAULT_EXPERIMENT_TIMEOUT_SECONDS,
    }
}

fn extract_json(text: &str) -> Option<Value> {
    if let Ok(value) = serde_json::from_str(text) {
        return Some(value);
    }
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(&text[start..=end]).ok()
}

fn snapshot_workspace_files(workspace: &Path) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for entry in WalkDir::new(workspace)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        if should_ignore_snapshot_path(entry.path(), workspace) {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(workspace)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .replace('\\', "/");
        out.insert(relative, sha256_file(entry.path())?);
    }
    Ok(out)
}

fn should_ignore_snapshot_path(path: &Path, workspace: &Path) -> bool {
    let relative = path.strip_prefix(workspace).unwrap_or(path);
    let mut components = relative
        .components()
        .filter_map(|component| component.as_os_str().to_str());
    if let Some(first) = components.next()
        && matches!(
            first,
            ".git"
                | ".RaymanCodingSkill"
                | "target"
                | ".tmp"
                | "logs"
                | "node_modules"
                | "dist"
                | "build"
        )
    {
        return true;
    }
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    if is_protected_snapshot_extension(ext) {
        return false;
    }
    if matches!(
        ext.to_ascii_lowercase().as_str(),
        "bk" | "bak" | "backup" | "orig"
    ) && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
        && let Some(source_ext) = Path::new(stem).extension().and_then(|ext| ext.to_str())
    {
        return !is_protected_snapshot_extension(source_ext);
    }
    true
}

fn is_protected_snapshot_extension(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "rs" | "toml" | "yaml" | "yml" | "md" | "json" | "lock" | "html"
    )
}

fn changed_snapshot_paths(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> Vec<String> {
    let keys = before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    keys.into_iter()
        .filter(|path| before.get(path) != after.get(path))
        .collect()
}

fn argv_starts_with(argv: &[String], allowed: &[String]) -> bool {
    argv.len() >= allowed.len() && argv.iter().zip(allowed).all(|(left, right)| left == right)
}

fn words(command: &str) -> Vec<String> {
    command.split_whitespace().map(str::to_string).collect()
}

fn bounded_experiment_timeout(seconds: u64) -> u64 {
    seconds.clamp(1, MAX_EXPERIMENT_TIMEOUT_SECONDS)
}

fn research_id(question: &str, created_at: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(question.as_bytes());
    digest.update(created_at.as_bytes());
    let hash = format!("{:x}", digest.finalize());
    format!("research_{}", &hash[..12])
}

fn sha256_text(text: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(text.as_bytes());
    format!("{:x}", digest.finalize())
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("无法读取文件: {}", path.display()))?;
    let mut digest = Sha256::new();
    digest.update(bytes);
    Ok(format!("{:x}", digest.finalize()))
}

fn tail(text: &str) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let start = chars.len().saturating_sub(MAX_TAIL_CHARS);
    chars[start..].iter().collect()
}

fn research_parallel_cap(configured: usize, role_count: usize) -> usize {
    configured.max(1).min(role_count.max(1))
}

fn run_research_role_jobs<T, F>(
    roles: &[&'static str],
    configured_cap: usize,
    run: F,
) -> Result<Vec<T>>
where
    T: Send,
    F: Fn(usize, &'static str) -> Result<T> + Sync,
{
    let cap = research_parallel_cap(configured_cap, roles.len());
    if cap == 1 {
        return roles
            .iter()
            .enumerate()
            .map(|(role_index, role)| run(role_index, role))
            .collect();
    }

    let mut ordered = Vec::new();
    for chunk_start in (0..roles.len()).step_by(cap) {
        let chunk_end = (chunk_start + cap).min(roles.len());
        let chunk_results = thread::scope(|scope| {
            let mut handles = Vec::new();
            for (role_index, role) in roles
                .iter()
                .copied()
                .enumerate()
                .take(chunk_end)
                .skip(chunk_start)
            {
                let run = &run;
                handles.push((role_index, scope.spawn(move || run(role_index, role))));
            }
            let mut out = Vec::new();
            for (role_index, handle) in handles {
                let result = handle
                    .join()
                    .map_err(|_| anyhow!("research role worker panicked"))??;
                out.push((role_index, result));
            }
            Ok::<_, anyhow::Error>(out)
        })?;
        ordered.extend(chunk_results);
    }
    ordered.sort_by_key(|(role_index, _)| *role_index);
    Ok(ordered.into_iter().map(|(_, result)| result).collect())
}

fn default_research_execution_mode() -> String {
    "legacy".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn record_peak(peak: &AtomicUsize, value: usize) {
        let mut observed = peak.load(Ordering::SeqCst);
        while value > observed {
            match peak.compare_exchange(observed, value, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => break,
                Err(next) => observed = next,
            }
        }
    }

    #[test]
    fn default_policy_rejects_non_whitelisted_command() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ResearchManager::new(temp.path()).unwrap();
        let command = ExperimentCommand {
            argv: vec!["git".into(), "reset".into(), "--hard".into()],
            cwd: None,
            timeout_seconds: 1,
        };

        let error = manager.validate_command(&command).unwrap_err().to_string();

        assert!(error.contains("not whitelisted"));
    }

    #[test]
    fn default_policy_rejects_full_workspace_test_as_scientist_experiment() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ResearchManager::new(temp.path()).unwrap();
        let command = ExperimentCommand {
            argv: vec!["cargo".into(), "test".into(), "--all".into()],
            cwd: None,
            timeout_seconds: 1,
        };

        let error = manager.validate_command(&command).unwrap_err().to_string();

        assert!(error.contains("not whitelisted"));
    }

    #[test]
    fn policy_rejects_shell_operators_inside_argv() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ResearchManager::new(temp.path()).unwrap();
        let command = ExperimentCommand {
            argv: vec!["rayman".into(), "context".into(), "status|more".into()],
            cwd: None,
            timeout_seconds: 1,
        };

        let error = manager.validate_command(&command).unwrap_err().to_string();

        assert!(error.contains("shell operator"));
    }

    #[test]
    fn policy_rejects_disabled_workspace_cwd_boundary() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        fs::write(
            temp.path().join("config").join("research_agents.yaml"),
            r#"
research_agents:
  enabled: true
  scientist:
    can_run_experiments: true
    can_edit_files: false
    can_close_goals: false
  command_policy:
    require_workspace_cwd: false
    reject_shell_operators: true
    diff_check_repo_tracked_files: true
    allowed:
      - ["cargo", "test", "--all"]
"#,
        )
        .unwrap();
        let manager = ResearchManager::new(temp.path()).unwrap();
        let command = ExperimentCommand {
            argv: vec!["cargo".into(), "test".into(), "--all".into()],
            cwd: None,
            timeout_seconds: 1,
        };

        let error = manager.validate_command(&command).unwrap_err().to_string();

        assert!(error.contains("workspace cwd boundary"));
    }

    #[test]
    fn command_cwd_rejects_outside_workspace_even_if_policy_is_misconfigured() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir_all(workspace.path().join("config")).unwrap();
        fs::write(
            workspace.path().join("config").join("research_agents.yaml"),
            r#"
research_agents:
  enabled: true
  scientist:
    can_run_experiments: true
    can_edit_files: false
    can_close_goals: false
  command_policy:
    require_workspace_cwd: false
    reject_shell_operators: true
    diff_check_repo_tracked_files: true
    allowed:
      - ["cargo", "test", "--all"]
"#,
        )
        .unwrap();
        let manager = ResearchManager::new(workspace.path()).unwrap();
        let command = ExperimentCommand {
            argv: vec!["cargo".into(), "test".into(), "--all".into()],
            cwd: Some(outside.path().to_string_lossy().into_owned()),
            timeout_seconds: 1,
        };

        let error = manager.command_cwd(&command).unwrap_err().to_string();

        assert!(error.contains("工作区内"));
    }

    #[test]
    fn snapshot_protects_source_backup_files() {
        let workspace = Path::new("/workspace");

        assert!(!should_ignore_snapshot_path(
            &workspace.join("src").join("lib.rs.bk"),
            workspace
        ));
        assert!(!should_ignore_snapshot_path(
            &workspace.join("config").join("default_config.yaml.bak"),
            workspace
        ));
        assert!(should_ignore_snapshot_path(
            &workspace.join("assets").join("logo.png.bk"),
            workspace
        ));
    }

    #[test]
    fn scientist_command_parser_accepts_structured_experiment() {
        let command = parse_scientist_command(
            r#"{"experiments":[{"argv":["rayman","context","task","research"],"timeout_seconds":30}]}"#,
        )
        .unwrap();

        assert_eq!(command.argv, vec!["rayman", "context", "task", "research"]);
        assert_eq!(command.timeout_seconds, 30);
    }

    #[test]
    fn scientist_command_parser_clamps_timeout_to_hard_limit() {
        let command = parse_scientist_command(
            r#"{"experiments":[{"argv":["rayman","context","task","research"],"timeout_seconds":9999}]}"#,
        )
        .unwrap();

        assert_eq!(command.timeout_seconds, MAX_EXPERIMENT_TIMEOUT_SECONDS);
    }

    #[test]
    fn research_roles_are_serial_when_max_parallel_agents_is_one() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let roles = run_research_role_jobs(RESEARCH_ROLES, 1, {
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            move |_index, role| {
                let active_now = active.fetch_add(1, Ordering::SeqCst) + 1;
                record_peak(&peak, active_now);
                std::thread::sleep(Duration::from_millis(2));
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(role.to_string())
            }
        })
        .unwrap();

        assert_eq!(peak.load(Ordering::SeqCst), 1);
        assert_eq!(roles, RESEARCH_ROLES);
    }

    #[test]
    fn research_roles_respect_parallel_cap_and_preserve_role_order() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let roles = run_research_role_jobs(RESEARCH_ROLES, 2, {
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            move |index, role| {
                let active_now = active.fetch_add(1, Ordering::SeqCst) + 1;
                record_peak(&peak, active_now);
                let delay_ms = (RESEARCH_ROLES.len() - index) as u64;
                std::thread::sleep(Duration::from_millis(delay_ms));
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(role.to_string())
            }
        })
        .unwrap();

        assert!(peak.load(Ordering::SeqCst) <= 2);
        assert_eq!(roles, RESEARCH_ROLES);
    }

    #[test]
    fn research_roles_use_same_explicit_primary_route_for_all_findings() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        let request_count = Arc::new(AtomicUsize::new(0));
        let base_url = openai_multi_test_server(
            r#"{"experiments":[{"argv":["cargo","--version"]}]}"#,
            RESEARCH_ROLES.len(),
            Arc::clone(&request_count),
        );
        fs::write(
            temp.path().join("config").join("default_config.yaml"),
            format!(
                r#"
config_files: {{}}
default_model:
  type: openai
  name: default-model
models:
  openai:
    adapter: openai_compatible
    auth_required: false
    base_url: "{base_url}"
    timeout: 5
"#
            ),
        )
        .unwrap();
        fs::write(
            temp.path().join("config").join("research_agents.yaml"),
            r#"
research_agents:
  enabled: true
  max_parallel_agents: 3
  scientist:
    can_run_experiments: true
    can_edit_files: false
    can_close_goals: false
  command_policy:
    allowed:
      - ["cargo", "--version"]
"#,
        )
        .unwrap();
        let manager = ResearchManager::new(temp.path()).unwrap();
        let session = manager.start("verify route propagation", None).unwrap();
        let mut agent = AgentManager::new(
            temp.path(),
            Some("openai".into()),
            Some("research-model".into()),
            Some("auto".into()),
            true,
        )
        .unwrap();

        let session = manager
            .run_once(Some(&session.id), Some(&mut agent))
            .unwrap();

        assert_eq!(request_count.load(Ordering::SeqCst), RESEARCH_ROLES.len());
        assert_eq!(
            session
                .findings
                .iter()
                .map(|finding| finding.role.as_str())
                .collect::<Vec<_>>(),
            RESEARCH_ROLES
        );
        for finding in &session.findings {
            assert_eq!(finding.status, "succeeded");
            assert_eq!(finding.execution_mode, "primary_route");
            assert_eq!(finding.model_ref.as_deref(), Some("openai/research-model"));
            assert_eq!(finding.route_attempts.len(), 1);
            assert_eq!(finding.route_attempts[0].model, "openai/research-model");
            assert_eq!(finding.route_attempts[0].status, "success");
        }
    }

    #[test]
    fn research_session_deserializes_legacy_agent_finding_json() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ResearchManager::new(temp.path()).unwrap();
        let id = "research_legacy";
        let path = manager.session_path(id).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            r#"
{
  "id": "research_legacy",
  "goal_id": null,
  "question": "legacy session",
  "status": "active",
  "current_stage": "question",
  "autonomy_policy": {
    "can_run_experiments": true,
    "can_edit_files": false,
    "can_close_goals": false
  },
  "created_at": "2026-06-09T00:00:00Z",
  "updated_at": "2026-06-09T00:00:00Z",
  "hypotheses": [],
  "experiments": [],
  "reflections": [],
  "findings": [
    {
      "id": "finding_scientist_1",
      "role": "scientist",
      "status": "succeeded",
      "model_ref": null,
      "prompt_hash": "p",
      "response_hash": "r",
      "summary": "legacy summary",
      "evidence_refs": [],
      "confidence": 0.8,
      "risk_level": "low",
      "created_at": "2026-06-09T00:00:00Z"
    }
  ],
  "conflicts": []
}
"#,
        )
        .unwrap();

        let session = manager.read_session(id).unwrap();

        assert_eq!(session.findings[0].execution_mode, "legacy");
        assert_eq!(session.findings[0].duration_ms, 0);
        assert!(session.findings[0].route_attempts.is_empty());
        assert!(session.findings[0].error.is_none());
        assert_eq!(session.findings[0].evidence_status, "advisory");
        assert!(session.findings[0].evidence_refs.is_empty());
        assert_eq!(session.findings[0].confidence, 0.8);
    }

    #[test]
    fn research_session_records_reconciled_experiment_without_model() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        fs::write(
            temp.path().join("config").join("research_agents.yaml"),
            r#"
research_agents:
  enabled: true
  scientist:
    can_run_experiments: true
    can_edit_files: false
    can_close_goals: false
  command_policy:
    allowed:
      - ["cargo", "--version"]
"#,
        )
        .unwrap();
        let manager = ResearchManager::new(temp.path()).unwrap();
        let mut session = manager.start("verify cargo availability", None).unwrap();
        session.findings.push(ResearchAgentFinding {
            id: "finding_scientist_1".into(),
            role: "scientist".into(),
            status: "succeeded".into(),
            model_ref: None,
            execution_mode: "local_synthesis".into(),
            duration_ms: 0,
            route_attempts: Vec::new(),
            error: None,
            prompt_hash: "p".into(),
            response_hash: "r".into(),
            summary: r#"{"experiments":[{"argv":["cargo","--version"]}]}"#.into(),
            evidence_status: "advisory".into(),
            evidence_refs: Vec::new(),
            confidence: 0.8,
            risk_level: "low".into(),
            created_at: now_iso(),
        });
        manager.write_session(&session).unwrap();

        let session = manager.run_once(Some(&session.id), None).unwrap();

        assert_eq!(session.status, "reconciled");
        assert_eq!(session.experiments.len(), 1);
        assert_eq!(session.experiments[0].status, "succeeded");
    }

    fn openai_multi_test_server(
        content: &'static str,
        expected_requests: usize,
        request_count: Arc<AtomicUsize>,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for stream in listener.incoming().take(expected_requests) {
                let Ok(mut stream) = stream else {
                    continue;
                };
                stream
                    .set_read_timeout(Some(Duration::from_millis(500)))
                    .unwrap();
                let mut buffer = Vec::new();
                let mut chunk = [0; 1024];
                while let Ok(n) = stream.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..n]);
                    let request = String::from_utf8_lossy(&buffer);
                    if let Some(header_end) = request.find("\r\n\r\n") {
                        let content_length = request[..header_end]
                            .lines()
                            .find_map(|line| {
                                line.split_once(':').and_then(|(name, value)| {
                                    name.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse::<usize>().ok())
                                        .flatten()
                                })
                            })
                            .unwrap_or(0);
                        if buffer.len() >= header_end + 4 + content_length {
                            break;
                        }
                    }
                }
                request_count.fetch_add(1, Ordering::SeqCst);
                let body = format!(r#"{{"choices":[{{"message":{{"content":{content:?}}}}}]}}"#);
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        format!("http://{addr}/v1")
    }
}
