use super::*;

mod cargo_isolation;
mod installation;
mod process_temp;
mod pytest_isolation;
mod receipts;

pub use cargo_isolation::ValidationExecutionSession;
use installation::{
    broker_install_invocation, broker_install_invocation_with_context,
    release_installer_invocation, release_installer_invocation_with_context,
};
pub use process_temp::{
    run_with_managed_validation_temp, test_invocation_requires_pytest_isolation,
};
pub use pytest_isolation::run_with_managed_pytest_lease;
use pytest_isolation::{
    insert_pytest_args_before_separator, pytest_has_pre_separator_option,
    validate_pytest_isolation_overrides,
};
use receipts::GoalPlanningValidationPolicy;
pub(crate) use receipts::validation_has_current_receipt_with_context;
#[allow(
    unused_imports,
    reason = "preserve the pre-split goal-module receipt facade"
)]
pub(super) use receipts::{
    ReceiptValidationPolicy, authority_scope_is_well_formed,
    direct_stable_authority_receipt_is_valid,
    direct_stable_authority_receipt_is_valid_with_baseline,
    direct_stable_authority_receipt_is_valid_with_context,
    goal_success_historical_receipt_gaps_with_identity,
    has_archived_direct_stable_authority_command,
    has_archived_direct_stable_authority_command_with_context, has_direct_stable_authority_command,
    has_direct_stable_authority_receipt, has_direct_stable_authority_receipt_with_baseline,
    has_direct_stable_authority_receipt_with_context, is_sha256,
    validation_has_historical_receipt_for_fingerprint_with_identity,
    validation_has_receipt_for_fingerprint, validation_has_receipt_for_fingerprint_with_context,
};
pub use receipts::{
    authority_invocation_sha256, authority_invocation_sha256_mode, authority_receipt_sha256,
    goal_contract_sha256, has_current_stable_authority_receipt,
    has_current_stable_authority_receipt_with_baseline,
    has_current_stable_authority_receipt_with_context, validation_contract_sha256,
    validation_has_current_receipt, validation_has_current_receipt_with_baseline,
    validation_invocation_sha256, validation_invocation_sha256_scoped,
    validation_invocation_sha256_scoped_mode, validation_scopes_for_impacts,
};

/// Parse a validation command into one executable plus an argv vector.  It is
/// intentionally not a shell grammar: control operators and nested shell
/// hosts are rejected, so a later successful command cannot hide an earlier
/// failure (`cargo test || exit 0`, `cmd /C ...`, and similar forms).
pub fn parse_validation_command(command: &str) -> Result<ParsedValidationCommand> {
    let command = command.trim();
    if command.is_empty() {
        bail!("验证命令不能为空");
    }
    if command.contains("$(")
        || command
            .chars()
            .any(|c| matches!(c, '&' | '|' | ';' | '<' | '>' | '`' | '\r' | '\n' | '\0'))
    {
        bail!("验证命令不允许 shell 控制符或命令替换；请提供单一可执行程序及参数");
    }

    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut token_started = false;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(active) if c == active => {
                quote = None;
                token_started = true;
            }
            Some('"') if c == '\\' => {
                if matches!(chars.peek(), Some('"' | '\\')) {
                    current.push(chars.next().expect("peeked character must exist"));
                } else {
                    current.push(c);
                }
                token_started = true;
            }
            Some(_) => {
                current.push(c);
                token_started = true;
            }
            None if matches!(c, '\'' | '"') => {
                quote = Some(c);
                token_started = true;
            }
            None if c.is_whitespace() => {
                if token_started {
                    words.push(std::mem::take(&mut current));
                    token_started = false;
                }
            }
            None if c == '\\' => {
                if matches!(chars.peek(), Some(next) if next.is_whitespace() || matches!(next, '\'' | '"' | '\\'))
                {
                    current.push(chars.next().expect("peeked character must exist"));
                } else {
                    current.push(c);
                }
                token_started = true;
            }
            None => {
                current.push(c);
                token_started = true;
            }
        }
    }
    if quote.is_some() {
        bail!("验证命令包含未闭合的引号");
    }
    if token_started {
        words.push(current);
    }
    if words.is_empty() || words[0].trim().is_empty() {
        bail!("验证命令缺少可执行程序");
    }

    let program = words.remove(0);
    let executable = Path::new(&program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&program)
        .to_ascii_lowercase();
    let executable = executable.strip_suffix(".exe").unwrap_or(&executable);
    let is_powershell = matches!(executable, "powershell" | "pwsh");
    let safe_powershell_file = is_powershell
        && words.len() >= 3
        && words[0].eq_ignore_ascii_case("-NoProfile")
        && words[1].eq_ignore_ascii_case("-File")
        && words[2].to_ascii_lowercase().ends_with(".ps1");
    let is_shell_host = matches!(executable, "cmd" | "sh" | "bash" | "zsh" | "fish" | "wsl")
        || (is_powershell && !safe_powershell_file)
        || executable.ends_with(".cmd")
        || executable.ends_with(".bat")
        || executable.ends_with(".ps1");
    if is_shell_host {
        // Naming only what is forbidden sent a real session down a dead end: it
        // read "no shell" as "a PowerShell repository gate cannot be recorded at
        // all", invented a workaround, and finally concluded the CLI could not
        // express its own documented gate. The accepted form has to be in the
        // message, because that is where the reader is standing.
        if is_powershell {
            bail!(
                "验证命令不能启动 shell；PowerShell 脚本请用 `pwsh -NoProfile -File <script>.ps1 [参数...]` 这一种形式"
            );
        }
        bail!("验证命令不能启动 shell；请直接提供要执行的程序及参数");
    }

    Ok(ParsedValidationCommand {
        program,
        args: words,
    })
}

pub(super) fn executable_name(command: &ParsedValidationCommand) -> String {
    let executable = Path::new(&command.program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&command.program)
        .to_ascii_lowercase();
    executable
        .strip_suffix(".exe")
        .unwrap_or(&executable)
        .to_string()
}

pub(super) fn powershell_script(command: &ParsedValidationCommand) -> Option<&str> {
    if matches!(executable_name(command).as_str(), "powershell" | "pwsh")
        && command.args.len() >= 3
        && command.args[0].eq_ignore_ascii_case("-NoProfile")
        && command.args[1].eq_ignore_ascii_case("-File")
    {
        command.args.get(2).map(String::as_str)
    } else {
        None
    }
}

fn cargo_test_invocation(command: &ParsedValidationCommand) -> bool {
    matches!(cargo_subcommand(command), Some(("test", _)))
        || matches!(cargo_subcommand(command), Some(("nextest", Some("run"))))
}

/// 不带值的 Python 解释器短标志字母（可组合，如 `-Es`、`-OO`）。
const PYTHON_VALUELESS_FLAGS: &str = "bBdEiIOPqsSuvx";

/// `python ... -m pytest` 中 `pytest` 之后第一个参数的下标。
///
/// `-m` 只有出现在**解释器选项位**才算数。Python 遇到 `-c`、`-` 或脚本路径后就
/// 停止解析自己的选项，把余下参数原样交给用户代码，此时 `-m pytest` 只是惰性的
/// `sys.argv` 内容，pytest 根本不会运行。把那种形式当成测试证明，等于让任意
/// 代码冒充测试结果——`parse_validation_command` 已经为此拒绝了 cmd/sh/pwsh，
/// `python -c` 属于同一类宿主。无法确定的形式一律返回 None（fail-closed）：
/// 命令只是不被当作测试证明，不会被误判为通过。
fn python_pytest_module_index(command: &ParsedValidationCommand) -> Option<usize> {
    let executable = executable_name(command);
    let mut index = 0;
    // Windows 的 py 启动器版本选择符，如 `py -3 -m pytest`、`py -3.12-64 -m pytest`。
    if executable == "py"
        && command.args.first().is_some_and(|arg| {
            arg.starts_with('-') && arg[1..].starts_with(|c: char| c.is_ascii_digit())
        })
    {
        index = 1;
    }
    while let Some(argument) = command.args.get(index) {
        if argument == "-m" {
            return match command.args.get(index + 1) {
                Some(module) if module == "pytest" => Some(index + 2),
                _ => None,
            };
        }
        let flags = argument.strip_prefix('-')?;
        if flags.is_empty() {
            return None; // `-`：从 stdin 读取代码
        }
        if let Some(rest) = flags.strip_prefix('W').or_else(|| flags.strip_prefix('X')) {
            index += if rest.is_empty() { 2 } else { 1 };
            continue;
        }
        if !flags
            .chars()
            .all(|flag| PYTHON_VALUELESS_FLAGS.contains(flag))
        {
            return None; // 含 `-c` 或未知带值选项
        }
        index += 1;
    }
    None
}

