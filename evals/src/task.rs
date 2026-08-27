//! 任务与 fixture 加载：每个任务 = 一段给 agent 的提示 + 起始工作区 + 一条隐藏的评分命令。
//!
//! Fixture 是评测输入的一部分，不能把其中的 symlink 当作普通文件递归跟随；否则一个
//! 看似局部的任务可以在复制时把宿主机任意目录带进 trial。这里一律拒绝 symlink。

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::grade;

const BUILTIN_TASK_MANIFEST: &str = "rayman-evals-builtins-v1";
const TASK_CONTRACT_SCHEMA: &str = "rayman.eval-task.v2";
const ORACLE_MODULE_PATH: &str = "src/__rayman_oracle.rs";
const ORACLE_MODULE_MARKER: &str =
    "\n#[cfg(test)]\n#[path = \"__rayman_oracle.rs\"]\nmod __rayman_oracle;\n";
const BUILTIN_TASK_HASHES: &[(&str, &str)] = &[
    (
        "add-feature",
        "e308acf3782167365d627e189ed4c311b2b4f678676eb601c8a3a8611fc088a9",
    ),
    (
        "adjacent-bug-escalation",
        "b491c9acac075991426ea6de0be1ab30cbc169b51ddad9d6eeb3ddf259be9ceb",
    ),
    (
        "audit-to-closure",
        "e937b5a498a9b13b73a7ecbe7878f1730395525387b256d4fd7675a15b70ce1b",
    ),
    (
        "edge-case-split",
        "ebf9f6851e697e85c16afe7448934fc4aa83e130350fac58222d4dc5ce457cac",
    ),
    (
        "emergent-risk-plan-extension",
        "d66eeb6fa30b61624eb2707946ee9e31331b0e8619e792c6969d4c3e96daed85",
    ),
    (
        "evidence-first-overflow",
        "96dc3cd9cf5b3f2d49f45e70e24474ff80bb995b0a2b198039091d6bcffe5a20",
    ),
    (
        "fix-failing-test",
        "49c5d27a6452c57b58edd979aaa9bc535bf82c4e33011707794a43fa61e79f63",
    ),
    (
        "human-boundary-solution-pack",
        "3821904cf0b20a190a637733cf14f5b6c701499fad02ae809f8a1be578e796fc",
    ),
    (
        "large-repo-nav",
        "be44614a117ead8607611153b2797b205c5c6f2dd4d4f9c6289fa4e6ff9c853c",
    ),
    (
        "remove-dead-code",
        "93f02fdf4f54c730d0b75c1509d474b73bbcf92495928d99f9e5c78320121f38",
    ),
    (
        "self-invalidating-gate",
        "4a2098997be196ee796d8feed1fc1556aa5003e73fd7690b9ddf08abfea24c8e",
    ),
];

#[derive(Debug, Clone)]
pub struct Task {
    pub name: String,
    pub prompt: String,
    prompt_path: PathBuf,
    pub fixture_dir: PathBuf,
    /// 隐藏评分命令：在 agent 完成后的工作区里运行，退出 0 记为成功。agent 看不到它。
    pub grade_cmd: String,
    grade_path: PathBuf,
    contract_path: PathBuf,
    oracle_dir: PathBuf,
    pub contract: TaskContract,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskContract {
    pub schema: String,
    pub source_ids: Vec<String>,
    pub rule_ids: Vec<String>,
    #[serde(default)]
    pub activate_workspace: bool,
    #[serde(default)]
    pub editable_paths: Vec<String>,
    #[serde(default)]
    pub editable_prefixes: Vec<String>,
    pub oracle: OracleContract,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OracleContract {
    RustModule {
        source: String,
        module_file: String,
        expected_tests: Vec<String>,
    },
    PowerShell {
        source: String,
        destination: String,
        success_marker: String,
    },
}

/// 写入 run manifest 的任务输入身份。`task_sha256` 绑定任务名、提示、评分命令和 fixture tree。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskProvenance {
    pub name: String,
    pub prompt_sha256: String,
    pub grade_sha256: String,
    pub fixture_sha256: String,
    pub contract_sha256: String,
    pub oracle_sha256: String,
    pub task_sha256: String,
}

#[derive(Debug, Clone)]
pub struct OracleSeal {
    sealed_files: BTreeMap<String, String>,
    expected_tests: Vec<String>,
    success_marker: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OraclePreparationFailure {
    PolicyViolation,
    Infrastructure,
}

#[derive(Debug)]
pub struct OraclePreparationError {
    kind: OraclePreparationFailure,
    message: String,
}

impl OraclePreparationError {
    fn policy(message: impl Into<String>) -> Self {
        Self {
            kind: OraclePreparationFailure::PolicyViolation,
            message: message.into(),
        }
    }

