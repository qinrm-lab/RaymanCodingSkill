# Appended to the compiled-in definitions of the official installer. This
# entrypoint only publishes files; it never invokes an application or project.
$ErrorActionPreference = 'Stop'
$planJson = $env:RAYMAN_GLOBAL_INSTALL_PLAN
$plan = $planJson | ConvertFrom-Json -Depth 16
$planHash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($planJson))).ToLowerInvariant()
$journalPath = [string]$plan.journal_path
$outcome = 'files_published'
if (Test-Path -LiteralPath $journalPath) {
    $journal = Get-Content -LiteralPath $journalPath -Raw | ConvertFrom-Json -Depth 16
    Assert-RaymanUpdateJournalMatchesPlan -Journal $journal -Plan $plan -PlanSha256 $planHash
    $recovered = Repair-RaymanUpdateJournal -Journal $journal -JournalPath $journalPath
    if ($recovered -ne 'committed') { $outcome = 'rolled_back' }
} elseif ($env:RAYMAN_GLOBAL_INSTALL_RECOVERY -eq '1') {
    $outcome = 'not_started'
} else {
    $journal = New-RaymanUpdateJournal -Plan $plan -PlanSha256 $planHash
    Write-RaymanUpdateJournal -Path $journalPath -Value $journal
    $installed = @()
    try {
        for ($index = 0; $index -lt @($plan.files).Count; $index++) {
            $file = @($plan.files)[$index]
            $journal.phase = 'publishing'
            $journal.next_role = [string]$file.role
            $journal.updated_at = [DateTime]::UtcNow.ToString('O')
            Write-RaymanUpdateJournal -Path $journalPath -Value $journal
            $current = Get-RaymanUpdateFileHashOrNull -Path ([string]$file.destination)
            if (-not ([bool]$file.allow_existing_new -and $current -ceq [string]$file.new_sha256)) {
                $record = Install-FileWithRollback -Source ([string]$file.source) -Destination ([string]$file.destination) -Nonce ([string]$plan.transaction_id) -ExpectedHash ([string]$file.new_sha256) -ExpectedDestinationHash $file.expected_current_sha256 -ExpectDestinationAbsent:([bool]$file.expect_absent)
                $installed += $record
            }
            # GLOBAL_INSTALL_FAILURE_INJECTION_POINT (test builds only)
            $journal.entries[$index].completed = $true
            $journal.updated_at = [DateTime]::UtcNow.ToString('O')
            Write-RaymanUpdateJournal -Path $journalPath -Value $journal
        }
        foreach ($file in @($plan.files)) {
            Assert-ExpectedFileHash -Path ([string]$file.destination) -ExpectedHash ([string]$file.new_sha256) -Label 'Global fixed file publication'
        }
        $journal.phase = 'committed'
        $journal.committed = $true
        $journal.next_role = $null
        $journal.updated_at = [DateTime]::UtcNow.ToString('O')
        Write-RaymanUpdateJournal -Path $journalPath -Value $journal
    } catch {
        $errors = @(Invoke-InstallRollback -InstallRecords $installed)
        $journal.phase = if ($errors.Count) { 'blocked' } else { 'rolled_back' }
        $journal.next_role = $null
        $journal.updated_at = [DateTime]::UtcNow.ToString('O')
        Write-RaymanUpdateJournal -Path $journalPath -Value $journal
        throw
    }
}
# Backup cleanup is deliberately deferred. Recovery retains exact prior bytes.
@{schema='rayman.global-install-outcome.v1';status=$outcome;transaction_id=$plan.transaction_id;runtime_validation_required=$true;project_code_executed=$false} | ConvertTo-Json -Compress