pub(super) fn pytest_invocation(command: &ParsedValidationCommand) -> bool {
    let executable = executable_name(command);
    if matches!(executable.as_str(), "pytest" | "py.test") {
        return true;
    }
    if executable == "py" || executable.starts_with("python") {
        return python_pytest_module_index(command).is_some();
    }
    false
}

fn test_invocation(command: &ParsedValidationCommand) -> bool {
    cargo_test_invocation(command) || pytest_invocation(command)
}

/// Ordinary workspace-owned PowerShell scripts are permitted as local
/// validation evidence, but the reviewed gate basenames are reserved.  This
/// keeps an arbitrary `tools/check-repo.ps1` (or an installer's `-Self*`
/// branch) from borrowing Rust/Cargo coverage merely because it exists under
/// the workspace.
fn ordinary_workspace_powershell_validation(
    root: &Path,
    command: &ParsedValidationCommand,
) -> bool {
    powershell_script(command).is_some()
        && !powershell_script_has_reserved_gate_basename(command)
        && resolve_live_powershell_script(root, command)
            .ok()
            .flatten()
            .is_some()
}

fn ordinary_workspace_powershell_validation_with_context(
    decision: &GoalDecisionContext<'_>,
    command: &ParsedValidationCommand,
) -> bool {
    powershell_script(command).is_some()
        && !powershell_script_has_reserved_gate_basename(command)
        && captured_workspace_powershell_key_with_context(decision, command)
            .ok()
            .flatten()
            .is_some()
}

fn python_quick_validate_invocation(command: &ParsedValidationCommand) -> bool {
    let executable = executable_name(command);
    if Path::new(&executable)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("quick_validate.py"))
    {
        return !command_is_inert_probe(command);
    }
    if executable != "py" && !executable.starts_with("python") {
        return false;
    }
    let mut index = 0;
    if executable == "py"
        && command.args.first().is_some_and(|argument| {
            argument.starts_with('-')
                && argument
                    .get(1..)
                    .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        })
    {
        index = 1;
    }
    let Some(script) = command.args.get(index) else {
        return false;
    };
    !script.starts_with('-')
        && Path::new(script)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("quick_validate.py"))
        && !command_is_inert_probe(command)
}

fn markdownlint_documentation_invocation(command: &ParsedValidationCommand) -> bool {
    executable_name(command) == "markdownlint"
        && !command_is_inert_probe(command)
        && command
            .args
            .iter()
            .any(|argument| !argument.starts_with('-') && !argument.trim().is_empty())
}

fn documentation_invocation(script: &str, command: &ParsedValidationCommand) -> bool {
    (script == "check-agent-instructions.ps1" && !command_is_inert_probe(command))
        || markdownlint_documentation_invocation(command)
        || python_quick_validate_invocation(command)
}

fn git_clean_head_invocation(command: &ParsedValidationCommand) -> bool {
    executable_name(command) == "git"
        && command.args
            == [
                "status".to_string(),
                "--porcelain=v1".to_string(),
                "--untracked-files=all".to_string(),
            ]
}

pub fn validation_proof_kind(root: &Path, command: &str) -> Result<ProofKind> {
    let parsed = parse_validation_command(command)?;
    let script = trusted_gate_script(root, &parsed).unwrap_or_default();

    if broker_install_invocation(root, &parsed)? {
        return Ok(ProofKind::Installation);
    }

    if trusted_xtask_repository_gate(root, &parsed)? {
        return Ok(ProofKind::RepositoryGate);
    }
    if trusted_source_fresh_gate_script(root, &parsed) {
        return Ok(ProofKind::SourceFresh);
    }
    if trusted_workspace_gate_script(root, &parsed) {
        return Ok(ProofKind::RepositoryGate);
    }
    if release_installer_invocation(root, &parsed) {
        return Ok(ProofKind::Installation);
    }
    if documentation_invocation(script, &parsed) {
        return Ok(ProofKind::Documentation);
    }
    if git_clean_head_invocation(&parsed) {
        return Ok(ProofKind::GitCommit);
    }
    if test_invocation(&parsed) {
        return Ok(ProofKind::Test);
    }
    Ok(ProofKind::Generic)
}

/// Capture-only proof classification for readiness.  It deliberately avoids
/// reopening a PowerShell path after the decision capture.
pub(crate) fn validation_proof_kind_with_context(
    decision: &GoalDecisionContext<'_>,
    command: &str,
) -> Result<ProofKind> {
    let parsed = parse_validation_command(command)?;
    let script = trusted_gate_script_with_context(decision, &parsed)?.unwrap_or_default();
    if broker_install_invocation_with_context(decision, &parsed)? {
        return Ok(ProofKind::Installation);
    }
    if trusted_xtask_repository_gate_with_context(decision, &parsed)? {
        return Ok(ProofKind::RepositoryGate);
    }
    if trusted_source_fresh_gate_script_with_context(decision, &parsed)? {
        return Ok(ProofKind::SourceFresh);
    }
    if trusted_workspace_gate_script_with_context(decision, &parsed)? {
        return Ok(ProofKind::RepositoryGate);
    }
    if release_installer_invocation_with_context(decision, &parsed)? {
        return Ok(ProofKind::Installation);
    }
    if documentation_invocation(script, &parsed) {
        return Ok(ProofKind::Documentation);
    }
    if git_clean_head_invocation(&parsed) {
        return Ok(ProofKind::GitCommit);
    }
    if test_invocation(&parsed) {
        return Ok(ProofKind::Test);
    }
    Ok(ProofKind::Generic)
}

pub fn proof_kind_matches(required: Option<ProofKind>, actual: ProofKind) -> bool {
    matches!(required, None | Some(ProofKind::Generic)) || required == Some(actual)
}

fn pytest_arguments(command: &ParsedValidationCommand) -> &[String] {
    let executable = executable_name(command);
    if (executable == "py" || executable.starts_with("python"))
        && let Some(index) = python_pytest_module_index(command)
    {
        return &command.args[index..];
    }
    &command.args
}

fn pytest_option_takes_value(argument: &str) -> bool {
    matches!(
        argument,
        "-k" | "-m"
            | "-o"
            | "--override-ini"
            | "--rootdir"
            | "--confcutdir"
            | "--basetemp"
            | "--junitxml"
            | "--junit-xml"
            | "--junit-prefix"
            | "--ignore"
            | "--ignore-glob"
            | "--deselect"
            | "--tb"
            | "--capture"
            | "--color"
            | "--code-highlight"
            | "--durations"
            | "--durations-min"
            | "--verbosity"
            | "--maxfail"
            | "--log-file"
            | "--log-file-level"
            | "--log-file-format"
            | "--log-file-date-format"
            | "-n"
            | "--numprocesses"
            | "--dist"
            | "--tx"
            | "--px"
            | "--max-worker-restart"
            | "--maxprocessesrestart"
    )
}

pub(super) fn pytest_path_arguments(command: &ParsedValidationCommand) -> Vec<&str> {
    if !pytest_invocation(command) {
        return Vec::new();
    }
    let mut selectors = Vec::new();
    let mut positional_only = false;
    let mut skip_value = false;
    for argument in pytest_arguments(command) {
        if skip_value {
            skip_value = false;
            continue;
        }
        if !positional_only && argument == "--" {
            positional_only = true;
            continue;
        }
        if !positional_only && argument.starts_with('-') {
            if !argument.contains('=') && pytest_option_takes_value(argument) {
                skip_value = true;
            }
            continue;
        }
        selectors.push(argument.as_str());
    }
    selectors
}