    fn infrastructure(message: impl Into<String>) -> Self {
        Self {
            kind: OraclePreparationFailure::Infrastructure,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> OraclePreparationFailure {
        self.kind
    }
}

impl std::fmt::Display for OraclePreparationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for OraclePreparationError {}

/// Why this run is allowed to execute hidden grade commands on the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GradeExecutionProvenance {
    pub mode: String,
    pub tasks_root: String,
    pub trusted_manifest: Option<String>,
    pub custom_execution_acknowledged: bool,
}

impl Task {
    pub fn provenance(&self) -> Result<TaskProvenance> {
        // Re-read all task source inputs for every integrity check. The task keeps the
        // startup strings used by the agent/grade, but a later host-shell mutation of
        // prompt.md or grade.txt must still invalidate the run rather than leave the
        // original hashes looking current.
        let prompt = read(&self.prompt_path)?.trim().to_string();
        let grade = read(&self.grade_path)?.trim().to_string();
        // Authorization must bind the exact strings that the evaluator will actually
        // use.  Hashing only the second disk read would leave a load->authorize race:
        // an attacker could make `Task` cache one grade command, restore the trusted
        // file before this read, and obtain approval for bytes that will not execute.
        if prompt != self.prompt {
            bail!(
                "task prompt changed after load; refusing stale cached input: {}",
                self.prompt_path.display()
            );
        }
        if grade != self.grade_cmd {
            bail!(
                "task grade changed after load; refusing stale cached command: {}",
                self.grade_path.display()
            );
        }
        let prompt_sha256 = sha256_bytes(self.prompt.as_bytes());
        let grade_sha256 = sha256_bytes(self.grade_cmd.as_bytes());
        let fixture_sha256 = hash_tree(&self.fixture_dir)?;
        let contract_text = read(&self.contract_path)?;
        let contract_sha256 = sha256_bytes(contract_text.as_bytes());
        let oracle_sha256 = hash_tree(&self.oracle_dir)?;
        let task_sha256 = sha256_parts(&[
            self.name.as_bytes(),
            prompt_sha256.as_bytes(),
            grade_sha256.as_bytes(),
            fixture_sha256.as_bytes(),
            contract_sha256.as_bytes(),
            oracle_sha256.as_bytes(),
        ]);
        Ok(TaskProvenance {
            name: self.name.clone(),
            prompt_sha256,
            grade_sha256,
            fixture_sha256,
            contract_sha256,
            oracle_sha256,
            task_sha256,
        })
    }
}

/// 从 `tasks_root` 加载所有任务；`filter` 非空时只保留名字匹配的那个。
pub fn load_tasks(tasks_root: &Path, filter: Option<&str>) -> Result<Vec<Task>> {
    ensure_real_dir(tasks_root, "任务根目录")?;
    let mut tasks = Vec::new();
    let entries = fs::read_dir(tasks_root)
        .with_context(|| format!("无法读取任务目录: {}", tasks_root.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("无法枚举任务目录: {}", tasks_root.display()))?;
        let dir = entry.path();
        let metadata = fs::symlink_metadata(&dir)
            .with_context(|| format!("无法检查任务目录: {}", dir.display()))?;
        if is_link_or_reparse(&metadata) {
            bail!("拒绝任务目录 symlink: {}", dir.display());
        }
        if !metadata.is_dir() {
            continue;
        }
        let name = dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        if let Some(filter) = filter
            && filter != name
        {
            continue;
        }
        // 杂散目录（IDE 缓存、临时文件夹等）没有 prompt.md：跳过并提示，别让整轮评测挂掉。
        let prompt_path = dir.join("prompt.md");
        if !prompt_path.is_file() {
            eprintln!("⚠ 跳过缺 prompt.md 的目录（非任务）: {}", dir.display());
            continue;
        }
        let prompt = read(&prompt_path)?;
        let grade_path = dir.join("grade.txt");
        let grade_cmd = read(&grade_path)?.trim().to_string();
        let contract_path = dir.join("task.json");
        let contract_text = read(&contract_path)?;
        let contract: TaskContract = serde_json::from_str(&contract_text)
            .with_context(|| format!("无法解析任务 contract: {}", contract_path.display()))?;
        validate_task_contract(&name, &contract)?;
        let fixture_dir = dir.join("fixture");
        ensure_real_dir(&fixture_dir, "任务 fixture")?;
        let oracle_dir = dir.join("oracle");
        ensure_real_dir(&oracle_dir, "任务 oracle")?;
        // On Windows `cmd` resolves a bare command from its CWD before PATH. A top-level
        // rayman wrapper would leak availability into control before a trial begins.
        if let Err(error) = grade::ensure_no_top_level_rayman_command(&fixture_dir) {
            bail!("任务 {name} 的 fixture 顶层包含不允许的 rayman 命令/包装器: {error}");
        }
        if grade_cmd.is_empty() {
            bail!("任务 {name} 的 grade.txt 为空");
        }
        // 在创建任何 trial 之前就遍历一次，确保 provenance 和复制看到的是同一类安全输入。
        let _ = hash_tree(&fixture_dir)?;
        tasks.push(Task {
            name,
            prompt: prompt.trim().to_string(),
            prompt_path,
            fixture_dir,
            grade_cmd,
            grade_path,
            contract_path,
            oracle_dir,
            contract,
        });
    }
    tasks.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(tasks)
}

fn validate_task_contract(name: &str, contract: &TaskContract) -> Result<()> {
    if contract.schema != TASK_CONTRACT_SCHEMA {
        bail!(
            "任务 {name} contract schema 必须是 {TASK_CONTRACT_SCHEMA}: {}",
            contract.schema
        );
    }
    for (label, values) in [
        ("source_ids", &contract.source_ids),
        ("rule_ids", &contract.rule_ids),
    ] {
        if values.is_empty()
            || values
                .iter()
                .any(|value| value.trim().is_empty() || value.trim() != value)
        {
            bail!("任务 {name} contract {label} 必须包含非空规范 ID");
        }
        let unique = values.iter().collect::<std::collections::BTreeSet<_>>();
        if unique.len() != values.len() {
            bail!("任务 {name} contract {label} 包含重复 ID");
        }
    }
    for path in contract
        .editable_paths
        .iter()
        .chain(contract.editable_prefixes.iter())
    {
        validate_relative_contract_path(name, path)?;
    }
    match &contract.oracle {
        OracleContract::RustModule {
            source,
            module_file,
            expected_tests,
        } => {
            validate_relative_contract_path(name, source)?;
            validate_relative_contract_path(name, module_file)?;
            if !contract.editable_paths.contains(module_file)
                && !contract
                    .editable_prefixes
                    .iter()
                    .any(|prefix| module_file.starts_with(prefix))
            {
                bail!("任务 {name} oracle module_file 必须属于 editable scope");
            }
            if expected_tests.is_empty()
                || expected_tests
                    .iter()
                    .any(|test| test.trim().is_empty() || test.trim() != test)
            {
                bail!("任务 {name} Rust oracle 必须声明 expected_tests");
            }
        }
        OracleContract::PowerShell {
            source,
            destination,
            success_marker,
        } => {
            validate_relative_contract_path(name, source)?;
            validate_relative_contract_path(name, destination)?;
            if success_marker.trim().is_empty() {
                bail!("任务 {name} PowerShell oracle 必须声明 success_marker");
            }
        }
    }
    Ok(())
}

fn validate_relative_contract_path(name: &str, path: &str) -> Result<()> {
    let normalized = Path::new(path);
    if path.is_empty()
        || path.contains('\\')
        || path.starts_with('/')
        || path.ends_with('/')
        || normalized
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        bail!("任务 {name} contract path 必须是规范相对路径: {path}");
    }
    Ok(())
}

/// Built-in grades execute without an acknowledgement only when both their repository
/// location and complete task-input hashes match the compiled manifest. A copied or edited
/// task tree is third-party input, even if it reuses a built-in task name/grade string.
pub fn authorize_grade_execution(
    tasks_root: &Path,
    filter: Option<&str>,
    manifests: &[TaskProvenance],
    custom_execution_acknowledged: bool,
) -> Result<GradeExecutionProvenance> {
    let builtin_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tasks");
    authorize_grade_execution_with_builtin_root(
        tasks_root,
        filter,
        manifests,
        custom_execution_acknowledged,
        &builtin_root,
    )
}

fn authorize_grade_execution_with_builtin_root(
    tasks_root: &Path,
    filter: Option<&str>,
    manifests: &[TaskProvenance],
    custom_execution_acknowledged: bool,
    builtin_root: &Path,
) -> Result<GradeExecutionProvenance> {
    let canonical_root = fs::canonicalize(tasks_root)
        .with_context(|| format!("无法解析任务根目录: {}", tasks_root.display()))?;
    // An explicit custom-task acknowledgement authorizes the exact loaded task manifest;
    // it must not depend on the build machine's repository `tasks/` directory still being
    // present at runtime (for example after installing only the evaluator binary).
    if custom_execution_acknowledged {
        return Ok(GradeExecutionProvenance {
            mode: "custom_tasks_explicitly_acknowledged".into(),
            tasks_root: canonical_root.display().to_string(),
            trusted_manifest: None,
            custom_execution_acknowledged: true,
        });
    }

    // Built-in trust is fail-closed: inability to resolve the compiled repository path means
    // it cannot establish built-in identity, but the resulting error must still route the
    // operator to the explicit custom-task acknowledgement instead of exposing an unrelated
    // installation-layout dependency.
    let canonical_builtin = fs::canonicalize(builtin_root).ok();
    if canonical_builtin.as_ref() == Some(&canonical_root)
        && builtin_manifest_matches(filter, manifests)
    {
        return Ok(GradeExecutionProvenance {
            mode: "trusted_builtin_manifest".into(),
            tasks_root: canonical_root.display().to_string(),
            trusted_manifest: Some(BUILTIN_TASK_MANIFEST.into()),
            custom_execution_acknowledged: false,
        });
    }
    let reason = if canonical_builtin.as_ref() == Some(&canonical_root) {
        "内置任务内容与编译时可信 manifest/hash 不一致"
    } else if canonical_builtin.is_none() {
        "运行时无法建立编译期内置任务目录身份"
    } else {
        "--tasks 指向第三方任务目录"
    };
    bail!(
        "拒绝执行 grade.txt：{reason}；grade 命令会以当前用户身份在宿主机执行。审核任务树后显式传 --unsafe-custom-grade-exec 确认"
    )
}

fn builtin_manifest_matches(filter: Option<&str>, manifests: &[TaskProvenance]) -> bool {
    let expected: BTreeMap<&str, &str> = BUILTIN_TASK_HASHES.iter().copied().collect();
    let expected_len = if filter.is_some() { 1 } else { expected.len() };
    manifests.len() == expected_len
        && manifests.iter().all(|manifest| {
            expected
                .get(manifest.name.as_str())
                .is_some_and(|hash| *hash == manifest.task_sha256)
        })
}

fn read(path: &Path) -> Result<String> {
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("无法检查: {}", path.display()))?;
    if is_link_or_reparse(&metadata) || !metadata.is_file() {
        bail!("拒绝非普通任务文件: {}", path.display());
    }
    fs::read_to_string(path).with_context(|| format!("无法读取: {}", path.display()))
}

fn ensure_real_dir(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("无法检查 {label}: {}", path.display()))?;
    if is_link_or_reparse(&metadata) || !metadata.is_dir() {
        bail!("{label} 必须是非 symlink 目录: {}", path.display());
    }
    Ok(())
}

fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// 把已授权任务的 fixture/ 递归复制到一个全新的工作区。既有目标是错误，而不是可删除的旧 run。
///
/// `expected_fixture_sha256` comes from the authorized run manifest. Hashing the completed
/// trial copy with the exact same tree algorithm closes the source verify->copy race: a source
/// mutation before or during copying cannot reach the model or grade unless the resulting copy
/// still has the exact authorized identity.
pub fn setup_workspace(task: &Task, dest: &Path, expected_fixture_sha256: &str) -> Result<()> {
    if fs::symlink_metadata(dest).is_ok() {
        bail!("拒绝覆写既有 trial 工作区: {}", dest.display());
    }
    copy_dir(&task.fixture_dir, dest)?;
    let actual_fixture_sha256 = hash_tree(dest)?;
    if actual_fixture_sha256 != expected_fixture_sha256 {
        bail!(
            "trial fixture 副本与已授权输入不一致，拒绝任何 agent/grade 执行: expected {expected_fixture_sha256}, actual {actual_fixture_sha256}, workspace {}",
            dest.display()
        );
    }
    // 给每个 fixture 一个 .RaymanCodingSkill/ 标记，让 rayman 把这个副本当作工作区根，
    // 否则它会沿目录向上找到真实仓库的 .git，把整个仓库当工作区（污染 + 超时）。
    fs::create_dir(dest.join(".RaymanCodingSkill"))
        .with_context(|| format!("无法创建工作区标记: {}", dest.display()))?;
    Ok(())
}

