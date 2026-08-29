[CmdletBinding()]
param(
    [string]$WorkflowPath = (Join-Path (Split-Path -Parent $PSScriptRoot) '.github/workflows/ci.yml'),
    [switch]$SelfTest
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-CiWorkflowContract {
    param([Parameter(Mandatory = $true)][string]$Text)

    $lines = @($Text -split "`r?`n")
    $inJobEnv = $false
    for ($index = 0; $index -lt $lines.Count; $index++) {
        $line = $lines[$index]
        if ($line -match "`t") {
            throw "CI workflow uses a tab at line $($index + 1); indentation must be spaces"
        }
        if ($line.Trim().Length -eq 0 -or $line.TrimStart().StartsWith('#')) {
            continue
        }
        $indent = $line.Length - $line.TrimStart().Length
        if ($indent -eq 4 -and $line.Trim() -eq 'env:') {
            $inJobEnv = $true
            continue
        }
        if ($inJobEnv -and $indent -le 4) {
            $inJobEnv = $false
        }
        if ($inJobEnv -and $line -match '\$\{\{\s*runner\.') {
            throw "CI workflow uses runner context in job-level env at line $($index + 1); move it to a step env or run block"
        }
    }

    $releaseMatch = [regex]::Match(
        $Text,
        '(?ms)^  release-contract:\s*$(.*?)(?=^  [A-Za-z0-9_-]+:\s*$)'
    )
    if (-not $releaseMatch.Success) {
        throw 'CI workflow is missing the release-contract job'
    }
    $release = $releaseMatch.Groups[1].Value
    if ($release -notmatch 'workspace activate --skill-file' -or
        $release -notmatch '--yes') {
        throw 'release-contract must use the product-owned confirmed workspace activation command'
    }
    if ($release -match 'Set-Content[^\r\n]*workspace_skill\.yaml' -or
        $release -match '(?s)workspace_skill\.yaml.*Set-Content') {
        throw 'release-contract must not hand-write workspace_skill.yaml'
    }

    $coverageMatch = [regex]::Match(
        $Text,
        '(?ms)^  coverage:\s*$(.*?)(?=^  [A-Za-z0-9_-]+:\s*$)'
    )
    if (-not $coverageMatch.Success -or
        $coverageMatch.Groups[1].Value -notmatch '(?m)^        env:\s*$' -or
        $coverageMatch.Groups[1].Value -notmatch '\$\{\{\s*runner\.temp\s*\}\}') {
        throw 'coverage must bind runner.temp from a step-level env block'
    }

    # Every first-party Rust/PowerShell test with an all/Unix cfg needs a real
    # execution target.  Windows + Linux does not cover the explicit
    # non-Linux-Unix contract, so the three authoritative test jobs must keep
    # one macOS runner in their exact matrix.
    foreach ($jobName in @('check', 'test-traceability', 'evals')) {
        $jobMatch = [regex]::Match(
            $Text,
            "(?ms)^  $([regex]::Escape($jobName)):\s*`$(.*?)(?=^  [A-Za-z0-9_-]+:\s*`$|\z)"
        )
        if (-not $jobMatch.Success -or
            $jobMatch.Groups[1].Value -notmatch '(?m)^        os:\s*\[ubuntu-latest, windows-latest, macos-latest\]\s*$') {
            throw "CI job '$jobName' must execute the full Windows/Linux/non-Linux-Unix test matrix"
        }
    }

    # The traceability checker verifies immutable retirement against Git
    # history. Every job that executes it directly or through root tests/the
    # repository gate must have the predecessor commits locally available.
    foreach ($jobName in @('check', 'msrv', 'coverage', 'test-traceability')) {
        $jobMatch = [regex]::Match(
            $Text,
            "(?ms)^  $([regex]::Escape($jobName)):\s*`$(.*?)(?=^  [A-Za-z0-9_-]+:\s*`$|\z)"
        )
        if ($jobMatch.Success -and $jobMatch.Groups[1].Value -notmatch '(?m)^          fetch-depth:\s*0\s*$') {
            throw "CI job '$jobName' executes traceability-sensitive tests without fetch-depth: 0"
        }
    }
    $signedRelease = [regex]::Match(
        $Text,
        '(?ms)^  signed-release:\s*$(.*?)(?=^  [A-Za-z0-9_-]+:\s*$|\z)'
    )
    if ($signedRelease.Success -and
        $signedRelease.Groups[1].Value -notmatch '(?m)^      - test-traceability\s*$') {
        throw 'signed-release must directly depend on the full-history test-traceability job'
    }
    if (-not $signedRelease.Success -or
        $signedRelease.Groups[1].Value -notmatch '(?m)-AssetDirectory\s+dist(?:\s|$)') {
        throw 'signed-release must verify the complete staged asset directory before publication'
    }
    $freshness = [regex]::Match(
        $Text,
        '(?ms)^  signed-release-freshness:\s*$(.*?)(?=^  [A-Za-z0-9_-]+:\s*$|\z)'
    )
    if (-not $freshness.Success) {
        throw 'CI workflow is missing the signed-release-freshness job'
    }
    foreach ($required in @(
            '--pattern rayman-update-manifest-v1.json',
            '--pattern rayman-update-manifest-v1.sig',
            '--pattern rayman-windows-x86_64.exe',
            '--pattern rayman-update-worker-windows-x86_64.exe',
            '--pattern raymancodingskill-SKILL.md',
            '--pattern raymancodingskill-AGENTS.md',
            '--pattern raymancodingskill-workflow-contract.md',
            '--pattern install-rayman.ps1',
            '-AssetDirectory $assetRoot'
        )) {
        if (-not $freshness.Groups[1].Value.Contains($required)) {
            throw "signed-release-freshness does not verify complete bundle input: $required"
        }
    }
}

function Assert-Rejected {
    param(
        [Parameter(Mandatory = $true)][string]$Label,
        [Parameter(Mandatory = $true)][string]$Text
    )
    $rejected = $false
    try {
        Assert-CiWorkflowContract -Text $Text
    } catch {
        $rejected = $true
    }
    if (-not $rejected) {
        throw "CI workflow self-test did not reject: $Label"
    }
}

if ($SelfTest) {
    $valid = @'
jobs:
  check:
    strategy:
      matrix:
        os: [ubuntu-latest, windows-latest, macos-latest]
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
  test-traceability:
    strategy:
      matrix:
        os: [ubuntu-latest, windows-latest, macos-latest]
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
  evals:
    strategy:
      matrix:
        os: [ubuntu-latest, windows-latest, macos-latest]
    steps: []
  release-contract:
    steps:
      - run: rayman workspace activate --skill-file SKILL.md --yes
  coverage:
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - name: coverage
        env:
          CARGO_TARGET_DIR: ${{ runner.temp }}/target
        run: cargo llvm-cov
  next-job:
    steps: []
  signed-release:
    needs:
      - test-traceability
    steps:
      - run: ./scripts/check-update-freshness.ps1 -AssetDirectory dist
  signed-release-freshness:
    steps:
      - run: |
          gh release download v1.2.3 --pattern rayman-update-manifest-v1.json --pattern rayman-update-manifest-v1.sig --pattern rayman-windows-x86_64.exe --pattern rayman-update-worker-windows-x86_64.exe --pattern raymancodingskill-SKILL.md --pattern raymancodingskill-AGENTS.md --pattern raymancodingskill-workflow-contract.md --pattern install-rayman.ps1
          ./scripts/check-update-freshness.ps1 -AssetDirectory $assetRoot
'@
    Assert-CiWorkflowContract -Text $valid
    Assert-Rejected -Label 'runner context in job env' -Text @'
jobs:
  release-contract:
    steps:
      - run: rayman workspace activate --skill-file SKILL.md --yes
  coverage:
    env:
      CARGO_TARGET_DIR: ${{ runner.temp }}/target
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - name: coverage
        env:
          CARGO_TARGET_DIR: ${{ runner.temp }}/target
        run: cargo llvm-cov
  next-job:
    steps: []
'@
    Assert-Rejected -Label 'hand-written activation' -Text @'
jobs:
  release-contract:
    steps:
      - run: Set-Content .RaymanCodingSkill/workspace_skill.yaml
  coverage:
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - name: coverage
        env:
          CARGO_TARGET_DIR: ${{ runner.temp }}/target
        run: cargo llvm-cov
  next-job:
    steps: []
'@
    Assert-Rejected -Label 'shallow traceability history' -Text ($valid -replace 'fetch-depth: 0', 'fetch-depth: 1')
    Assert-Rejected -Label 'signed release without traceability dependency' -Text ($valid -replace '      - test-traceability', '      - check')
    Assert-Rejected -Label 'missing non-linux-unix test gate' -Text ($valid -replace ', macos-latest', '')
    Assert-Rejected -Label 'freshness missing payload asset' -Text ($valid -replace '--pattern raymancodingskill-SKILL.md', '--pattern missing-SKILL.md')
    Assert-Rejected -Label 'tag release missing staged asset directory' -Text ($valid -replace '-AssetDirectory dist', '-MinimumRemainingDays 29')
    Write-Output 'check-ci-workflow self-test: PASS'
    return
}

if (-not (Test-Path -LiteralPath $WorkflowPath -PathType Leaf)) {
    throw "CI workflow is missing: $WorkflowPath"
}
$text = [IO.File]::ReadAllText(
    (Resolve-Path -LiteralPath $WorkflowPath).ProviderPath,
    [Text.UTF8Encoding]::new($false, $true)
)
Assert-CiWorkflowContract -Text $text
Write-Output 'check-ci-workflow: PASS'