fn pytest_selector_path(argument: &str) -> &str {
    argument.split("::").next().unwrap_or(argument)
}
fn validate_test_execution_mode(command: &ParsedValidationCommand) -> Result<()> {
    if !test_invocation(command) {
        return Ok(());
    }
    validate_pytest_isolation_overrides(command)?;
    // Exact literals only covered the spellings someone happened to think of.
    // pytest accepts `--co` for `--collect-only` and has several other modes
    // that collect or plan without executing, each of which would otherwise
    // produce a "successful" zero-test receipt.
    const NON_EXECUTING: &[&str] = &[
        "--no-run",
        "--list",
        "--collect-only",
        "--co",
        "--setup-only",
        "--setup-plan",
        "--fixtures",
        "--fixtures-per-test",
        "--markers",
        "--collect-in-virtualenv",
        "--help",
        "-h",
        "--version",
        "-V",
    ];
    if let Some(flag) = command.args.iter().find(|arg| {
        let name = arg.split('=').next().unwrap_or(arg);
        NON_EXECUTING.contains(&name)
    }) {
        bail!("测试验证命令包含非执行模式 {flag}；receipt 必须实际运行至少一个测试");
    }
    if matches!(cargo_subcommand(command), Some(("nextest", _))) {
        bail!("nextest 暂无独立 list proof 支持；请使用 `cargo test` 生成 receipt");
    }
    Ok(())
}

pub fn validate_command_security(root: &Path, command: &ParsedValidationCommand) -> Result<()> {
    validate_test_execution_mode(command)?;
    validate_command_containment(root, command)
}

pub(super) fn validate_command_security_with_context(
    decision: &GoalDecisionContext<'_>,
    command: &ParsedValidationCommand,
) -> Result<()> {
    validate_test_execution_mode(command)?;
    if !decision.has_captured_workspace_bytes() {
        return validate_command_containment(decision.root(), command);
    }
    if powershell_script(command).is_some()
        && captured_workspace_powershell_key_with_context(decision, command)?.is_none()
    {
        bail!("captured PowerShell validation script is absent from the workspace capture");
    }
    Ok(())
}

/// The spawn-safety half of [`validate_command_security`], without the
/// test-execution-mode rule.
///
/// `goal progress` records explicitly non-authoritative evidence, so demanding
/// that a test command actually execute tests (a receipt-grade rule) rejected
/// legitimate progress commands like a collect-only dry run. It still must not
/// be able to spawn anything the validation path refuses, which is exactly what
/// this half enforces.
pub fn validate_command_containment(root: &Path, command: &ParsedValidationCommand) -> Result<()> {
    let _ = resolve_live_powershell_script(root, command)?;
    Ok(())
}

pub fn validation_list_command(
    command: &ParsedValidationCommand,
) -> Result<Option<ParsedValidationCommand>> {
    validate_test_execution_mode(command)?;
    let mut args = command.args.clone();
    if matches!(cargo_subcommand(command), Some(("test", _))) {
        if let Some(separator) = args.iter().position(|argument| argument == "--") {
            args.truncate(separator);
        }
        args.extend(["--".into(), "--list".into()]);
    } else if pytest_invocation(command) {
        let mut collect_arguments = vec!["--collect-only".into()];
        if !pytest_has_pre_separator_option(command, &["-q", "--quiet"]) {
            collect_arguments.push("-q".into());
        }
        let mut list = ParsedValidationCommand {
            program: command.program.clone(),
            args,
        };
        insert_pytest_args_before_separator(&mut list, collect_arguments)?;
        return Ok(Some(list));
    } else {
        return Ok(None);
    }
    Ok(Some(ParsedValidationCommand {
        program: command.program.clone(),
        args,
    }))
}

/// 从 `--collect-only -q` 的末行取"将要执行的用例数"。
///
/// 两种形式都要正确处理：`N tests collected in Xs`，以及取消选择时的
/// `M/N tests collected (K deselected) in Xs`——后者 M 是选中数、N 是总数，
/// 而运行期 summary 报的是 M。取 N 会让 `passed + ignored == listed` 恒不成立，
/// 于是任何带 `-k` / `-m <marker>` / `--deselect` 的命令都永远写不出 receipt，
/// 尽管 `-k` 正是文档建模并支持的用法。因此正向取第一个数字（即 M）。
pub(super) fn pytest_collected_count(text: &str) -> Option<u64> {
    for line in text.lines().rev() {
        let tokens = line
            .split(|character: char| !character.is_ascii_alphanumeric())
            .filter(|token| !token.is_empty())
            .collect::<Vec<_>>();
        if let Some(index) = tokens.iter().position(|token| *token == "collected")
            && let Some(count) = tokens[..index]
                .iter()
                .find_map(|token| token.parse::<u64>().ok())
        {
            return Some(count);
        }
    }
    None
}