pub fn prepare_grade_workspace(
    task: &Task,
    workspace: &Path,
    authorized: &TaskProvenance,
) -> std::result::Result<OracleSeal, OraclePreparationError> {
    let current = task.provenance().map_err(|error| {
        OraclePreparationError::infrastructure(format!("无法复核 grade 输入身份: {error:#}"))
    })?;
    if current != *authorized {
        return Err(OraclePreparationError::infrastructure(format!(
            "任务输入在 grade 发布前漂移，拒绝评分: {}",
            task.name
        )));
    }
    verify_agent_workspace_delta(task, workspace)?;

    let mut sealed_files = BTreeMap::new();
    let mut expected_tests = Vec::new();
    let mut success_marker = None;
    match &task.contract.oracle {
        OracleContract::RustModule {
            source,
            module_file,
            expected_tests: declared_tests,
        } => {
            let oracle_source = task.oracle_dir.join(source);
            let oracle_bytes =
                read_regular_bytes(&oracle_source, "Rust oracle").map_err(|error| {
                    OraclePreparationError::infrastructure(format!(
                        "无法读取受信任 Rust oracle: {error:#}"
                    ))
                })?;
            let destination = workspace.join(ORACLE_MODULE_PATH);
            match fs::symlink_metadata(&destination) {
                Ok(_) => {
                    return Err(OraclePreparationError::policy(format!(
                        "agent 创建了保留 oracle 路径，拒绝评分: {}",
                        destination.display()
                    )));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(OraclePreparationError::infrastructure(format!(
                        "无法检查保留 Rust oracle 路径 {}: {error}",
                        destination.display()
                    )));
                }
            }
            fs::write(&destination, &oracle_bytes)
                .with_context(|| format!("无法发布 Rust oracle: {}", destination.display()))
                .map_err(|error| OraclePreparationError::infrastructure(format!("{error:#}")))?;

            let module_path = workspace.join(module_file);
            let module_metadata = match fs::symlink_metadata(&module_path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err(OraclePreparationError::policy(format!(
                        "oracle module_file 不存在: {}",
                        module_path.display()
                    )));
                }
                Err(error) => {
                    return Err(OraclePreparationError::infrastructure(format!(
                        "无法检查 oracle module_file {}: {error}",
                        module_path.display()
                    )));
                }
            };
            if is_link_or_reparse(&module_metadata) || !module_metadata.is_file() {
                return Err(OraclePreparationError::policy(format!(
                    "oracle module_file 不是普通文件: {}",
                    module_path.display()
                )));
            }
            let module_before = fs::read_to_string(&module_path)
                .with_context(|| format!("无法读取 oracle module_file: {}", module_path.display()))
                .map_err(|error| OraclePreparationError::infrastructure(format!("{error:#}")))?;
            if module_before.contains("__rayman_oracle") {
                return Err(OraclePreparationError::policy(
                    "agent 预置了保留 oracle module 名称，拒绝评分",
                ));
            }
            fs::OpenOptions::new()
                .append(true)
                .open(&module_path)
                .and_then(|mut file| file.write_all(ORACLE_MODULE_MARKER.as_bytes()))
                .with_context(|| {
                    format!("无法发布 oracle module anchor: {}", module_path.display())
                })
                .map_err(|error| OraclePreparationError::infrastructure(format!("{error:#}")))?;

            sealed_files.insert(
                ORACLE_MODULE_PATH.into(),
                sha256_regular_file(&destination, "已发布 Rust oracle").map_err(|error| {
                    OraclePreparationError::infrastructure(format!("{error:#}"))
                })?,
            );
            sealed_files.insert(
                module_file.clone(),
                sha256_regular_file(&module_path, "oracle module anchor").map_err(|error| {
                    OraclePreparationError::infrastructure(format!("{error:#}"))
                })?,
            );
            expected_tests = declared_tests.clone();
        }
        OracleContract::PowerShell {
            source,
            destination,
            success_marker: declared_marker,
        } => {
            let oracle_source = task.oracle_dir.join(source);
            let oracle_bytes =
                read_regular_bytes(&oracle_source, "PowerShell oracle").map_err(|error| {
                    OraclePreparationError::infrastructure(format!(
                        "无法读取受信任 PowerShell oracle: {error:#}"
                    ))
                })?;
            let destination_path = workspace.join(destination);
            match fs::symlink_metadata(&destination_path) {
                Ok(_) => {
                    return Err(OraclePreparationError::policy(format!(
                        "agent 创建了保留 oracle 路径，拒绝评分: {}",
                        destination_path.display()
                    )));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(OraclePreparationError::infrastructure(format!(
                        "无法检查保留 PowerShell oracle 路径 {}: {error}",
                        destination_path.display()
                    )));
                }
            }
            let parent = destination_path.parent().ok_or_else(|| {
                OraclePreparationError::infrastructure("oracle destination 缺少父目录")
            })?;
            if !parent.is_dir() {
                fs::create_dir(parent)
                    .with_context(|| {
                        format!("无法创建 oracle destination 父目录: {}", parent.display())
                    })
                    .map_err(|error| {
                        OraclePreparationError::infrastructure(format!("{error:#}"))
                    })?;
            }
            fs::write(&destination_path, &oracle_bytes)
                .with_context(|| {
                    format!("无法发布 PowerShell oracle: {}", destination_path.display())
                })
                .map_err(|error| OraclePreparationError::infrastructure(format!("{error:#}")))?;
            sealed_files.insert(
                destination.clone(),
                sha256_regular_file(&destination_path, "已发布 PowerShell oracle").map_err(
                    |error| OraclePreparationError::infrastructure(format!("{error:#}")),
                )?,
            );
            success_marker = Some(declared_marker.clone());
        }
    }
    Ok(OracleSeal {
        sealed_files,
        expected_tests,
        success_marker,
    })
}

