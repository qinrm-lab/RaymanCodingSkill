use super::*;

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
        bail!("验证命令不能启动 shell；请直接提供要执行的程序及参数");
    }

    Ok(ParsedValidationCommand {
        program,
        args: words,
    })
}

fn executable_name(command: &ParsedValidationCommand) -> String {
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

fn powershell_script(command: &ParsedValidationCommand) -> Option<&str> {
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

fn normalized_script_name(command: &ParsedValidationCommand) -> Option<String> {
    let path = powershell_script(command)?;
    Some(
        Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(path)
            .to_ascii_lowercase(),
    )
}

fn release_installer_invocation(command: &ParsedValidationCommand) -> bool {
    normalized_script_name(command).as_deref() == Some("install-rayman.ps1")
        && !command
            .args
            .iter()
            .any(|argument| argument.eq_ignore_ascii_case("-SelfTest"))
}

pub fn validation_proof_kind(command: &str) -> Result<ProofKind> {
    let parsed = parse_validation_command(command)?;
    let executable = executable_name(&parsed);
    let script = normalized_script_name(&parsed).unwrap_or_default();
    let has_arg = |needle: &str| {
        parsed
            .args
            .iter()
            .any(|argument| argument.eq_ignore_ascii_case(needle))
    };

    if script == "verify-release-contract.ps1" && has_arg("-RequireSourceFresh") {
        return Ok(ProofKind::SourceFresh);
    }
    if matches!(
        script.as_str(),
        "check-repo.ps1" | "audit-repository.ps1" | "release-closeout.ps1"
    ) {
        return Ok(ProofKind::RepositoryGate);
    }
    if script == "install-rayman.ps1" {
        return Ok(if release_installer_invocation(&parsed) {
            ProofKind::Installation
        } else {
            ProofKind::Test
        });
    }
    if script == "check-agent-instructions.ps1"
        || executable == "markdownlint"
        || parsed.args.iter().any(|argument| {
            argument
                .replace('\\', "/")
                .to_ascii_lowercase()
                .ends_with("quick_validate.py")
        })
    {
        return Ok(ProofKind::Documentation);
    }
    if executable == "git"
        && parsed
            .args
            .first()
            .is_some_and(|argument| argument == "rev-parse")
        && parsed.args.iter().any(|argument| argument == "--verify")
    {
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
    const NON_EXECUTING: &[&str] = &[
        "--no-run",
        "--list",
        "--collect-only",
        "--help",
        "-h",
        "--version",
        "-V",
    ];
    if let Some(flag) = command
        .args
        .iter()
        .find(|arg| NON_EXECUTING.iter().any(|candidate| arg == candidate))
    {
        bail!("测试验证命令包含非执行模式 {flag}；receipt 必须实际运行至少一个测试");
    }
    if matches!(cargo_subcommand(command), Some(("nextest", _))) {
        bail!("nextest 暂无独立 list proof 支持；请使用 `cargo test` 生成 receipt");
    }
    Ok(())
}

pub fn validate_command_security(root: &Path, command: &ParsedValidationCommand) -> Result<()> {
    validate_test_execution_mode(command)?;
    let Some(script) = powershell_script(command) else {
        return Ok(());
    };
    let lexical_script = {
        let path = Path::new(script);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        }
    };
    let metadata = fs::symlink_metadata(&lexical_script).map_err(|error| {
        anyhow::anyhow!(
            "无法读取 PowerShell 验证脚本 {}: {error}",
            lexical_script.display()
        )
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        bail!("PowerShell 验证脚本必须是工作区内的普通 .ps1 文件");
    }
    let canonical_root = root.canonicalize()?;
    let canonical_script = lexical_script.canonicalize()?;
    if canonical_script.extension().and_then(|ext| ext.to_str()) != Some("ps1")
        || !canonical_script.starts_with(&canonical_root)
    {
        bail!("PowerShell 验证脚本必须是工作区内的普通 .ps1 文件");
    }
    let visible = crate::walk::workspace_files_checked(root)?
        .into_iter()
        .any(|path| {
            path.canonicalize()
                .is_ok_and(|candidate| candidate == canonical_script)
        });
    if !visible {
        bail!("PowerShell 验证脚本不在当前工作区的受检文件集合中");
    }
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
        args.push("--collect-only".into());
        if !args
            .iter()
            .any(|argument| argument == "-q" || argument == "--quiet")
        {
            args.push("-q".into());
        }
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

pub fn validation_scopes_for_impacts(impacts: &[ImpactEvidence]) -> Vec<ValidationImpactScope> {
    let mut scopes = impacts
        .iter()
        .map(|impact| ValidationImpactScope {
            changed_path: impact.changed_path.replace('\\', "/"),
            package: impact.package.clone(),
            manifest_path: impact
                .manifest_path
                .as_ref()
                .map(|path| path.replace('\\', "/")),
        })
        .collect::<Vec<_>>();
    scopes.sort();
    scopes.dedup();
    scopes
}

pub fn validation_invocation_sha256(command: &str, impact_paths: &[String]) -> String {
    let scopes = impact_paths
        .iter()
        .map(|path| ValidationImpactScope {
            changed_path: path.replace('\\', "/"),
            package: None,
            manifest_path: None,
        })
        .collect::<Vec<_>>();
    validation_invocation_sha256_scoped(command, &scopes, impact_paths.is_empty())
}

pub fn validation_invocation_sha256_scoped(
    command: &str,
    impact_scopes: &[ValidationImpactScope],
    non_code: bool,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(command.as_bytes());
    hasher.update([0]);
    hasher.update([u8::from(non_code)]);
    let mut scopes = impact_scopes.to_vec();
    scopes.sort();
    scopes.dedup();
    for scope in scopes {
        hasher.update(scope.changed_path.replace('\\', "/").as_bytes());
        hasher.update([0]);
        hasher.update(scope.package.as_deref().unwrap_or_default().as_bytes());
        hasher.update([0]);
        hasher.update(
            scope
                .manifest_path
                .as_deref()
                .unwrap_or_default()
                .replace('\\', "/")
                .as_bytes(),
        );
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

pub fn authority_invocation_sha256(
    command: &str,
    requirement_id: &str,
    repeat: u32,
    impact_scopes: &[ValidationImpactScope],
    non_code: bool,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"rayman.authority-validation.v1");
    hasher.update(validation_invocation_sha256_scoped(
        command,
        impact_scopes,
        non_code,
    ));
    hasher.update([0]);
    hasher.update(requirement_id.as_bytes());
    hasher.update([0]);
    hasher.update(repeat.to_le_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn has_current_stable_authority_receipt(
    goal: &Goal,
    root: &Path,
    current_fingerprint: &str,
) -> bool {
    has_direct_stable_authority_receipt(goal, root, current_fingerprint)
        || (goal.replacement_authority.is_some()
            && replacement_authority_error(goal, root, current_fingerprint).is_none())
}

pub(super) fn has_direct_stable_authority_receipt(
    goal: &Goal,
    root: &Path,
    current_fingerprint: &str,
) -> bool {
    goal.authority_receipts.iter().any(|authority| {
        direct_stable_authority_receipt_is_valid(goal, root, current_fingerprint, authority)
    })
}

pub(super) fn has_direct_stable_authority_command(
    goal: &Goal,
    root: &Path,
    fingerprint: &str,
    command: &str,
) -> bool {
    goal.authority_receipts.iter().any(|authority| {
        authority.command == command
            && direct_stable_authority_receipt_is_valid(goal, root, fingerprint, authority)
    })
}

pub(super) fn direct_stable_authority_receipt_is_valid(
    goal: &Goal,
    root: &Path,
    fingerprint: &str,
    authority: &AuthorityReceipt,
) -> bool {
    let Ok(contract_sha256) = validation_contract_sha256(goal, &authority.requirement_id) else {
        return false;
    };
    authority.repeat >= 2
        && authority.runs.len() == authority.repeat as usize
        && authority.workspace_fingerprint == fingerprint
        && authority.contract_sha256 == contract_sha256
        && authority.invocation_sha256
            == authority_invocation_sha256(
                &authority.command,
                &authority.requirement_id,
                authority.repeat,
                &authority.impact_scopes,
                authority.non_code,
            )
        && validate_authority_command(root, &authority.command).is_ok()
        && authority.runs.iter().all(|run| {
            run.exit_code == 0
                && run.workspace_fingerprint_before == fingerprint
                && run.workspace_fingerprint_after == fingerprint
                && is_sha256(&run.stdout_sha256)
                && is_sha256(&run.stderr_sha256)
        })
}

#[derive(Serialize)]
struct ImmutableRequirementContract<'a> {
    id: &'a str,
    text: &'a str,
    kind: RequirementKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    proof_kind: Option<ProofKind>,
}

#[derive(Serialize)]
struct ImmutableGoalContract<'a> {
    goal_id: &'a str,
    title: &'a str,
    created_at: &'a str,
    target_requirement_id: &'a str,
    requirements: Vec<ImmutableRequirementContract<'a>>,
}

#[derive(Serialize)]
struct ImmutableGoalIdentityContract<'a> {
    goal_id: &'a str,
    title: &'a str,
    created_at: &'a str,
    requirements: Vec<ImmutableRequirementContract<'a>>,
}

fn immutable_requirements(goal: &Goal) -> Vec<ImmutableRequirementContract<'_>> {
    goal.requirements
        .iter()
        .map(|requirement| ImmutableRequirementContract {
            id: &requirement.id,
            text: &requirement.text,
            kind: requirement.kind,
            proof_kind: requirement.proof_kind,
        })
        .collect()
}

pub fn goal_contract_sha256(goal: &Goal) -> Result<String> {
    let contract = ImmutableGoalIdentityContract {
        goal_id: &goal.id,
        title: &goal.title,
        created_at: &goal.created_at,
        requirements: immutable_requirements(goal),
    };
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(&contract)?);
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn authority_receipt_sha256(receipt: &AuthorityReceipt) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(b"rayman.authority-receipt.v1");
    hasher.update(serde_json::to_vec(receipt)?);
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn validation_contract_sha256(goal: &Goal, requirement_id: &str) -> Result<String> {
    if !goal
        .requirements
        .iter()
        .any(|requirement| requirement.id == requirement_id)
    {
        bail!("需求不存在: {requirement_id}");
    }
    let contract = ImmutableGoalContract {
        goal_id: &goal.id,
        title: &goal.title,
        created_at: &goal.created_at,
        target_requirement_id: requirement_id,
        requirements: immutable_requirements(goal),
    };
    let bytes = serde_json::to_vec(&contract)?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

pub(super) fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn same_canonical_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn test_receipt_has_structured_proof(receipt: &ValidationReceipt) -> bool {
    matches!(
        (receipt.listed_tests, receipt.passed_tests, receipt.ignored_tests),
        (Some(listed), Some(passed), Some(ignored))
            if listed > 0 && passed > 0 && passed.saturating_add(ignored) == listed
    ) && receipt.list_stdout_sha256.as_deref().is_some_and(is_sha256)
        && receipt.list_stderr_sha256.as_deref().is_some_and(is_sha256)
}

fn validation_scope_is_well_formed(validation: &ValidationEvidence) -> bool {
    let mut paths = validation
        .impact_paths
        .iter()
        .map(|path| path.replace('\\', "/"))
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    let scope_paths = validation
        .impact_scopes
        .iter()
        .map(|scope| scope.changed_path.replace('\\', "/"))
        .collect::<Vec<_>>();
    paths == scope_paths
        && ((validation.non_code && validation.impact_scopes.is_empty())
            || (!validation.non_code && !validation.impact_scopes.is_empty()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReceiptValidationPolicy {
    /// Policy used before Python/pytest receipts gained mandatory collect proof
    /// and Python impact relevance checks.
    LegacyV1,
    /// Current receipt integrity, test-proof, and relevance policy.
    CurrentV2,
}

pub fn validation_has_current_receipt(
    validation: &ValidationEvidence,
    goal: &Goal,
    requirement: &Requirement,
    root: &Path,
    current_fingerprint: &str,
) -> bool {
    if !proof_kind_matches(
        requirement.proof_kind,
        validation_proof_kind(&validation.command)
            .ok()
            .unwrap_or_default(),
    ) {
        return false;
    }
    let Ok(contract_sha256) = validation_contract_sha256(goal, &requirement.id) else {
        return false;
    };
    validation_has_receipt_for_fingerprint(
        validation,
        root,
        current_fingerprint,
        &contract_sha256,
        true,
        ReceiptValidationPolicy::CurrentV2,
    )
}

pub(super) fn validation_has_receipt_for_fingerprint(
    validation: &ValidationEvidence,
    root: &Path,
    fingerprint: &str,
    contract_sha256: &str,
    enforce_current_security: bool,
    policy: ReceiptValidationPolicy,
) -> bool {
    let Ok(parsed) = parse_validation_command(&validation.command) else {
        return false;
    };
    if enforce_current_security && validate_command_security(root, &parsed).is_err() {
        return false;
    }
    let Some(receipt) = validation.receipt.as_ref() else {
        return false;
    };
    receipt.exit_code == 0
        && receipt.workspace_fingerprint_before == fingerprint
        && receipt.workspace_fingerprint_after == fingerprint
        && receipt.workspace_fingerprint_before == receipt.workspace_fingerprint_after
        && same_canonical_path(Path::new(&receipt.cwd), root)
        && is_sha256(&receipt.stdout_sha256)
        && is_sha256(&receipt.stderr_sha256)
        && is_sha256(&receipt.invocation_sha256)
        && receipt.contract_sha256 == contract_sha256
        && is_sha256(&receipt.contract_sha256)
        && receipt.invocation_sha256
            == validation_invocation_sha256_scoped(
                &validation.command,
                &validation.impact_scopes,
                validation.non_code,
            )
        && validation_scope_is_well_formed(validation)
        && (!(match policy {
            ReceiptValidationPolicy::LegacyV1 => cargo_test_invocation(&parsed),
            ReceiptValidationPolicy::CurrentV2 => test_invocation(&parsed),
        }) || test_receipt_has_structured_proof(receipt))
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

fn validation_matches_expectation(
    command: &ParsedValidationCommand,
    expectation: ValidationExpectation,
) -> bool {
    if expectation == ValidationExpectation::NonProbeCommand {
        return !command_is_inert_probe(command);
    }
    if pytest_invocation(command) {
        return matches!(expectation, ValidationExpectation::PythonTest);
    }
    if release_installer_invocation(command) {
        return matches!(
            expectation,
            ValidationExpectation::RustBuildOrTest | ValidationExpectation::CargoManifestValidation
        );
    }
    if powershell_script(command).is_some_and(|script| {
        Path::new(script)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                matches!(
                    name.to_ascii_lowercase().as_str(),
                    "check-repo.ps1" | "audit-repository.ps1" | "verify-release-contract.ps1"
                )
            })
    }) {
        return true;
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
        // 已在函数开头短路，不会走到这里。
        ValidationExpectation::NonProbeCommand => true,
    }
}

fn normalized_path_text(path: &str) -> String {
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

pub(super) fn command_is_workspace_wide(_root: &Path, command: &ParsedValidationCommand) -> bool {
    release_installer_invocation(command)
        || powershell_script(command).is_some_and(|script| {
            Path::new(script)
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    matches!(
                        name.to_ascii_lowercase().as_str(),
                        "check-repo.ps1" | "audit-repository.ps1" | "verify-release-contract.ps1"
                    )
                })
        })
        || (cargo_subcommand(command).is_some()
            && command
                .args
                .iter()
                .any(|argument| matches!(argument.as_str(), "--workspace" | "--all")))
        || (pytest_invocation(command) && pytest_path_arguments(command).is_empty())
}

/// Authority receipts are stronger than ordinary validation receipts: the
/// command must be an explicit repository gate, not merely a locally relevant
/// build. Unknown ecosystems can opt in by exposing a reviewed workspace-local
/// gate at one of the conventional script paths below.
pub fn validate_authority_command(root: &Path, command: &str) -> Result<()> {
    let parsed = parse_validation_command(command)?;
    validate_command_security(root, &parsed)?;

    let trusted_script = powershell_script(&parsed).is_some_and(|script| {
        let lexical = if Path::new(script).is_absolute() {
            PathBuf::from(script)
        } else {
            root.join(script)
        };
        let Ok(canonical_root) = root.canonicalize() else {
            return false;
        };
        let Ok(canonical_script) = lexical.canonicalize() else {
            return false;
        };
        let Ok(relative) = canonical_script.strip_prefix(canonical_root) else {
            return false;
        };
        matches!(
            normalized_path_text(&relative.to_string_lossy()).as_str(),
            "check-repo.ps1"
                | "audit-repository.ps1"
                | "verify-release-contract.ps1"
                | "scripts/check-repo.ps1"
                | "scripts/audit-repository.ps1"
                | "scripts/verify-release-contract.ps1"
        )
    });
    let workspace_cargo_test = matches!(cargo_subcommand(&parsed), Some(("test", _)))
        && parsed
            .args
            .iter()
            .any(|argument| matches!(argument.as_str(), "--workspace" | "--all"));
    let workspace_pytest = pytest_invocation(&parsed) && pytest_path_arguments(&parsed).is_empty();

    if !trusted_script && !workspace_cargo_test && !workspace_pytest {
        bail!(
            "authority gate 必须是受检的 check-repo/audit-repository/verify-release-contract 脚本、`cargo test --workspace|--all`，或无路径选择器的全工作区 pytest"
        );
    }
    Ok(())
}

fn validation_matches_impact(
    root: &Path,
    command: &ParsedValidationCommand,
    impact: &ImpactEvidence,
) -> bool {
    let Some(expectation) = validation_expectation_for_path(&impact.changed_path) else {
        return true;
    };
    if !validation_matches_expectation(command, expectation) {
        return false;
    }
    // 未建模生态只做"非探针"这一条下限判断，不再往下走 Rust/pytest/cargo 的
    // 作用域匹配——那些规则对未知生态没有意义，硬套只会误拒。
    if expectation == ValidationExpectation::NonProbeCommand {
        return true;
    }
    if command_is_workspace_wide(root, command) {
        return true;
    }
    if executable_name(command) == "rustc" {
        return impact.changed_path.to_ascii_lowercase().ends_with(".rs")
            && command.args.iter().any(|argument| {
                argument.to_ascii_lowercase().ends_with(".rs")
                    && path_argument_matches(root, argument, &impact.changed_path)
            });
    }
    if pytest_invocation(command) {
        let paths = pytest_path_arguments(command);
        return paths.iter().any(|argument| {
            pytest_selector_covers(root, argument, &impact.changed_path)
                || impact
                    .candidate_tests
                    .iter()
                    .any(|test| pytest_selector_covers(root, argument, test))
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
                .any(|value| path_argument_matches(root, value, manifest))
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
) -> Result<()> {
    if impacts.is_empty() && !non_code {
        bail!("validation 必须提供至少一个 `--changed`；非代码需求必须显式使用 `--non-code`");
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
    let parsed = parse_validation_command(command)?;
    validate_command_security(root, &parsed)?;
    validate_scope_declaration(&parsed, impacts, non_code)?;
    for impact in impacts {
        let Some(expectation) = validation_expectation_for_path(&impact.changed_path) else {
            continue;
        };
        if !validation_matches_impact(root, &parsed, impact) {
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
    validation_relevance_gaps_for_fingerprint(
        requirement,
        goal,
        root,
        current_fingerprint,
        true,
        ReceiptValidationPolicy::CurrentV2,
    )
}

fn validation_relevance_gaps_for_fingerprint(
    requirement: &Requirement,
    goal: &Goal,
    root: &Path,
    fingerprint: &str,
    enforce_current_security: bool,
    policy: ReceiptValidationPolicy,
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
            validation_has_receipt_for_fingerprint(
                validation,
                root,
                fingerprint,
                &contract_sha256,
                enforce_current_security,
                policy,
            ) && validation.impact_scopes.iter().any(|scope| {
                scope.changed_path.replace('\\', "/") == impact.changed_path.replace('\\', "/")
                    && scope.package == impact.package
                    && scope.manifest_path.as_deref().map(normalized_path_text)
                        == impact.manifest_path.as_deref().map(normalized_path_text)
            }) && parse_validation_command(&validation.command)
                .is_ok_and(|parsed| validation_matches_impact(root, &parsed, impact))
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
        ReceiptValidationPolicy::CurrentV2,
    )
}

pub(super) fn goal_success_receipt_gaps_for_policy(
    goal: &Goal,
    root: &Path,
    fingerprint: &str,
    enforce_current_security: bool,
    policy: ReceiptValidationPolicy,
) -> Vec<String> {
    let mut gaps = Vec::new();
    if goal.status != GoalStatus::Success {
        gaps.push(format!("goal 状态为 {}，不是 success", goal.status));
    }
    if goal.replacement_authority.is_some() {
        if policy != ReceiptValidationPolicy::CurrentV2 {
            gaps.push("lifecycle-only replacement 只接受 current-v2 receipt policy".into());
        } else if let Some(error) = replacement_authority_error(goal, root, fingerprint) {
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
            validation_has_receipt_for_fingerprint(
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
        ));
    }
    if enforce_current_security {
        gaps.extend(goal_planning_gaps(goal, root, fingerprint));
    }
    gaps
}