pub fn listed_test_count(
    command: &ParsedValidationCommand,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<u64> {
    if pytest_invocation(command) {
        if pytest_collected_count(&String::from_utf8_lossy(stderr)).is_some() {
            bail!("pytest collect proof 出现在 stderr，来源不可区分");
        }
        let stdout = String::from_utf8_lossy(stdout);
        let count = pytest_collected_count(&stdout).unwrap_or_else(|| {
            stdout
                .lines()
                .filter(|line| line.contains("::") && !line.trim_start().starts_with('='))
                .count() as u64
        });
        if count == 0 {
            bail!("pytest collect proof 没有收集任何测试；不会写入 receipt");
        }
        return Ok(count);
    }

    if !stderr.is_empty() {
        let stderr = String::from_utf8_lossy(stderr);
        if stderr
            .lines()
            .any(|line| line.trim_end().ends_with(": test"))
        {
            bail!("test list proof 出现在 stderr，来源不可区分");
        }
    }
    let count = String::from_utf8_lossy(stdout)
        .lines()
        .filter(|line| line.trim_end().ends_with(": test"))
        .count() as u64;
    if count == 0 {
        bail!("独立 test list proof 没有列出任何测试；不会写入 receipt");
    }
    Ok(count)
}

fn summary_field(line: &str, label: &str) -> Option<u64> {
    let marker = format!(" {label};");
    let end = line.find(&marker)?;
    line[..end]
        .chars()
        .rev()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>()
        .parse()
        .ok()
}

fn pytest_summary_counts(text: &str) -> Option<(u64, u64)> {
    for line in text.lines().rev() {
        let tokens = line
            .split(|character: char| !character.is_ascii_alphanumeric())
            .filter(|token| !token.is_empty())
            .collect::<Vec<_>>();
        if !tokens.contains(&"in") {
            continue;
        }
        let mut passed = 0u64;
        let mut ignored = 0u64;
        for window in tokens.windows(2) {
            let Ok(count) = window[0].parse::<u64>() else {
                continue;
            };
            match window[1] {
                "passed" => passed = passed.saturating_add(count),
                "skipped" | "xfailed" | "xpassed" => {
                    ignored = ignored.saturating_add(count);
                }
                _ => {}
            }
        }
        if passed > 0 || ignored > 0 {
            return Some((passed, ignored));
        }
    }
    None
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TestExecutionProof {
    pub listed: u64,
    pub passed: u64,
    pub ignored: u64,
}

pub fn validation_execution_proof(
    command: &ParsedValidationCommand,
    stdout: &[u8],
    stderr: &[u8],
    listed_tests: Option<u64>,
) -> Result<Option<TestExecutionProof>> {
    validate_test_execution_mode(command)?;
    if git_clean_head_invocation(command) {
        if !stdout.is_empty() || !stderr.is_empty() {
            bail!("git_commit proof requires a clean HEAD: canonical git status produced output");
        }
        return Ok(None);
    }
    if !test_invocation(command) {
        return Ok(None);
    }
    let listed =
        listed_tests.ok_or_else(|| anyhow::anyhow!("test command 缺少独立 list/collect proof"))?;

    if pytest_invocation(command) {
        let stdout = String::from_utf8_lossy(stdout);
        let stderr = String::from_utf8_lossy(stderr);
        if pytest_summary_counts(&stderr).is_some() {
            bail!("pytest summary 出现在 stderr，来源不可区分");
        }
        let (passed, ignored) = pytest_summary_counts(&stdout)
            .ok_or_else(|| anyhow::anyhow!("pytest 成功退出但缺少可验证的终端汇总"))?;
        if passed == 0 {
            bail!("pytest 成功退出但没有可验证的 passed>0 汇总；不会写入 receipt");
        }
        if passed.saturating_add(ignored) != listed {
            bail!(
                "pytest summary 与独立 collect proof 不一致：listed={listed} passed={passed} ignored={ignored}"
            );
        }
        return Ok(Some(TestExecutionProof {
            listed,
            passed,
            ignored,
        }));
    }

    let mut passed = 0u64;
    let mut ignored = 0u64;
    for line in String::from_utf8_lossy(stdout).lines() {
        if !line.starts_with("test result: ok. ") {
            continue;
        }
        passed += summary_field(line, "passed").unwrap_or(0);
        ignored += summary_field(line, "ignored").unwrap_or(0);
    }
    if String::from_utf8_lossy(stderr)
        .lines()
        .any(|line| line.starts_with("test result: ok. "))
    {
        bail!("test summary 出现在 stderr，来源不可区分");
    }
    if passed == 0 {
        bail!("测试命令成功退出但没有可验证的 passed>0 汇总；不会写入 receipt");
    }
    if passed.saturating_add(ignored) != listed {
        bail!(
            "test summary 与独立 list proof 不一致：listed={listed} passed={passed} ignored={ignored}；拒绝混合/伪造输出"
        );
    }
    Ok(Some(TestExecutionProof {
        listed,
        passed,
        ignored,
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValidationExpectation {
    RustBuildOrTest,
    CargoManifestValidation,
    PythonTest,
    /// 未建模生态的下限：命令至少不能是自证无关的探针。
    NonProbeCommand,
}

/// 命令是否自证与任何变更无关——本工具自身、纯 version/help 查询、空操作。
///
/// 未建模生态此前完全 fail-open：一条 `rayman --version` 就能当作 `main.go`
/// 变更的交付证据并让 `check --goal` 报 ready=true。这里不假装能判断任意生态的
/// "这条命令是否真的验证了这个文件"——那需要逐语言建模——但至少要拒掉自证
/// 无关的探针。真实的 `go test ./...`、`make test`、`npm test` 都不受影响。
fn command_is_inert_probe(command: &ParsedValidationCommand) -> bool {
    const INERT_PROGRAMS: &[&str] = &[
        "rayman", "echo", "true", "cd", "pwd", "ver", "hostname", "whoami", "date",
    ];
    const PROBE_FLAGS: &[&str] = &["--version", "-V", "--help", "-h", "version", "help"];

    let executable = executable_name(command);
    if INERT_PROGRAMS.contains(&executable.as_str()) {
        return true;
    }
    // 只要**出现**查询标志就算探针，而不是要求全部参数都是。要求"全部"可以被一个
    // 无意义参数击穿：`git --no-pager --version` 什么都没验证，却因为多了一个
    // `--no-pager` 而不满足"全部是探针标志"，于是能当作 Go 源码变更的交付证据。
    // 真实命令不会把 `--version`/`--help` 混进来——它们要么查询、要么干活。
    command
        .args
        .iter()
        .any(|argument| PROBE_FLAGS.iter().any(|flag| argument == flag))
}

fn validation_expectation_for_path(path: &str) -> Option<ValidationExpectation> {
    let path = path.to_ascii_lowercase();
    if path.ends_with(".rs") {
        return Some(ValidationExpectation::RustBuildOrTest);
    }
    if path.ends_with("cargo.toml") || path.ends_with("cargo.lock") {
        return Some(ValidationExpectation::CargoManifestValidation);
    }
    if path.ends_with(".py") || path.ends_with("pyproject.toml") {
        return Some(ValidationExpectation::PythonTest);
    }
    // 未建模的生态仍有一条下限，不再无条件放行。
    Some(ValidationExpectation::NonProbeCommand)
}

fn validation_expectation_for_policy(
    path: &str,
    policy: ReceiptValidationPolicy,
) -> Option<ValidationExpectation> {
    let expectation = validation_expectation_for_path(path)?;
    // 历史策略下不追加新要求：给旧记录套上后来才有的判定会让本来有效的
    // 历史证明凭空失效。NonProbeCommand 与 PythonTest 都属于后加的。
    if policy == ReceiptValidationPolicy::LegacyV1
        && matches!(
            expectation,
            ValidationExpectation::PythonTest | ValidationExpectation::NonProbeCommand
        )
    {
        None
    } else {
        Some(expectation)
    }
}

fn validation_expectation_label(expectation: ValidationExpectation) -> &'static str {
    match expectation {
        ValidationExpectation::RustBuildOrTest => {
            "Rust build/test validation such as `cargo test`, `cargo clippy`, `cargo check`, or `cargo build`"
        }
        ValidationExpectation::CargoManifestValidation => {
            "Cargo manifest validation such as `cargo test`, `cargo clippy`, `cargo check`, `cargo build`, `cargo deny check`, or `cargo audit`"
        }
        ValidationExpectation::PythonTest => {
            "Python test validation via direct `python -m pytest` or `pytest`"
        }
        ValidationExpectation::NonProbeCommand => {
            "一条真正验证该变更的命令；rayman 自身与 --version/--help 之类的查询不是证据"
        }
    }
}

fn cargo_subcommand(command: &ParsedValidationCommand) -> Option<(&str, Option<&str>)> {
    let executable = Path::new(&command.program)
        .file_name()
        .and_then(|name| name.to_str())?
        .to_ascii_lowercase();
    if executable != "cargo" && executable != "cargo.exe" {
        return None;
    }
    let mut args = command.args.iter().map(String::as_str);
    let mut first = args.next()?;
    if first.starts_with('+') {
        first = args.next()?;
    }
    Some((first, args.next()))
}

fn validation_matches_expectation_with_context(
    decision: &GoalDecisionContext<'_>,
    command: &ParsedValidationCommand,
    expectation: ValidationExpectation,
) -> bool {
    if expectation == ValidationExpectation::NonProbeCommand {
        return !command_is_inert_probe(command);
    }
    if pytest_invocation(command) {
        return matches!(expectation, ValidationExpectation::PythonTest);
    }
    if release_installer_invocation_with_context(decision, command).unwrap_or(false) {
        return matches!(
            expectation,
            ValidationExpectation::RustBuildOrTest | ValidationExpectation::CargoManifestValidation
        );
    }
    if trusted_workspace_gate_script_with_context(decision, command).unwrap_or(false) {
        return true;
    }
    // Execution safety and authority identity intentionally differ. A unique
    // workspace-owned PowerShell file may be a relevant validation command
    // without acquiring the immutable logical-key authority of a reviewed
    // repository gate. Capture-only resolution uses only the fixed bytes.
    if ordinary_workspace_powershell_validation_with_context(decision, command) {
        return true;
    }
    if trusted_xtask_repository_gate_with_context(decision, command).unwrap_or(false) {
        return matches!(
            expectation,
            ValidationExpectation::RustBuildOrTest | ValidationExpectation::CargoManifestValidation
        );
    }
    let rustc_build = executable_name(command) == "rustc"
        && command.args.iter().any(|arg| arg.ends_with(".rs"))
        && !command.args.iter().any(|arg| {
            matches!(
                arg.as_str(),
                "--help" | "-h" | "--version" | "-V" | "--print"
            )
        });
    if rustc_build {
        return matches!(expectation, ValidationExpectation::RustBuildOrTest);
    }
    let Some((subcommand, next)) = cargo_subcommand(command) else {
        return false;
    };
    let rust_build_or_test = matches!(subcommand, "test" | "clippy" | "check" | "build")
        || (subcommand == "nextest" && next == Some("run"));
    match expectation {
        ValidationExpectation::RustBuildOrTest => rust_build_or_test,
        ValidationExpectation::CargoManifestValidation => {
            rust_build_or_test
                || subcommand == "audit"
                || (subcommand == "deny" && next == Some("check"))
        }
        ValidationExpectation::PythonTest => false,
        ValidationExpectation::NonProbeCommand => true,
    }
}

fn validation_matches_expectation(
    root: &Path,
    command: &ParsedValidationCommand,
    expectation: ValidationExpectation,
) -> bool {
    if expectation == ValidationExpectation::NonProbeCommand {
        return !command_is_inert_probe(command);
    }
    if pytest_invocation(command) {
        return matches!(expectation, ValidationExpectation::PythonTest);
    }
    if release_installer_invocation(root, command) {
        return matches!(
            expectation,
            ValidationExpectation::RustBuildOrTest | ValidationExpectation::CargoManifestValidation
        );
    }
    if trusted_workspace_gate_script(root, command) {
        return true;
    }
    if ordinary_workspace_powershell_validation(root, command) {
        return true;
    }
    if trusted_xtask_repository_gate(root, command).unwrap_or(false) {
        return matches!(
            expectation,
            ValidationExpectation::RustBuildOrTest | ValidationExpectation::CargoManifestValidation
        );
    }
    let rustc_build = executable_name(command) == "rustc"
        && command.args.iter().any(|arg| arg.ends_with(".rs"))
        && !command.args.iter().any(|arg| {
            matches!(
                arg.as_str(),
                "--help" | "-h" | "--version" | "-V" | "--print"
            )
        });
    if rustc_build {
        return matches!(expectation, ValidationExpectation::RustBuildOrTest);
    }
    let Some((subcommand, next)) = cargo_subcommand(command) else {
        return false;
    };
    let rust_build_or_test = matches!(subcommand, "test" | "clippy" | "check" | "build")
        || (subcommand == "nextest" && next == Some("run"));
    match expectation {
        ValidationExpectation::RustBuildOrTest => rust_build_or_test,
        ValidationExpectation::CargoManifestValidation => {
            rust_build_or_test
                || subcommand == "audit"
                || (subcommand == "deny" && next == Some("check"))
        }
        ValidationExpectation::PythonTest => false,
        ValidationExpectation::NonProbeCommand => true,
    }
}

pub(super) fn normalized_path_text(path: &str) -> String {
    path.trim_start_matches("./")
        .trim_start_matches(".\\")
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn path_argument_matches(root: &Path, argument: &str, expected: &str) -> bool {
    let argument_path = Path::new(argument);
    let expected_path = Path::new(expected);
    let argument_path = if argument_path.is_absolute() {
        argument_path.to_path_buf()
    } else {
        root.join(argument_path)
    };
    let expected_path = if expected_path.is_absolute() {
        expected_path.to_path_buf()
    } else {
        root.join(expected_path)
    };
    match (argument_path.canonicalize(), expected_path.canonicalize()) {
        (Ok(argument), Ok(expected)) => argument == expected,
        _ => normalized_path_text(argument) == normalized_path_text(expected),
    }
}

fn captured_relative_path(key: &str) -> Option<String> {
    if key.is_empty()
        || key.contains('\\')
        || key.contains(':')
        || key.starts_with('/')
        || key
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return None;
    }
    Some(key.to_string())
}

fn captured_path_argument_matches(
    decision: &GoalDecisionContext<'_>,
    argument: &str,
    expected: &str,
) -> bool {
    let Some(argument) = captured_relative_path(argument) else {
        return false;
    };
    let Some(expected) = captured_relative_path(expected) else {
        return false;
    };
    let Ok(Some(argument)) = decision.captured_workspace_file(&argument) else {
        return false;
    };
    let Ok(Some(expected)) = decision.captured_workspace_file(&expected) else {
        return false;
    };
    argument.key == expected.key
}

fn pytest_selector_covers(root: &Path, selector: &str, expected: &str) -> bool {
    let selector = pytest_selector_path(selector);
    let selector_path = if Path::new(selector).is_absolute() {
        PathBuf::from(selector)
    } else {
        root.join(selector)
    };
    let expected_path = if Path::new(expected).is_absolute() {
        PathBuf::from(expected)
    } else {
        root.join(expected)
    };
    if let (Ok(selector), Ok(expected)) =
        (selector_path.canonicalize(), expected_path.canonicalize())
    {
        return if selector.is_dir() {
            expected.starts_with(selector)
        } else {
            selector == expected
        };
    }
    let selector = normalized_path_text(selector)
        .trim_end_matches('/')
        .to_string();
    let expected = normalized_path_text(expected);
    expected == selector
        || expected
            .strip_prefix(&selector)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn captured_pytest_selector_covers(
    decision: &GoalDecisionContext<'_>,
    selector: &str,
    expected: &str,
) -> bool {
    let Some(selector) = captured_relative_path(pytest_selector_path(selector)) else {
        return false;
    };
    let Some(expected) = captured_relative_path(expected) else {
        return false;
    };
    let Ok(Some(expected)) = decision.captured_workspace_file(&expected) else {
        return false;
    };
    // A complete workspace capture stores regular files, not synthetic
    // directory entries.  Therefore a directory selector such as `tests`
    // must be proven by the captured descendant it covers, rather than by a
    // nonexistent `tests` file.  The strict logical-key grammar above rules
    // out aliases and traversal; this remains entirely capture-only.
    selector == expected.key || expected.key.starts_with(&format!("{selector}/"))
}
fn cargo_option_values<'a>(
    command: &'a ParsedValidationCommand,
    long: &str,
    short: &str,
) -> Vec<&'a str> {
    let mut values = Vec::new();
    let mut index = 0usize;
    while index < command.args.len() {
        let argument = &command.args[index];
        if argument == long || argument == short {
            if let Some(value) = command.args.get(index + 1) {
                values.push(value.as_str());
            }
            index += 2;
            continue;
        }
        if let Some(value) = argument.strip_prefix(&format!("{long}=")) {
            values.push(value);
        } else if short == "-p" && argument.starts_with(short) && argument.len() > short.len() {
            values.push(&argument[short.len()..]);
        }
        index += 1;
    }
    values
}

/// Cargo options that consume the following argument, so their value is never
/// mistaken for a positional test-name filter.
const CARGO_VALUE_OPTIONS: &[&str] = &[
    "--manifest-path",
    "--target",
    "--target-dir",
    "--profile",
    "--config",
    "--color",
    "--message-format",
    "--jobs",
    "-j",
    "--features",
    "-F",
    "--out-dir",
    "--unit-graph",
    "--keep-going",
];

/// libtest options (after `--`) that consume the following argument.
const LIBTEST_VALUE_OPTIONS: &[&str] = &[
    "--test-threads",
    "--logfile",
    "--format",
    "--color",
    "--skip",
    "--shuffle-seed",
];

/// Does this cargo invocation run **less** than the whole selected workspace?
///
/// The contract calls the authority gate "selector-free", but the only check
/// used to be "does `--workspace` or `--all` appear". That let
/// `cargo test --workspace <filter>` — which skips every other test and exits 0
/// while the real suite is red — stand in for the whole suite, and let
/// `cargo build --workspace --exclude <pkg>` claim coverage of a package it
/// never compiled.
///
/// Target-kind selectors (`--lib`, `--tests`, `--all-targets`, ...) are
/// deliberately *not* treated as narrowing: they choose target kinds across the
/// entire workspace, and the repository's own gate runs
/// `cargo test --locked --workspace --all-targets`.
fn cargo_command_is_narrowed(command: &ParsedValidationCommand) -> bool {
    let Some((subcommand, _)) = cargo_subcommand(command) else {
        return false;
    };
    // Skip `+toolchain` and the subcommand itself.
    let mut index = 0;
    if command.args.first().is_some_and(|arg| arg.starts_with('+')) {
        index += 1;
    }
    if command.args.get(index).map(String::as_str) == Some(subcommand) {
        index += 1;
    }
    // Cargo accepts `-pNAME` as well as `-p NAME`. Keep package/exclude
    // detection on the Cargo side of `--`; after it, similarly-spelled values
    // belong to libtest and cannot retroactively select workspace packages.
    let cargo_end = command
        .args
        .iter()
        .position(|argument| argument == "--")
        .unwrap_or(command.args.len());
    let cargo_arguments = &command.args[..cargo_end];
    let compact_package = cargo_arguments.iter().any(|argument| {
        argument
            .strip_prefix("-p")
            .is_some_and(|value| !value.is_empty())
    });
    let compact_features = cargo_arguments.iter().any(|argument| {
        argument
            .strip_prefix("-F")
            .is_some_and(|value| !value.is_empty())
    });
    let named_selector = cargo_arguments.iter().any(|argument| {
        let name = argument.split('=').next().unwrap_or(argument);
        matches!(name, "-p" | "--package" | "--exclude")
    });
    if compact_package || compact_features || named_selector {
        return true;
    }
    let mut after_separator = false;
    while let Some(argument) = command.args.get(index) {
        index += 1;
        if argument == "--" {
            after_separator = true;
            continue;
        }
        let value_options = if after_separator {
            LIBTEST_VALUE_OPTIONS
        } else {
            CARGO_VALUE_OPTIONS
        };
        if argument.starts_with('-') && argument.len() > 1 {
            let name = argument.split('=').next().unwrap_or(argument);
            if !after_separator
                && matches!(
                    name,
                    "-p" | "--package"
                        | "--exclude"
                        | "--lib"
                        | "--bin"
                        | "--bins"
                        | "--test"
                        | "--tests"
                        | "--bench"
                        | "--benches"
                        | "--example"
                        | "--examples"
                        | "--doc"
                        | "--features"
                        | "-F"
                        | "--no-default-features"
                        | "--no-run"
                )
            {
                return true; // selects a subset of the workspace
            }
            if after_separator && matches!(name, "--skip" | "--exact" | "--ignored") {
                return true; // libtest-side filtering
            }
            if !argument.contains('=') && value_options.contains(&name) {
                index += 1; // consume the option value
            }
            continue;
        }
        // A bare positional is a test-name filter on either side of `--`.
        return true;
    }
    false
}

/// pytest options that select a subset of the collected tests.
fn pytest_short_option_is_narrowing(argument: &str) -> bool {
    let Some(flags) = argument
        .strip_prefix('-')
        .filter(|flags| !flags.is_empty() && !flags.starts_with('-'))
    else {
        return false;
    };

    for flag in flags.chars() {
        if matches!(flag, 'k' | 'm') {
            return true;
        }
        // The remainder is a value, not more clustered flags. Keeping this
        // list explicit prevents values such as `-pmark_plugin` or `-rA` from
        // being mistaken for selectors. Unknown/plugin flags are treated as
        // valueless so a later `k`/`m` fails closed.
        if matches!(flag, 'p' | 'r' | 'o' | 'c' | 'n' | 'W') {
            return false;
        }
    }
    false
}

fn pytest_command_is_narrowed(command: &ParsedValidationCommand) -> bool {
    if !pytest_path_arguments(command).is_empty() {
        return true;
    }
    pytest_arguments(command).iter().any(|argument| {
        let name = argument.split('=').next().unwrap_or(argument);
        matches!(
            name,
            "-k" | "-m" | "--deselect" | "--ignore" | "--ignore-glob" | "--lf" | "--last-failed"
        ) || pytest_short_option_is_narrowing(argument)
    })
}

fn command_is_workspace_wide_with_context(
    decision: &GoalDecisionContext<'_>,
    command: &ParsedValidationCommand,
) -> bool {
    release_installer_invocation_with_context(decision, command).unwrap_or(false)
        || trusted_workspace_gate_script_with_context(decision, command).unwrap_or(false)
        || trusted_xtask_repository_gate_with_context(decision, command).unwrap_or(false)
        || (cargo_subcommand(command).is_some()
            && command
                .args
                .iter()
                .any(|argument| matches!(argument.as_str(), "--workspace" | "--all"))
            && !cargo_command_is_narrowed(command))
        || (pytest_invocation(command) && !pytest_command_is_narrowed(command))
}

pub(super) fn command_is_workspace_wide(root: &Path, command: &ParsedValidationCommand) -> bool {
    release_installer_invocation(root, command)
        || trusted_workspace_gate_script(root, command)
        || trusted_xtask_repository_gate(root, command).unwrap_or(false)
        || (cargo_subcommand(command).is_some()
            && command
                .args
                .iter()
                .any(|argument| matches!(argument.as_str(), "--workspace" | "--all"))
            && !cargo_command_is_narrowed(command))
        || (pytest_invocation(command) && !pytest_command_is_narrowed(command))
}

/// Authority receipts are stronger than ordinary validation receipts: the
/// command must be an explicit repository gate, not merely a locally relevant
/// build. Unknown ecosystems can opt in by exposing a reviewed workspace-local
/// gate at one of the conventional script paths below.
pub fn validate_authority_command(root: &Path, command: &str) -> Result<()> {
    let parsed = parse_validation_command(command)?;
    validate_command_security(root, &parsed)?;
    validate_authority_command_syntax_with_gate(
        &parsed,
        trusted_workspace_gate_script(root, &parsed)
            || trusted_xtask_repository_gate(root, &parsed)?,
    )
}

pub(super) fn validate_authority_command_syntax_with_gate(
    parsed: &ParsedValidationCommand,
    trusted_script: bool,
) -> Result<()> {
    // "Selector-free" is part of the contract, not decoration: a filtered run
    // exits 0 while the rest of the suite is red.
    let workspace_cargo_test = matches!(cargo_subcommand(parsed), Some(("test", _)))
        && parsed
            .args
            .iter()
            .any(|argument| matches!(argument.as_str(), "--workspace" | "--all"))
        && !cargo_command_is_narrowed(parsed);
    let workspace_pytest = pytest_invocation(parsed) && !pytest_command_is_narrowed(parsed);

    if !trusted_script && !workspace_cargo_test && !workspace_pytest {
        bail!(
            "authority gate 必须是完整参数集的受检 check-repo/audit-repository/verify-release-contract 脚本、精确 `cargo run --locked --manifest-path xtask/Cargo.toml -- repository-gate`、无 target/feature 缩窄选择器的 `cargo test --workspace|--all`，或无路径选择器的全工作区 pytest"
        );
    }
    Ok(())
}

pub(crate) fn validate_authority_command_with_context(
    decision: &GoalDecisionContext<'_>,
    command: &str,
) -> Result<()> {
    let parsed = parse_validation_command(command)?;
    validate_command_security_with_context(decision, &parsed)?;
    validate_authority_command_syntax_with_gate(
        &parsed,
        trusted_workspace_gate_script_with_context(decision, &parsed)?
            || trusted_xtask_repository_gate_with_context(decision, &parsed)?,
    )
}

pub(super) fn validation_matches_impact_with_context(
    decision: &GoalDecisionContext<'_>,
    command: &ParsedValidationCommand,
    impact: &ImpactEvidence,
) -> bool {
    let root = decision.root();
    let Some(expectation) = validation_expectation_for_path(&impact.changed_path) else {
        return true;
    };
    let matches_expectation = if decision.has_captured_workspace_bytes() {
        validation_matches_expectation_with_context(decision, command, expectation)
    } else {
        validation_matches_expectation(root, command, expectation)
    };
    if !matches_expectation {
        return false;
    }
    // 未建模生态只做"非探针"这一条下限判断，不再往下走 Rust/pytest/cargo 的
    // 作用域匹配——那些规则对未知生态没有意义，硬套只会误拒。
    if expectation == ValidationExpectation::NonProbeCommand {
        return true;
    }
    let workspace_wide = if decision.has_captured_workspace_bytes() {
        command_is_workspace_wide_with_context(decision, command)
    } else {
        command_is_workspace_wide(root, command)
    };
    if workspace_wide {
        // "Workspace-wide" is a Cargo claim, and Cargo never builds a package
        // the root manifest excludes. Treating `--workspace` (or the installer,
        // which is a `cargo build -p` wrapper) as covering an excluded package
        // accepted a receipt for code the command provably never compiled.
        let cargo_driven = cargo_subcommand(command).is_some()
            || if decision.has_captured_workspace_bytes() {
                release_installer_invocation_with_context(decision, command).unwrap_or(false)
            } else {
                release_installer_invocation(root, command)
            };
        if cargo_driven && decision.path_is_excluded_from_root_cargo_workspace(&impact.changed_path)
        {
            return false;
        }
        return true;
    }
    if decision.has_captured_workspace_bytes() {
        if ordinary_workspace_powershell_validation_with_context(decision, command) {
            return true;
        }
    } else if ordinary_workspace_powershell_validation(root, command) {
        return true;
    }
    if executable_name(command) == "rustc" {
        return impact.changed_path.to_ascii_lowercase().ends_with(".rs")
            && command.args.iter().any(|argument| {
                argument.to_ascii_lowercase().ends_with(".rs")
                    && if decision.has_captured_workspace_bytes() {
                        captured_path_argument_matches(decision, argument, &impact.changed_path)
                    } else {
                        path_argument_matches(root, argument, &impact.changed_path)
                    }
            });
    }
    if pytest_invocation(command) {
        let paths = pytest_path_arguments(command);
        return paths.iter().any(|argument| {
            (if decision.has_captured_workspace_bytes() {
                captured_pytest_selector_covers(decision, argument, &impact.changed_path)
            } else {
                pytest_selector_covers(root, argument, &impact.changed_path)
            }) || impact.candidate_tests.iter().any(|test| {
                if decision.has_captured_workspace_bytes() {
                    captured_pytest_selector_covers(decision, argument, test)
                } else {
                    pytest_selector_covers(root, argument, test)
                }
            })
        });
    }
    if cargo_subcommand(command).is_some() {
        if impact.package.as_deref().is_some_and(|package| {
            cargo_option_values(command, "--package", "-p").contains(&package)
        }) {
            return true;
        }
        if impact.manifest_path.as_deref().is_some_and(|manifest| {
            cargo_option_values(command, "--manifest-path", "")
                .iter()
                .any(|value| {
                    if decision.has_captured_workspace_bytes() {
                        captured_path_argument_matches(decision, value, manifest)
                    } else {
                        path_argument_matches(root, value, manifest)
                    }
                })
        }) {
            return true;
        }
        let has_explicit_scope = !cargo_option_values(command, "--package", "-p").is_empty()
            || !cargo_option_values(command, "--manifest-path", "").is_empty();
        if !has_explicit_scope
            && impact
                .manifest_path
                .as_deref()
                .is_some_and(|manifest| normalized_path_text(manifest) == "cargo.toml")
        {
            // Validation commands run with `current_dir(root)`. A bare Cargo
            // command therefore targets the root package manifest, but it must
            // not be generalized to nested/workspace packages without -p,
            // --manifest-path, --workspace, or --all evidence.
            return true;
        }
        return false;
    }
    false
}

fn validation_command_is_code(command: &ParsedValidationCommand) -> bool {
    cargo_subcommand(command).is_some()
        || executable_name(command) == "rustc"
            && command
                .args
                .iter()
                .any(|argument| argument.to_ascii_lowercase().ends_with(".rs"))
        || powershell_script(command).is_some()
        || pytest_invocation(command)
}

fn validate_scope_declaration(
    command: &ParsedValidationCommand,
    impacts: &[ImpactEvidence],
    non_code: bool,
    workspace_snapshot: bool,
) -> Result<()> {
    if workspace_snapshot && (!impacts.is_empty() || non_code) {
        bail!("`--workspace-snapshot` 不能与 `--changed` 或 `--non-code` 同时使用");
    }
    if impacts.is_empty() && !non_code && !workspace_snapshot {
        bail!(
            "validation 必须提供至少一个 `--changed`；非代码需求必须显式使用 `--non-code`；零变更 authority 审计使用 `--workspace-snapshot`"
        );
    }
    if !impacts.is_empty() && non_code {
        bail!("`--non-code` 不能与 `--changed` 同时使用");
    }
    if non_code && validation_command_is_code(command) {
        bail!("代码构建/测试命令不能声明为 `--non-code`；必须绑定实际 `--changed` scope");
    }
    Ok(())
}

pub fn validate_command_for_impacts(
    root: &Path,
    command: &str,
    impacts: &[ImpactEvidence],
    non_code: bool,
) -> Result<()> {
    validate_command_for_scope(root, command, impacts, non_code, false)
}

pub fn validate_command_for_scope(
    root: &Path,
    command: &str,
    impacts: &[ImpactEvidence],
    non_code: bool,
    workspace_snapshot: bool,
) -> Result<()> {
    let decision = GoalDecisionContext::live(root, None);
    validate_command_for_scope_with_context(
        &decision,
        command,
        impacts,
        non_code,
        workspace_snapshot,
    )
}

pub(crate) fn validate_command_for_scope_with_context(
    decision: &GoalDecisionContext<'_>,
    command: &str,
    impacts: &[ImpactEvidence],
    non_code: bool,
    workspace_snapshot: bool,
) -> Result<()> {
    let parsed = parse_validation_command(command)?;
    validate_command_security_with_context(decision, &parsed)?;
    validate_scope_declaration(&parsed, impacts, non_code, workspace_snapshot)?;
    for impact in impacts {
        let Some(expectation) = validation_expectation_for_path(&impact.changed_path) else {
            continue;
        };
        if !validation_matches_impact_with_context(decision, &parsed, impact) {
            bail!(
                "验证命令不覆盖 {}；需要 {}",
                impact.changed_path,
                validation_expectation_label(expectation)
            );
        }
    }
    Ok(())
}

pub fn validation_relevance_gaps(
    requirement: &Requirement,
    goal: &Goal,
    root: &Path,
    current_fingerprint: &str,
) -> Vec<String> {
    let decision = GoalDecisionContext::live(root, None);
    validation_relevance_gaps_with_context(requirement, goal, &decision, current_fingerprint)
}

pub(crate) fn validation_relevance_gaps_with_context(
    requirement: &Requirement,
    goal: &Goal,
    decision: &GoalDecisionContext<'_>,
    current_fingerprint: &str,
) -> Vec<String> {
    validation_relevance_gaps_for_fingerprint_with_context(
        requirement,
        goal,
        decision,
        current_fingerprint,
        true,
        ReceiptValidationPolicy::CurrentV3,
        false,
    )
}

fn validation_relevance_gaps_for_fingerprint(
    requirement: &Requirement,
    goal: &Goal,
    root: &Path,
    fingerprint: &str,
    enforce_current_security: bool,
    policy: ReceiptValidationPolicy,
    require_plan_contained: bool,
) -> Vec<String> {
    let decision = GoalDecisionContext::live(root, None);
    validation_relevance_gaps_for_fingerprint_with_context(
        requirement,
        goal,
        &decision,
        fingerprint,
        enforce_current_security,
        policy,
        require_plan_contained,
    )
}

fn validation_relevance_gaps_for_fingerprint_with_context(
    requirement: &Requirement,
    goal: &Goal,
    decision: &GoalDecisionContext<'_>,
    fingerprint: &str,
    enforce_current_security: bool,
    policy: ReceiptValidationPolicy,
    require_plan_contained: bool,
) -> Vec<String> {
    let Ok(contract_sha256) = validation_contract_sha256(goal, &requirement.id) else {
        return vec!["validation contract 无法计算".into()];
    };
    let mut gaps = Vec::new();
    for impact in &requirement.impacts {
        let Some(expectation) = validation_expectation_for_policy(&impact.changed_path, policy)
        else {
            continue;
        };
        let covered = requirement.validations.iter().any(|validation| {
            (!require_plan_contained
                || validation_is_plan_contained_for_historical_legacy_success(goal, validation))
                && validation_has_receipt_for_fingerprint_with_context(
                    validation,
                    decision,
                    fingerprint,
                    &contract_sha256,
                    enforce_current_security,
                    policy,
                )
                && validation.impact_scopes.iter().any(|scope| {
                    scope.changed_path.replace('\\', "/") == impact.changed_path.replace('\\', "/")
                        && scope.package == impact.package
                        && scope.manifest_path.as_deref().map(normalized_path_text)
                            == impact.manifest_path.as_deref().map(normalized_path_text)
                })
                && parse_validation_command(&validation.command).is_ok_and(|parsed| {
                    validation_matches_impact_with_context(decision, &parsed, impact)
                })
        });
        if !covered {
            gaps.push(format!(
                "validation 不覆盖 {}；需要同一条当前成功 receipt 绑定 {}",
                impact.changed_path,
                validation_expectation_label(expectation)
            ));
        }
    }
    gaps
}

pub fn goal_success_receipt_gaps(goal: &Goal, root: &Path, fingerprint: &str) -> Vec<String> {
    goal_success_receipt_gaps_for_fingerprint(goal, root, fingerprint, true)
}

fn goal_success_receipt_gaps_for_fingerprint(
    goal: &Goal,
    root: &Path,
    fingerprint: &str,
    enforce_current_security: bool,
) -> Vec<String> {
    goal_success_receipt_gaps_for_policy(
        goal,
        root,
        fingerprint,
        enforce_current_security,
        ReceiptValidationPolicy::CurrentV3,
    )
}

pub(super) fn goal_success_receipt_gaps_for_policy(
    goal: &Goal,
    root: &Path,
    fingerprint: &str,
    enforce_current_security: bool,
    policy: ReceiptValidationPolicy,
) -> Vec<String> {
    let planning_policy = if enforce_current_security {
        GoalPlanningValidationPolicy::Current
    } else if goal.lifecycle == GoalLifecycle::Archived && goal.plan_publication_policy.is_none() {
        GoalPlanningValidationPolicy::HistoricalLegacySuccess
    } else {
        GoalPlanningValidationPolicy::Skip
    };
    goal_success_receipt_gaps_with_policy(
        goal,
        root,
        fingerprint,
        enforce_current_security,
        planning_policy,
        policy,
    )
}

pub(super) fn goal_success_receipt_gaps_for_retiring_legacy_success(
    goal: &Goal,
    root: &Path,
    fingerprint: &str,
) -> Vec<String> {
    goal_success_receipt_gaps_with_policy(
        goal,
        root,
        fingerprint,
        true,
        GoalPlanningValidationPolicy::RetiringLegacySuccess,
        ReceiptValidationPolicy::CurrentV3,
    )
}

pub(super) fn goal_success_receipt_gaps_for_historical_legacy_success(
    goal: &Goal,
    root: &Path,
    fingerprint: &str,
    policy: ReceiptValidationPolicy,
) -> Vec<String> {
    goal_success_receipt_gaps_with_policy(
        goal,
        root,
        fingerprint,
        false,
        GoalPlanningValidationPolicy::HistoricalLegacySuccess,
        policy,
    )
}

pub(super) fn goal_retiring_legacy_success_unreceipted_migration_gaps(
    goal: &Goal,
    root: &Path,
    fingerprint: &str,
) -> Vec<String> {
    goal_plan_governance_gaps_for_retiring_legacy_success(goal, root, fingerprint)
}

pub(super) fn goal_retiring_legacy_success_v1_migration_gaps(
    goal: &Goal,
    root: &Path,
    fingerprint: &str,
) -> Vec<String> {
    let mut gaps = goal_v1_governance_gaps_for_retiring_legacy_success(goal, root, fingerprint);
    // Migration needs one complete *safe* LegacyV1 proof set, not a promise
    // that every historical receipt sharing the fingerprint remains usable.
    // Unsafe extras contribute no must/relevance/delta coverage, but they also
    // must not poison an otherwise complete safe set.
    gaps.extend(goal_success_receipt_gaps_with_policy(
        goal,
        root,
        fingerprint,
        true,
        GoalPlanningValidationPolicy::Skip,
        ReceiptValidationPolicy::LegacyV1,
    ));
    gaps
}

fn goal_success_receipt_gaps_with_policy(
    goal: &Goal,
    root: &Path,
    fingerprint: &str,
    enforce_current_security: bool,
    planning_policy: GoalPlanningValidationPolicy,
    policy: ReceiptValidationPolicy,
) -> Vec<String> {
    let mut gaps = Vec::new();
    if goal.status != GoalStatus::Success {
        gaps.push(format!("goal 状态为 {}，不是 success", goal.status));
    }
    if goal.replacement_authority.is_some() {
        if policy != ReceiptValidationPolicy::CurrentV3 {
            gaps.push("lifecycle-only replacement 只接受 current-v3 receipt policy".into());
        } else if let Some(error) = replacement_authority_error(goal, root, fingerprint) {
            gaps.push(format!("lifecycle-only replacement proof 无效: {error}"));
        }
        return gaps;
    }
    let require_plan_contained =
        planning_policy == GoalPlanningValidationPolicy::HistoricalLegacySuccess;
    for requirement in &goal.requirements {
        if requirement.kind != RequirementKind::Must {
            // 读门禁（goal_gate_verdict）对所有需求种类检查 Done-无-receipt 与
            // impact 相关性；非 Must 允许保持未完成，但一旦标记 Done，写路径
            // 必须执行与读门禁相同的检查，否则 close/archive 会持久化一个
            // 读门禁立刻拒绝的 success。
            if requirement.status == RequirementStatus::Done {
                if requirement
                    .evidence
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or_default()
                    .is_empty()
                {
                    gaps.push(format!("需求 {} 缺少 evidence 文本", requirement.id));
                }
                if requirement.validations.is_empty() {
                    gaps.push(format!("需求 {} 缺少验证 receipt", requirement.id));
                }
            }
            for gap in validation_relevance_gaps_for_fingerprint(
                requirement,
                goal,
                root,
                fingerprint,
                enforce_current_security,
                policy,
                require_plan_contained,
            ) {
                gaps.push(format!("需求 {} {gap}", requirement.id));
            }
            continue;
        }
        if requirement.status != RequirementStatus::Done
            || requirement
                .evidence
                .as_deref()
                .map(str::trim)
                .unwrap_or_default()
                .is_empty()
        {
            gaps.push(format!("must {} 未完成或缺少 evidence", requirement.id));
            continue;
        }
        let Ok(contract_sha256) = validation_contract_sha256(goal, &requirement.id) else {
            gaps.push(format!(
                "must {} immutable contract 无法计算",
                requirement.id
            ));
            continue;
        };
        // The typed proof kind must be enforced here too, not only in the
        // readiness gate: this is the predicate `close --status success` uses,
        // and a write path weaker than the read path persists — and reports —
        // a success the tool's own validator rejects.
        if !requirement.validations.iter().any(|validation| {
            (!require_plan_contained
                || validation_is_plan_contained_for_historical_legacy_success(goal, validation))
                && proof_kind_matches(
                    requirement.proof_kind,
                    validation_proof_kind(root, &validation.command)
                        .ok()
                        .unwrap_or_default(),
                )
                && validation_has_receipt_for_fingerprint(
                    validation,
                    root,
                    fingerprint,
                    &contract_sha256,
                    enforce_current_security,
                    policy,
                )
        }) {
            gaps.push(format!(
                "must {} 缺少当前成功 validation receipt",
                requirement.id
            ));
        }
        gaps.extend(validation_relevance_gaps_for_fingerprint(
            requirement,
            goal,
            root,
            fingerprint,
            enforce_current_security,
            policy,
            require_plan_contained,
        ));
    }
    match planning_policy {
        GoalPlanningValidationPolicy::Skip => {}
        GoalPlanningValidationPolicy::Current => {
            gaps.extend(goal_planning_gaps(goal, root, fingerprint));
        }
        GoalPlanningValidationPolicy::RetiringLegacySuccess => {
            gaps.extend(goal_planning_gaps_for_retiring_legacy_success(
                goal,
                root,
                fingerprint,
            ));
        }
        GoalPlanningValidationPolicy::HistoricalLegacySuccess => {
            gaps.extend(goal_historical_planning_gaps_for_legacy_success(
                goal,
                root,
                fingerprint,
                policy,
            ));
        }
    }
    gaps
}

/// Captured counterpart used only by the readiness decision.  Its public
/// write/history helpers remain live because they intentionally verify the
/// workspace at the transaction boundary; this version must never reopen it.
pub(super) fn goal_success_receipt_gaps_with_context(
    goal: &Goal,
    decision: &GoalDecisionContext<'_>,
    all_goals: &[Goal],
    policy: ReceiptValidationPolicy,
) -> Vec<String> {
    let Some(current) = decision.current() else {
        return vec!["current captured workspace snapshot 缺失".into()];
    };
    let fingerprint = &current.workspace_fingerprint;
    let mut gaps = Vec::new();
    if goal.status != GoalStatus::Success {
        gaps.push(format!("goal 状态为 {}，不是 success", goal.status));
    }
    if goal.replacement_authority.is_some() {
        if policy != ReceiptValidationPolicy::CurrentV3 {
            gaps.push("lifecycle-only replacement 只接受 current-v3 receipt policy".into());
        } else if let Some(error) =
            replacement_authority_error_with_context(goal, decision, all_goals)
        {
            gaps.push(format!("lifecycle-only replacement proof 无效: {error}"));
        }
        return gaps;
    }
    for requirement in &goal.requirements {
        if requirement.kind != RequirementKind::Must {
            continue;
        }
        if requirement.status != RequirementStatus::Done
            || requirement
                .evidence
                .as_deref()
                .map(str::trim)
                .unwrap_or_default()
                .is_empty()
        {
            gaps.push(format!("must {} 未完成或缺少 evidence", requirement.id));
            continue;
        }
        let Ok(contract_sha256) = validation_contract_sha256(goal, &requirement.id) else {
            gaps.push(format!(
                "must {} immutable contract 无法计算",
                requirement.id
            ));
            continue;
        };
        if !requirement.validations.iter().any(|validation| {
            proof_kind_matches(
                requirement.proof_kind,
                validation_proof_kind_with_context(decision, &validation.command)
                    .ok()
                    .unwrap_or_default(),
            ) && validation_has_receipt_for_fingerprint_with_context(
                validation,
                decision,
                fingerprint,
                &contract_sha256,
                policy == ReceiptValidationPolicy::CurrentV3,
                policy,
            )
        }) {
            gaps.push(format!(
                "must {} 缺少当前成功 validation receipt",
                requirement.id
            ));
        }
    }
    gaps
}