pub fn verify_grade_seal(workspace: &Path, seal: &OracleSeal) -> Result<()> {
    for (relative, expected) in &seal.sealed_files {
        let actual = sha256_regular_file(&workspace.join(relative), "grade oracle seal")?;
        if &actual != expected {
            bail!("grade 执行期间 oracle 漂移: {relative}");
        }
    }
    Ok(())
}

pub fn verify_grade_observation(
    workspace: &Path,
    seal: &OracleSeal,
    stdout: &str,
    stderr: &str,
) -> Result<()> {
    verify_grade_seal(workspace, seal)?;
    let combined = format!("{stdout}\n{stderr}");
    for test in &seal.expected_tests {
        let expected = format!("test __rayman_oracle::{test} ... ok");
        if !combined.contains(&expected) {
            bail!("grade 未执行并通过受保护测试: __rayman_oracle::{test}");
        }
    }
    if let Some(marker) = &seal.success_marker
        && !combined.contains(marker)
    {
        bail!("grade 未产生受保护 oracle 成功标记: {marker}");
    }
    Ok(())
}

fn verify_agent_workspace_delta(
    task: &Task,
    workspace: &Path,
) -> std::result::Result<(), OraclePreparationError> {
    let baseline = file_manifest(&task.fixture_dir).map_err(|error| {
        OraclePreparationError::infrastructure(format!("无法读取 fixture 基线: {error:#}"))
    })?;
    let current = file_manifest(workspace).map_err(|error| {
        OraclePreparationError::infrastructure(format!("无法读取 agent workspace: {error:#}"))
    })?;
    for (path, expected) in &baseline {
        if editable_path(&task.contract, path) {
            continue;
        }
        match current.get(path) {
            Some(actual) if actual == expected => {}
            Some(_) => {
                return Err(OraclePreparationError::policy(format!(
                    "agent 修改了非 editable fixture 文件: {path}"
                )));
            }
            None => {
                return Err(OraclePreparationError::policy(format!(
                    "agent 删除了非 editable fixture 文件: {path}"
                )));
            }
        }
    }
    for path in current.keys() {
        if !baseline.contains_key(path) && !editable_path(&task.contract, path) {
            return Err(OraclePreparationError::policy(format!(
                "agent 创建了 editable scope 外文件: {path}"
            )));
        }
    }
    Ok(())
}

fn editable_path(contract: &TaskContract, path: &str) -> bool {
    contract.editable_paths.iter().any(|value| value == path)
        || contract
            .editable_prefixes
            .iter()
            .any(|prefix| path == prefix || path.starts_with(&format!("{prefix}/")))
}

fn file_manifest(root: &Path) -> Result<BTreeMap<String, String>> {
    ensure_real_dir(root, "manifest root")?;
    let mut files = BTreeMap::new();
    collect_file_manifest(root, root, &mut files)?;
    Ok(files)
}

fn collect_file_manifest(
    root: &Path,
    path: &Path,
    files: &mut BTreeMap<String, String>,
) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("无法检查 workspace manifest 输入: {}", path.display()))?;
    if is_link_or_reparse(&metadata) {
        bail!("拒绝 workspace manifest symlink: {}", path.display());
    }
    if metadata.is_dir() {
        let mut entries = fs::read_dir(path)
            .with_context(|| format!("无法读取 workspace manifest: {}", path.display()))?
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name();
            let child = entry.path();
            let child_metadata = fs::symlink_metadata(&child)?;
            if child_metadata.is_dir()
                && matches!(
                    name.to_string_lossy().as_ref(),
                    "target" | ".git" | "node_modules" | ".RaymanCodingSkill"
                )
            {
                continue;
            }
            collect_file_manifest(root, &child, files)?;
        }
    } else if metadata.is_file() {
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        files.insert(
            relative,
            sha256_regular_file(path, "workspace manifest file")?,
        );
    } else {
        bail!("拒绝 workspace manifest 非普通文件: {}", path.display());
    }
    Ok(())
}

fn read_regular_bytes(path: &Path, label: &str) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("无法检查 {label}: {}", path.display()))?;
    if is_link_or_reparse(&metadata) || !metadata.is_file() {
        bail!("{label} 必须是普通文件: {}", path.display());
    }
    fs::read(path).with_context(|| format!("无法读取 {label}: {}", path.display()))
}

fn sha256_regular_file(path: &Path, label: &str) -> Result<String> {
    Ok(sha256_bytes(&read_regular_bytes(path, label)?))
}

fn copy_dir(src: &Path, dest: &Path) -> Result<()> {
    ensure_real_dir(src, "fixture 目录")?;
    match fs::create_dir(dest) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            bail!("拒绝覆写既有目录: {}", dest.display());
        }
        Err(error) => {
            return Err(error).with_context(|| format!("无法创建目录: {}", dest.display()));
        }
    }
    let entries = fs::read_dir(src).with_context(|| format!("无法读取目录: {}", src.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("无法枚举目录: {}", src.display()))?;
        let from = entry.path();
        let name = entry.file_name();
        let metadata = fs::symlink_metadata(&from)
            .with_context(|| format!("无法检查 fixture 输入: {}", from.display()))?;
        if is_link_or_reparse(&metadata) {
            bail!("拒绝 fixture 内 symlink: {}", from.display());
        }
        // 构建产物/元数据不属于任务起点，复制会拖慢评测并污染工作区。
        if metadata.is_dir()
            && matches!(
                name.to_string_lossy().as_ref(),
                "target" | ".git" | "node_modules" | ".RaymanCodingSkill"
            )
        {
            continue;
        }
        let to = dest.join(name);
        if metadata.is_dir() {
            copy_dir(&from, &to)?;
        } else if metadata.is_file() {
            fs::copy(&from, &to)
                .with_context(|| format!("无法复制 {} -> {}", from.display(), to.display()))?;
        } else {
            bail!("拒绝 fixture 内非普通文件: {}", from.display());
        }
    }
    Ok(())
}

fn hash_tree(root: &Path) -> Result<String> {
    ensure_real_dir(root, "fixture")?;
    let mut hasher = Sha256::new();
    hash_tree_into(root, root, &mut hasher)?;
    Ok(hex::encode(hasher.finalize()))
}

fn hash_tree_into(root: &Path, path: &Path, hasher: &mut Sha256) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("无法检查 fixture 输入: {}", path.display()))?;
    if is_link_or_reparse(&metadata) {
        bail!("拒绝 fixture 内 symlink: {}", path.display());
    }
    let relative = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    if metadata.is_dir() {
        hasher.update(b"dir\0");
        hasher.update(relative.as_bytes());
        hasher.update(b"\0");
        let mut entries: Vec<_> = fs::read_dir(path)
            .with_context(|| format!("无法读取 fixture 目录: {}", path.display()))?
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let child = entry.path();
            let child_metadata = fs::symlink_metadata(&child)
                .with_context(|| format!("无法检查 fixture 输入: {}", child.display()))?;
            if child_metadata.is_dir()
                && matches!(
                    entry.file_name().to_string_lossy().as_ref(),
                    "target" | ".git" | "node_modules" | ".RaymanCodingSkill"
                )
            {
                continue;
            }
            hash_tree_into(root, &child, hasher)?;
        }
    } else if metadata.is_file() {
        hasher.update(b"file\0");
        hasher.update(relative.as_bytes());
        hasher.update(b"\0");
        let bytes =
            fs::read(path).with_context(|| format!("无法读取 fixture 文件: {}", path.display()))?;
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    } else {
        bail!("拒绝 fixture 内非普通文件: {}", path.display());
    }
    Ok(())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn sha256_parts(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    hex::encode(hasher.finalize())
}

// `hex` 的格式化逻辑很小，但不值得为了它再引入一条依赖树。
mod hex {
    use std::fmt::Write;

    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        let mut out = String::with_capacity(bytes.as_ref().len() * 2);
        for byte in bytes.as_ref() {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    fn seed_contract(task_dir: &Path) -> TaskContract {
        let contract = TaskContract {
            schema: TASK_CONTRACT_SCHEMA.into(),
            source_ids: vec!["SRC-TEST".into()],
            rule_ids: vec!["RULE-TEST".into()],
            activate_workspace: false,
            editable_paths: vec!["src/lib.rs".into()],
            editable_prefixes: Vec::new(),
            oracle: OracleContract::RustModule {
                source: "tests.rs".into(),
                module_file: "src/lib.rs".into(),
                expected_tests: vec!["oracle_smoke".into()],
            },
        };
        write(
            &task_dir.join("task.json"),
            &serde_json::to_string_pretty(&contract).unwrap(),
        );
        write(
            &task_dir.join("oracle/tests.rs"),
            "#[test]\nfn oracle_smoke() {}\n",
        );
        contract
    }

    fn sample_task(task_dir: &Path, fixture: PathBuf, prompt: &str, grade: &str) -> Task {
        let prompt_path = task_dir.join("prompt.md");
        let grade_path = task_dir.join("grade.txt");
        write(&prompt_path, prompt);
        write(&grade_path, grade);
        let contract = seed_contract(task_dir);
        Task {
            name: "sample".into(),
            prompt: prompt.trim().into(),
            prompt_path,
            fixture_dir: fixture,
            grade_cmd: grade.trim().into(),
            grade_path,
            contract_path: task_dir.join("task.json"),
            oracle_dir: task_dir.join("oracle"),
            contract,
        }
    }

    #[test]
    fn load_tasks_filters_and_sorts() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path();
        for name in ["zeta", "alpha"] {
            let task_dir = tasks.join(name);
            write(&task_dir.join("prompt.md"), &format!("fix {name}\n"));
            write(&task_dir.join("grade.txt"), "cargo test\n");
            write(&task_dir.join("fixture/src/lib.rs"), "pub fn ok() {}\n");
            seed_contract(&task_dir);
        }

        let all = load_tasks(tasks, None).unwrap();
        assert_eq!(
            all.iter()
                .map(|task| task.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "zeta"]
        );

        let filtered = load_tasks(tasks, Some("zeta")).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "zeta");
        assert_eq!(filtered[0].prompt, "fix zeta");
    }

    #[test]
    fn repository_tasks_match_the_compiled_trusted_manifest() {
        let tasks_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tasks");
        let tasks = load_tasks(&tasks_root, None).unwrap();
        let manifests = tasks
            .iter()
            .map(Task::provenance)
            .collect::<Result<Vec<_>>>()
            .unwrap();

        let authorization = authorize_grade_execution(&tasks_root, None, &manifests, false)
            .unwrap_or_else(|error| panic!("{error}\nactual manifests: {manifests:#?}"));

        assert_eq!(authorization.mode, "trusted_builtin_manifest");
        assert_eq!(
            authorization.trusted_manifest.as_deref(),
            Some(BUILTIN_TASK_MANIFEST)
        );
        assert!(!authorization.custom_execution_acknowledged);
    }

    #[test]
    fn custom_tasks_require_a_separate_explicit_grade_acknowledgement() {
        let dir = tempfile::tempdir().unwrap();
        let task_dir = dir.path().join("custom");
        write(&task_dir.join("prompt.md"), "fix it\n");
        write(&task_dir.join("grade.txt"), "echo host-command\n");
        write(&task_dir.join("fixture/src/lib.rs"), "pub fn ok() {}\n");
        seed_contract(&task_dir);
        let tasks = load_tasks(dir.path(), None).unwrap();
        let manifests = tasks
            .iter()
            .map(Task::provenance)
            .collect::<Result<Vec<_>>>()
            .unwrap();

        let error = authorize_grade_execution(dir.path(), None, &manifests, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("--unsafe-custom-grade-exec"), "{error}");

        let authorization = authorize_grade_execution(dir.path(), None, &manifests, true).unwrap();
        assert_eq!(authorization.mode, "custom_tasks_explicitly_acknowledged");
        assert!(authorization.custom_execution_acknowledged);
        assert!(authorization.trusted_manifest.is_none());
    }

    #[test]
    fn custom_ack_does_not_require_the_compiled_builtin_directory_at_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let tasks_root = dir.path().join("custom-tasks");
        let task_dir = tasks_root.join("custom");
        write(&task_dir.join("prompt.md"), "fix it\n");
        write(&task_dir.join("grade.txt"), "echo host-command\n");
        write(&task_dir.join("fixture/src/lib.rs"), "pub fn ok() {}\n");
        seed_contract(&task_dir);
        let tasks = load_tasks(&tasks_root, None).unwrap();
        let manifests = tasks
            .iter()
            .map(Task::provenance)
            .collect::<Result<Vec<_>>>()
            .unwrap();
        let missing_builtin_root = dir.path().join("compiled-builtins-not-installed");

        let authorization = authorize_grade_execution_with_builtin_root(
            &tasks_root,
            None,
            &manifests,
            true,
            &missing_builtin_root,
        )
        .unwrap();

        assert_eq!(authorization.mode, "custom_tasks_explicitly_acknowledged");
        assert!(authorization.custom_execution_acknowledged);
        assert!(authorization.trusted_manifest.is_none());

        let error = authorize_grade_execution_with_builtin_root(
            &tasks_root,
            None,
            &manifests,
            false,
            &missing_builtin_root,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("--unsafe-custom-grade-exec"), "{error}");
    }

    #[test]
    fn builtin_hash_mismatch_is_not_trusted() {
        let mut manifest = TaskProvenance {
            name: "add-feature".into(),
            prompt_sha256: String::new(),
            grade_sha256: String::new(),
            fixture_sha256: String::new(),
            contract_sha256: String::new(),
            oracle_sha256: String::new(),
            task_sha256: BUILTIN_TASK_HASHES[0].1.into(),
        };
        assert!(builtin_manifest_matches(
            Some("add-feature"),
            &[manifest.clone()]
        ));
        manifest.task_sha256 = "0".repeat(64);
        assert!(!builtin_manifest_matches(Some("add-feature"), &[manifest]));
    }

    #[test]
    fn load_tasks_skips_stray_dirs_without_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path();
        let task_dir = tasks.join("real");
        write(&task_dir.join("prompt.md"), "fix it\n");
        write(&task_dir.join("grade.txt"), "cargo test\n");
        write(&task_dir.join("fixture/src/lib.rs"), "pub fn ok() {}\n");
        seed_contract(&task_dir);
        // 杂散目录：没有 prompt.md，应被跳过而非报错。
        write(&tasks.join("stray/junk.txt"), "not a task\n");

        let all = load_tasks(tasks, None).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "real");
    }

    #[test]
    fn copy_dir_skips_build_and_vcs_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("fixture");
        write(&src.join("src/lib.rs"), "pub fn ok() {}\n");
        for skipped in ["target", ".git", "node_modules", ".RaymanCodingSkill"] {
            write(&src.join(skipped).join("junk"), "x");
        }
        let dest = dir.path().join("dest");

        copy_dir(&src, &dest).unwrap();

        assert!(dest.join("src/lib.rs").exists());
        for skipped in ["target", ".git", "node_modules", ".RaymanCodingSkill"] {
            assert!(!dest.join(skipped).exists(), "{skipped} 不应被复制");
        }
    }

    #[test]
    fn setup_workspace_copies_fixture_and_refuses_reuse() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path().join("fixture");
        write(&fixture.join("src/lib.rs"), "pub fn ok() {}\n");
        let task = sample_task(dir.path(), fixture, "fix it\n", "cargo test\n");
        let dest = dir.path().join("workspace");

        let expected_fixture_sha256 = hash_tree(&task.fixture_dir).unwrap();
        setup_workspace(&task, &dest, &expected_fixture_sha256).unwrap();

        assert!(dest.join("src/lib.rs").exists());
        assert!(dest.join(".RaymanCodingSkill").is_dir());
        assert!(setup_workspace(&task, &dest, &expected_fixture_sha256).is_err());
    }

    #[test]
    fn setup_workspace_rejects_a_source_change_after_authorization() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path().join("fixture");
        write(&fixture.join("src/lib.rs"), "pub fn authorized() {}\n");
        let expected_fixture_sha256 = hash_tree(&fixture).unwrap();
        let task = sample_task(dir.path(), fixture.clone(), "fix it\n", "cargo test\n");
        // Simulate the exact verify->copy window: authorization hashed the first tree, then
        // the task source changed before this trial copied it.
        write(&fixture.join("src/lib.rs"), "pub fn unauthorized() {}\n");
        let dest = dir.path().join("workspace");

        let error = setup_workspace(&task, &dest, &expected_fixture_sha256)
            .unwrap_err()
            .to_string();

        assert!(error.contains("拒绝任何 agent/grade 执行"), "{error}");
        assert!(!dest.join(".RaymanCodingSkill").exists());
    }

    #[test]
    fn provenance_hash_binds_fixture_content_and_rejects_cached_input_drift() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path().join("fixture");
        write(&fixture.join("src/lib.rs"), "pub fn ok() {}\n");
        let task = sample_task(dir.path(), fixture.clone(), "fix it\n", "cargo test\n");
        let prompt_path = task.prompt_path.clone();
        let grade_path = task.grade_path.clone();
        let first = task.provenance().unwrap();
        write(&fixture.join("src/lib.rs"), "pub fn changed() {}\n");
        let second = task.provenance().unwrap();
        assert_ne!(first.fixture_sha256, second.fixture_sha256);
        assert_ne!(first.task_sha256, second.task_sha256);

        write(&prompt_path, "fix a different issue\n");
        let prompt_error = task.provenance().unwrap_err().to_string();
        assert!(prompt_error.contains("prompt changed after load"));

        write(&prompt_path, "fix it\n");
        write(&grade_path, "cargo test --all\n");
        let grade_error = task.provenance().unwrap_err().to_string();
        assert!(grade_error.contains("grade changed after load"));
    }

    #[test]
    fn post_agent_oracle_rejects_noneditable_workspace_changes() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path().join("fixture");
        write(&fixture.join("src/lib.rs"), "pub fn value() -> u8 { 1 }\n");
        write(
            &fixture.join("Cargo.toml"),
            "[package]\nname='sample'\nversion='0.1.0'\nedition='2021'\n",
        );
        let task = sample_task(dir.path(), fixture, "fix it\n", "cargo test\n");
        let manifest = task.provenance().unwrap();
        let workspace = dir.path().join("workspace");
        setup_workspace(&task, &workspace, &manifest.fixture_sha256).unwrap();
        write(
            &workspace.join("Cargo.toml"),
            "[package]\nname='forged'\nversion='0.1.0'\nedition='2021'\n",
        );

        let error = prepare_grade_workspace(&task, &workspace, &manifest)
            .unwrap_err()
            .to_string();
        assert!(error.contains("非 editable"), "{error}");
        assert!(!workspace.join(ORACLE_MODULE_PATH).exists());
    }

    #[test]
    fn post_agent_oracle_is_published_late_sealed_and_observed() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path().join("fixture");
        write(&fixture.join("src/lib.rs"), "pub fn value() -> u8 { 1 }\n");
        let task = sample_task(dir.path(), fixture, "fix it\n", "cargo test\n");
        let manifest = task.provenance().unwrap();
        let workspace = dir.path().join("workspace");
        setup_workspace(&task, &workspace, &manifest.fixture_sha256).unwrap();
        assert!(!workspace.join(ORACLE_MODULE_PATH).exists());

        let seal = prepare_grade_workspace(&task, &workspace, &manifest).unwrap();
        assert!(workspace.join(ORACLE_MODULE_PATH).is_file());
        verify_grade_observation(
            &workspace,
            &seal,
            "test __rayman_oracle::oracle_smoke ... ok",
            "",
        )
        .unwrap();
        let missing = verify_grade_observation(&workspace, &seal, "test result: ok", "")
            .unwrap_err()
            .to_string();
        assert!(missing.contains("未执行并通过"), "{missing}");

        write(
            &workspace.join(ORACLE_MODULE_PATH),
            "#[test]\nfn replaced() {}\n",
        );
        let drift = verify_grade_observation(
            &workspace,
            &seal,
            "test __rayman_oracle::oracle_smoke ... ok",
            "",
        )
        .unwrap_err()
        .to_string();
        assert!(drift.contains("oracle 漂移"), "{drift}");
    }

    #[cfg(windows)]
    #[test]
    fn load_tasks_rejects_top_level_custom_rayman_wrapper_in_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let task_dir = dir.path().join("task");
        write(&task_dir.join("prompt.md"), "fix it\n");
        write(&task_dir.join("grade.txt"), "cargo test\n");
        seed_contract(&task_dir);
        write(
            &task_dir.join("fixture/RAYMAN.custom.extension"),
            "@echo off",
        );

        let error = load_tasks(dir.path(), None).unwrap_err().to_string();
        assert!(error.contains("rayman"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn fixture_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("fixture");
        write(&src.join("normal.txt"), "ok");
        let outside = dir.path().join("outside.txt");
        write(&outside, "secret");
        symlink(&outside, src.join("escape.txt")).unwrap();

        assert!(copy_dir(&src, &dir.path().join("dest")).is_err());
        assert!(hash_tree(&src).is_err());
    }
}
