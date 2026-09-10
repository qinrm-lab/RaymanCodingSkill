[CmdletBinding(DefaultParameterSetName='Plan')]
param(
    [Parameter(ParameterSetName='Plan')][switch]$Plan,
    [Parameter(Mandatory,ParameterSetName='Simulate')][switch]$Simulate,
    [Parameter(Mandatory,ParameterSetName='SelfTest')][switch]$SelfTest,
    [Parameter(Mandatory,ParameterSetName='Install')][switch]$Install,
    [Parameter(Mandatory,ParameterSetName='Install')][switch]$Yes,
    [Parameter(Mandatory,ParameterSetName='Install')][string]$ExpectedCommit,
    [Parameter(Mandatory,ParameterSetName='Install')][string]$GoalId,
    [Parameter(Mandatory,ParameterSetName='Install')][string]$SaveStatusSource,
    [Parameter(Mandatory,ParameterSetName='Install')][string]$ExpectedSaveStatusCommit,
    [Parameter(ParameterSetName='Install')][string]$CanonicalSourceRoot=(Split-Path -Parent $PSScriptRoot),
    [string]$Root='C:\ProgramData\Rayman\CodexGlobalExecution',
    [string]$WorkerPath=(Join-Path (Split-Path -Parent $PSScriptRoot) 'target/release/rayman-global-worker.exe'),
    [string]$ClientPath=(Join-Path (Split-Path -Parent $PSScriptRoot) 'target/release/rayman-global.exe'),
    [string]$GitPath='C:\Program Files\Git\mingw64\bin\git.exe',
    [string]$RaymanPath,
    [string]$UserAccount="$env:USERDOMAIN\$env:USERNAME",
    [string]$SandboxGroup="$env:COMPUTERNAME\CodexSandboxUsers",
    [string]$TaskName='Rayman-CodexGlobalExecution'
)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
if(-not $IsWindows -or $PSVersionTable.PSVersion.Major -lt 7){throw 'Native Windows PowerShell 7 is required.'}
$repoRoot=Split-Path -Parent $PSScriptRoot
$identity=[Security.Principal.WindowsIdentity]::GetCurrent()
$userSid=([Security.Principal.NTAccount]::new($UserAccount)).Translate([Security.Principal.SecurityIdentifier]).Value
$sandboxSid=([Security.Principal.NTAccount]::new($SandboxGroup)).Translate([Security.Principal.SecurityIdentifier]).Value

function Assert-OrdinaryPath([string]$Path){
    $current=[IO.Path]::GetFullPath($Path)
    while($current){
        if(Test-Path -LiteralPath $current){$item=Get-Item -LiteralPath $current -Force;if($item.Attributes -band [IO.FileAttributes]::ReparsePoint){throw "Reparse path refused: $current"}}
        $parent=[IO.Path]::GetDirectoryName($current);if($parent -eq $current){break};$current=$parent
    }
}
function Protect-Directory([string]$Path,[string]$Owner,[switch]$Queue){
    $security=[Security.AccessControl.DirectorySecurity]::new()
    $security.SetAccessRuleProtection($true,$false)
    $security.SetOwner([Security.Principal.SecurityIdentifier]::new($Owner))
    foreach($sid in @($Owner,'S-1-5-18','S-1-5-32-544')){
        $security.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]::new($sid),[Security.AccessControl.FileSystemRights]::FullControl,[Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit',[Security.AccessControl.PropagationFlags]::None,[Security.AccessControl.AccessControlType]::Allow))
    }
    if($Queue){
        $rights=[Security.AccessControl.FileSystemRights]'CreateFiles,ReadAndExecute,Synchronize'
        $security.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]::new($sandboxSid),$rights,[Security.AccessControl.InheritanceFlags]::None,[Security.AccessControl.PropagationFlags]::None,[Security.AccessControl.AccessControlType]::Allow))
        $security.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]::new($sandboxSid),[Security.AccessControl.FileSystemRights]::Modify,[Security.AccessControl.InheritanceFlags]::ObjectInherit,[Security.AccessControl.PropagationFlags]::InheritOnly,[Security.AccessControl.AccessControlType]::Allow))
    }else{
        $security.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]::new($sandboxSid),[Security.AccessControl.FileSystemRights]::ReadAndExecute,[Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit',[Security.AccessControl.PropagationFlags]::None,[Security.AccessControl.AccessControlType]::Allow))
    }
    Set-Acl -LiteralPath $Path -AclObject $security
}
function Invoke-JsonProcess([string]$Program,[string[]]$Arguments,[int]$TimeoutMilliseconds=30000,[string]$WorkingDirectory=(Get-Location).ProviderPath){
    $start=[Diagnostics.ProcessStartInfo]::new();$start.FileName=$Program;$start.WorkingDirectory=$WorkingDirectory;$start.UseShellExecute=$false;$start.CreateNoWindow=$true;$start.RedirectStandardOutput=$true;$start.RedirectStandardError=$true
    foreach($arg in $Arguments){$start.ArgumentList.Add($arg)}
    $process=[Diagnostics.Process]::new();$process.StartInfo=$start
    try{
        if(-not $process.Start()){throw 'Cannot start global worker'}
        $out=$process.StandardOutput.ReadToEndAsync();$err=$process.StandardError.ReadToEndAsync()
        if(-not $process.WaitForExit($TimeoutMilliseconds)){$process.Kill($true);$process.WaitForExit();throw "Global process timed out: $Program"}
        $stdout=$out.GetAwaiter().GetResult();$stderr=$err.GetAwaiter().GetResult()
        try{$document=$stdout | ConvertFrom-Json -Depth 30}catch{throw "Global process returned invalid JSON: exit=$($process.ExitCode) stdout=$stdout stderr=$stderr"}
        [pscustomobject]@{exit_code=$process.ExitCode;document=$document;stdout=$stdout;stderr=$stderr}
    }finally{$process.Dispose()}
}
function Invoke-Json([string]$Program,[string[]]$Arguments,[int]$TimeoutMilliseconds=30000,[string]$WorkingDirectory=(Get-Location).ProviderPath){
    $result=Invoke-JsonProcess $Program $Arguments $TimeoutMilliseconds $WorkingDirectory
    if($result.exit_code -ne 0){throw "Global process failed: exit=$($result.exit_code) stdout=$($result.stdout) stderr=$($result.stderr)"}
    $result.document
}
function Assert-SourceGoalReady($Report,[string]$GoalId,[string]$ExpectedCommit){
    if($Report.exit_code -notin @(0,1) -or $null -eq $Report.document -or $Report.document.profile -cne 'release' -or $Report.document.task.goal_id -cne $GoalId -or -not $Report.document.task.ready -or -not $Report.document.source.clean -or $Report.document.source.head -cne $ExpectedCommit){
        throw "Source Goal is not ready: exit=$($Report.exit_code) stdout=$($Report.stdout) stderr=$($Report.stderr)"
    }
    $Report.document
}
function Invoke-FixedProcess([string]$Program,[string[]]$Arguments,[string]$WorkingDirectory,[int]$TimeoutMilliseconds){
    $start=[Diagnostics.ProcessStartInfo]::new();$start.FileName=$Program;$start.WorkingDirectory=$WorkingDirectory;$start.UseShellExecute=$false;$start.CreateNoWindow=$true;$start.RedirectStandardOutput=$true;$start.RedirectStandardError=$true
    foreach($arg in $Arguments){$start.ArgumentList.Add($arg)}
    $process=[Diagnostics.Process]::new();$process.StartInfo=$start
    try{
        if(-not $process.Start()){throw 'Cannot start fixed rollout process'}
        $out=$process.StandardOutput.ReadToEndAsync();$err=$process.StandardError.ReadToEndAsync()
        if(-not $process.WaitForExit($TimeoutMilliseconds)){$process.Kill($true);$process.WaitForExit();throw "Fixed rollout process timed out: $Program"}
        $stdout=$out.GetAwaiter().GetResult();$stderr=$err.GetAwaiter().GetResult()
        if($process.ExitCode -ne 0){throw "Fixed rollout process failed ($($process.ExitCode)): $Program`n$stderr"}
        [ordered]@{
            exit_code=$process.ExitCode
            stdout_sha256=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($stdout))).ToLowerInvariant()
            stderr_sha256=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($stderr))).ToLowerInvariant()
        }
    }finally{$process.Dispose()}
}
if(-not $SelfTest){
    foreach($path in @($WorkerPath,$ClientPath)){Assert-OrdinaryPath $path;if(-not(Test-Path -LiteralPath $path -PathType Leaf)){throw "Build the global worker/client first: $path"}}
    $workerHash=(Get-FileHash -LiteralPath $WorkerPath).Hash.ToLowerInvariant()
    $clientHash=(Get-FileHash -LiteralPath $ClientPath).Hash.ToLowerInvariant()
}else{$workerHash='0'*64;$clientHash='0'*64}
$description=[ordered]@{schema='rayman.global-install-plan.v2';root=[IO.Path]::GetFullPath($Root);owner_sid=$userSid;worker_sha256=$workerHash;client_sha256=$clientHash;task_name=$TaskName;run_level='LeastPrivilege';workspace_acl_preflight='required';products=@('rayman','save-work-status');state_migration='trusted_projects';resume_requires_exact_installed_identity=$true;existing_repositories_changed=$false;configuration_changed=$false}
function Get-WorkerTaskArguments([string]$InstallRoot){
    $resolved=[IO.Path]::GetFullPath($InstallRoot)
    if($resolved.Contains('"') -or $resolved.Contains("`r") -or $resolved.Contains("`n")){throw 'Invalid worker task root'}
    return 'serve --root "'+$resolved+'"'
}
function Assert-WorkerTaskXml([string]$Xml,[string]$Program,[string]$Arguments,[string]$Account){
    [xml]$task=$Xml
    $taskNamespace='http://schemas.microsoft.com/windows/2004/02/mit/task'
    if($task.DocumentElement.NamespaceURI -cne $taskNamespace){throw 'Live worker task has an invalid namespace.'}
    $ns=[Xml.XmlNamespaceManager]::new($task.NameTable);$ns.AddNamespace('t',$taskNamespace)
    $read={param([string]$XPath);$nodes=@($task.SelectNodes($XPath,$ns));if($nodes.Count -ne 1){throw "Live worker task must contain exactly one $XPath"};[string]$nodes[0].InnerText}
    $command=& $read '/t:Task/t:Actions/t:Exec/t:Command'
    $actualArguments=& $read '/t:Task/t:Actions/t:Exec/t:Arguments'
    $user=& $read '/t:Task/t:Principals/t:Principal/t:UserId'
    $logon=& $read '/t:Task/t:Principals/t:Principal/t:LogonType'
    $runLevels=@($task.SelectNodes('/t:Task/t:Principals/t:Principal/t:RunLevel',$ns))
    if($runLevels.Count -gt 1 -or ($runLevels.Count -eq 1 -and [string]$runLevels[0].InnerText -cne 'LeastPrivilege')){throw 'Live worker task has an elevated, duplicate, or invalid RunLevel.'}
    if(-not $command.Equals($Program,[StringComparison]::OrdinalIgnoreCase) -or $actualArguments -cne $Arguments -or
       -not $user.Equals($Account,[StringComparison]::OrdinalIgnoreCase) -or $logon -cne 'InteractiveToken'){
        throw 'Live worker task differs from its fixed least-privilege contract.'
    }
}
function Get-InitializedResume([string]$InstallRoot,[string]$OwnerSid,[string]$ExpectedWorkerHash,[string]$ExpectedClientHash){
    Assert-OrdinaryPath $InstallRoot
    $expected=@('.installation.rayman.lock','client.exe','installation.json','requests','worker.exe')
    $actual=@(Get-ChildItem -LiteralPath $InstallRoot -Force|ForEach-Object Name|Sort-Object)
    if(($actual -join "`n") -cne (($expected|Sort-Object) -join "`n")){throw 'Initialized global installation contains an unexpected or missing entry.'}
    if(-not(Test-Path -LiteralPath (Join-Path $InstallRoot 'requests') -PathType Container) -or @(Get-ChildItem -LiteralPath (Join-Path $InstallRoot 'requests') -Force).Count){throw 'Initialized global installation request queue is missing or non-empty.'}
    $actualOwner=([Security.Principal.NTAccount]::new((Get-Acl -LiteralPath $InstallRoot).Owner)).Translate([Security.Principal.SecurityIdentifier]).Value
    if($actualOwner -cne $OwnerSid){throw 'Initialized global installation owner differs.'}
    if((Get-FileHash (Join-Path $InstallRoot 'worker.exe')).Hash.ToLowerInvariant() -cne $ExpectedWorkerHash -or (Get-FileHash (Join-Path $InstallRoot 'client.exe')).Hash.ToLowerInvariant() -cne $ExpectedClientHash){throw 'Initialized global installation executable identity differs.'}
    $installed=Get-Content -Raw -LiteralPath (Join-Path $InstallRoot 'installation.json')|ConvertFrom-Json -Depth 12
    if($installed.schema_version -ne 1 -or $installed.owner_sid -cne $OwnerSid -or $installed.worker_sha256 -cne $ExpectedWorkerHash -or $installed.client_sha256 -cne $ExpectedClientHash -or [string]::IsNullOrWhiteSpace([string]$installed.installation_id) -or [string]::IsNullOrWhiteSpace([string]$installed.root_identity) -or [string]::IsNullOrWhiteSpace([string]$installed.source_project_id)){throw 'Initialized global installation receipt differs.'}
    $installed
}
function Stop-VerifiedWorkerTask([string]$Name,[string]$InstallRoot,[string]$Account){
    $program=Join-Path $InstallRoot 'worker.exe'
    Assert-WorkerTaskXml (Export-ScheduledTask -TaskName $Name) $program (Get-WorkerTaskArguments $InstallRoot) $Account
    # Prevent a logon trigger racing the rollback; unregister alone does not
    # terminate a running task. Preserve the installation if stop is uncertain.
    Disable-ScheduledTask -TaskName $Name | Out-Null
    Stop-ScheduledTask -TaskName $Name
    $deadline=[DateTimeOffset]::UtcNow.AddSeconds(10)
    do {
        $task=Get-ScheduledTask -TaskName $Name -ErrorAction Stop
        if([string]$task.State -notin @('Running','Queued')){break}
        if([DateTimeOffset]::UtcNow -ge $deadline){throw 'Worker task did not stop; backend retained for recovery.'}
        Start-Sleep -Milliseconds 100
    } while($true)
    $lockPath=Join-Path $InstallRoot '.worker.rayman.lock'
    Assert-OrdinaryPath $lockPath
    $guard=$null
    try {
        # Exclusive open proves the worker released its lifetime lock. Keep it
        # held through task removal so no second worker can start in between.
        if(Test-Path -LiteralPath $lockPath){$guard=[IO.File]::Open($lockPath,[IO.FileMode]::Open,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)}
        Unregister-ScheduledTask -TaskName $Name -Confirm:$false -ErrorAction Stop
    } finally {if($null -ne $guard){$guard.Dispose()}}
}
if($SelfTest){
    $arguments=Get-WorkerTaskArguments 'C:\ProgramData\Rayman\CodexGlobalExecution'
    if($arguments -cne 'serve --root "C:\ProgramData\Rayman\CodexGlobalExecution"'){throw 'Task argument rendering differs'}
    $xml='<?xml version="1.0"?><Task xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task"><Principals><Principal><UserId>QIN5521\qinrm</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals><Actions><Exec><Command>C:\ProgramData\Rayman\CodexGlobalExecution\worker.exe</Command><Arguments>serve --root "C:\ProgramData\Rayman\CodexGlobalExecution"</Arguments></Exec></Actions></Task>'
    Assert-WorkerTaskXml $xml 'C:\ProgramData\Rayman\CodexGlobalExecution\worker.exe' $arguments 'QIN5521\qinrm'
    Assert-WorkerTaskXml ($xml.Replace('<RunLevel>LeastPrivilege</RunLevel>','')) 'C:\ProgramData\Rayman\CodexGlobalExecution\worker.exe' $arguments 'QIN5521\qinrm'
    $rejected=$false;try{Assert-WorkerTaskXml ($xml.Replace('LeastPrivilege','HighestAvailable')) 'C:\ProgramData\Rayman\CodexGlobalExecution\worker.exe' $arguments 'QIN5521\qinrm'}catch{$rejected=$true}
    if(-not $rejected){throw 'Elevated task fixture was accepted'}
    foreach($invalidXml in @($xml.Replace('</RunLevel>','</RunLevel><RunLevel>LeastPrivilege</RunLevel>'),$xml.Replace('<Command>C:\ProgramData\Rayman\CodexGlobalExecution\worker.exe</Command>',''))){$rejected=$false;try{Assert-WorkerTaskXml $invalidXml 'C:\ProgramData\Rayman\CodexGlobalExecution\worker.exe' $arguments 'QIN5521\qinrm'}catch{$rejected=$true};if(-not $rejected){throw 'Invalid materialized task XML was accepted'}}
    & {
        $fixture=Join-Path ([IO.Path]::GetTempPath()) ('global-stop-test-'+[guid]::NewGuid().ToString('N'))
        [void][IO.Directory]::CreateDirectory($fixture)
        $events=[Collections.Generic.List[string]]::new()
        function Export-ScheduledTask {param($TaskName) '<Task xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task"><Principals><Principal><UserId>fixture-owner</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals><Actions><Exec><Command>'+[Security.SecurityElement]::Escape((Join-Path $fixture 'worker.exe'))+'</Command><Arguments>'+[Security.SecurityElement]::Escape((Get-WorkerTaskArguments $fixture))+'</Arguments></Exec></Actions></Task>'}
        function Disable-ScheduledTask {param($TaskName) $events.Add('disable')}
        function Stop-ScheduledTask {param($TaskName) $events.Add('stop')}
        function Get-ScheduledTask {param($TaskName,$ErrorAction) $events.Add('inspect'); [pscustomobject]@{State='Disabled'}}
        function Unregister-ScheduledTask {param($TaskName,$Confirm,$ErrorAction) $events.Add('unregister')}
        $lock=Join-Path $fixture '.worker.rayman.lock'
        [IO.File]::WriteAllText($lock,'')
        Stop-VerifiedWorkerTask 'fixture' $fixture 'fixture-owner'
        if(($events -join ',') -cne 'disable,stop,inspect,unregister'){throw 'Task stop ordering incorrect'}
        $events.Clear()
        $held=[IO.File]::Open($lock,[IO.FileMode]::Open,[IO.FileAccess]::ReadWrite,[IO.FileShare]::ReadWrite)
        try {
            $rejected=$false
            try{Stop-VerifiedWorkerTask 'fixture' $fixture 'fixture-owner'}catch{$rejected=$true}
            if(-not $rejected -or $events.Contains('unregister')){throw 'Live worker lock did not prevent rollback'}
        }finally{$held.Dispose()}
        $events.Clear()
        $rejected=$false
        try{Stop-VerifiedWorkerTask 'fixture' $fixture 'wrong-owner'}catch{$rejected=$true}
        if(-not $rejected -or $events.Count){throw 'Mismatched task was modified'}
        $probePwsh=@(Get-Command pwsh -CommandType Application -ErrorAction Stop|Select-Object -First 1)
        $probe=Invoke-FixedProcess $probePwsh[0].Source @('-NoProfile','-Command','[Console]::Out.Write(''ok'')') $fixture 30000
        if($probe.exit_code -ne 0 -or [string]::IsNullOrWhiteSpace([string]$probe.stdout_sha256)){throw 'Fixed process probe differs'}
        $readyHead='a'*40
        $readyDocument=[ordered]@{profile='release';task=[ordered]@{goal_id='goal_0123456789';ready=$true};source=[ordered]@{clean=$true;head=$readyHead}}
        $readyJson=$readyDocument|ConvertTo-Json -Depth 5 -Compress
        $encoded=[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($readyJson))
        $childCommand="[Console]::Out.Write([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$encoded')));[Console]::Error.Write('structured-stderr');exit 1"
        $report=Invoke-JsonProcess $probePwsh[0].Source @('-NoProfile','-Command',$childCommand)
        if($report.exit_code -ne 1 -or $report.stdout -cne $readyJson -or $report.stderr -cne 'structured-stderr' -or -not $report.document.task.ready){throw 'Structured nonzero JSON process report differs'}
        $accepted=Assert-SourceGoalReady $report 'goal_0123456789' $readyHead
        if(-not $accepted.task.ready){throw 'Ready source Goal report was not accepted'}
        $rejected=$false
        try{[void](Invoke-Json $probePwsh[0].Source @('-NoProfile','-Command',$childCommand))}catch{$rejected=$_.Exception.Message.Contains('exit=1') -and $_.Exception.Message.Contains($readyJson) -and $_.Exception.Message.Contains('structured-stderr')}
        if(-not $rejected){throw 'Generic JSON failure did not preserve stdout and stderr diagnostics'}
        foreach($invalid in @(
            [pscustomobject]@{exit_code=2;document=$report.document;stdout=$report.stdout;stderr=$report.stderr},
            [pscustomobject]@{exit_code=1;document=[pscustomobject]@{profile='release';task=[pscustomobject]@{goal_id='goal_0123456789';ready=$true};source=[pscustomobject]@{clean=$true;head='b'*40}};stdout=$report.stdout;stderr=$report.stderr}
        )){
            $rejected=$false;try{[void](Assert-SourceGoalReady $invalid 'goal_0123456789' $readyHead)}catch{$rejected=$true}
            if(-not $rejected){throw 'Invalid source Goal process report was accepted'}
        }
        $initialized=Join-Path $fixture 'initialized';[void][IO.Directory]::CreateDirectory($initialized);[void][IO.Directory]::CreateDirectory((Join-Path $initialized 'requests'))
        [IO.File]::WriteAllText((Join-Path $initialized '.installation.rayman.lock'),'')
        [IO.File]::WriteAllBytes((Join-Path $initialized 'worker.exe'),[byte[]](1,2,3));[IO.File]::WriteAllBytes((Join-Path $initialized 'client.exe'),[byte[]](4,5,6))
        $fixtureOwner=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value;$fixtureWorker=(Get-FileHash (Join-Path $initialized 'worker.exe')).Hash.ToLowerInvariant();$fixtureClient=(Get-FileHash (Join-Path $initialized 'client.exe')).Hash.ToLowerInvariant()
        $installation=[ordered]@{schema_version=1;installation_id='a'*32;owner_sid=$fixtureOwner;root_identity='b'*64;worker_sha256=$fixtureWorker;client_sha256=$fixtureClient;source_project_id='c'*32}
        [IO.File]::WriteAllText((Join-Path $initialized 'installation.json'),($installation|ConvertTo-Json),[Text.UTF8Encoding]::new($false))
        $accepted=Get-InitializedResume $initialized $fixtureOwner $fixtureWorker $fixtureClient;if($accepted.installation_id -cne 'a'*32){throw 'Initialized resume fixture was not accepted'}
        [IO.File]::WriteAllText((Join-Path $initialized 'unexpected.txt'),'drift');$rejected=$false;try{[void](Get-InitializedResume $initialized $fixtureOwner $fixtureWorker $fixtureClient)}catch{$rejected=$true};Remove-Item (Join-Path $initialized 'unexpected.txt') -Force;if(-not $rejected){throw 'Initialized resume accepted an extra file'}
        [IO.File]::WriteAllText((Join-Path $initialized 'requests\request.json'),'{}');$rejected=$false;try{[void](Get-InitializedResume $initialized $fixtureOwner $fixtureWorker $fixtureClient)}catch{$rejected=$true};Remove-Item (Join-Path $initialized 'requests\request.json') -Force;if(-not $rejected){throw 'Initialized resume accepted a non-empty queue'}
        $rejected=$false;try{[void](Get-InitializedResume $initialized $fixtureOwner ('0'*64) $fixtureClient)}catch{$rejected=$true};if(-not $rejected){throw 'Initialized resume accepted executable drift'}
        Remove-Item -LiteralPath $fixture -Recurse -Force
    }
    Write-Output 'install-global-codex-execution: PASS (task materialization, initialized resume, structured readiness and fixed process capture)'
    return
}
if($PSCmdlet.ParameterSetName -eq 'Plan'){$description | ConvertTo-Json;return}
if($Install){
    if($identity.User.Value -cne $userSid -or $identity.Name -match '\\CodexSandbox'){throw 'Production installation must use the explicitly selected desktop user.'}
    $pwsh=@(Get-Command pwsh -CommandType Application -ErrorAction Stop|Select-Object -First 1)[0].Source
    $aclRepair=Join-Path $PSScriptRoot 'repair-codex-workspace-acl.ps1'
    $aclPlan=Invoke-Json $pwsh @('-NoProfile','-File',$aclRepair,'-Plan','-OwnerAccount',$UserAccount)
    if(-not $aclPlan.safe){
        throw "Trusted project Git ACL preflight is not safe: needs_prepare=$($aclPlan.needs_prepare_count), blocked=$($aclPlan.blocked_count). Run the fixed ACL preparation from a healthy Codex thread before installation."
    }
    $git=@(Get-Command git -All -ErrorAction Stop | Where-Object CommandType -eq Application | Select-Object -First 1);if($git.Count -ne 1){throw 'Git must resolve to a native application'};$git=$git[0]
    $head=(& $git.Source -C $repoRoot rev-parse HEAD).Trim()
    if($LASTEXITCODE -ne 0 -or $head -cne $ExpectedCommit -or @(& $git.Source -C $repoRoot status --porcelain=v1 --untracked-files=all).Count){throw 'An exact clean independently committed source is required.'}
    $sourceRoot=[IO.Path]::GetFullPath($CanonicalSourceRoot);Assert-OrdinaryPath $sourceRoot
    $sourceHead=(& $git.Source -C $sourceRoot rev-parse HEAD).Trim()
    if($LASTEXITCODE -ne 0 -or $sourceHead -cne $ExpectedCommit -or @(& $git.Source -C $sourceRoot status --porcelain=v1 --untracked-files=all).Count){throw 'Canonical Rayman source must be the same exact clean commit as the validated source.'}
    $saveRoot=[IO.Path]::GetFullPath($SaveStatusSource);Assert-OrdinaryPath $saveRoot
    foreach($required in @('Cargo.toml','save-work-status\scripts\install_skill.py','target\release\save-work-status.exe','target\release\save-work-status-task-launcher.exe')){
        if(-not(Test-Path -LiteralPath (Join-Path $saveRoot $required) -PathType Leaf)){throw "SaveStatus release input is missing: $required"}
    }
    $saveHead=(& $git.Source -C $saveRoot rev-parse HEAD).Trim()
    if($LASTEXITCODE -ne 0 -or $saveHead -cne $ExpectedSaveStatusCommit -or @(& $git.Source -C $saveRoot status --porcelain=v1 --untracked-files=all).Count){throw 'An exact clean independently committed SaveStatus source is required.'}
    $saveVersion=& (Join-Path $saveRoot 'target\release\save-work-status.exe') version
    if($LASTEXITCODE -ne 0 -or (($saveVersion|ConvertFrom-Json).skill_version -cne '2.8.0')){throw 'SaveStatus release runtime is not v2.8.0'}
    $gate=Join-Path $repoRoot 'target/release/rayman.exe'
    Push-Location $repoRoot
    try{$finishReport=Invoke-JsonProcess $gate @('--format','json','check','--goal',$GoalId,'--profile','release');$finish=Assert-SourceGoalReady $finishReport $GoalId $ExpectedCommit}finally{Pop-Location}
    $resolvedRoot=[IO.Path]::GetFullPath($Root)
    if(-not $resolvedRoot.Equals('C:\ProgramData\Rayman\CodexGlobalExecution',[StringComparison]::OrdinalIgnoreCase)){throw 'Production root must match the fixed global configuration endpoint.'}
    $resumeExisting=Test-Path -LiteralPath $resolvedRoot -PathType Container
    $resumeComplete=$false;$resumeInitialized=$false;$retainedReceipt=$null
    if($resumeExisting){
        Assert-OrdinaryPath $resolvedRoot
        foreach($name in @('worker.exe','client.exe','installation.json')){if(-not(Test-Path -LiteralPath (Join-Path $resolvedRoot $name) -PathType Leaf)){throw "Retained global installation is incomplete: $name"}}
        if((Get-FileHash (Join-Path $resolvedRoot 'worker.exe')).Hash.ToLowerInvariant() -cne $workerHash -or (Get-FileHash (Join-Path $resolvedRoot 'client.exe')).Hash.ToLowerInvariant() -cne $clientHash){throw 'Retained global executable identity differs from this exact source'}
        if(Test-Path -LiteralPath (Join-Path $resolvedRoot 'install-receipt.json') -PathType Leaf){
            $retainedReceipt=Get-Content -LiteralPath (Join-Path $resolvedRoot 'install-receipt.json') -Raw|ConvertFrom-Json -Depth 12
            if($retainedReceipt.source_commit -cne $ExpectedCommit -or $retainedReceipt.goal_id -cne $GoalId -or $retainedReceipt.owner_sid -cne $userSid){throw 'Retained global installation receipt differs from this rollout'}
            $task=Get-ScheduledTask -TaskName $TaskName -ErrorAction Stop
            Assert-WorkerTaskXml (Export-ScheduledTask -TaskName $TaskName) (Join-Path $resolvedRoot 'worker.exe') (Get-WorkerTaskArguments $resolvedRoot) $userSid
            if([string]$task.State -notin @('Running','Queued')){Start-ScheduledTask -TaskName $TaskName}
            $resumeComplete=$true
        }else{
            if(Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue){throw 'Initialized global installation has a task without a final receipt.'}
            [void](Get-InitializedResume $resolvedRoot $userSid $workerHash $clientHash)
            $resumeInitialized=$true
        }
    } elseif(Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue){throw 'Global execution task exists without its exact installation root.'}
    $parent=Split-Path -Parent $resolvedRoot
    [void][IO.Directory]::CreateDirectory($parent)
    Assert-OrdinaryPath $parent
    $transaction=[Guid]::NewGuid().ToString('N')
    $stage=Join-Path $parent ((Split-Path -Leaf $resolvedRoot)+'.install-'+$transaction)
    $failed=Join-Path $parent ((Split-Path -Leaf $resolvedRoot)+'.failed-'+$transaction)
    $taskRegistered=$false;$published=$resumeExisting;$configurationPlan=$null;$configurationResult=$null;$externalIntegrationStarted=$false;$resumed=$resumeExisting
    try{
        if(-not $resumeExisting){
            [void][IO.Directory]::CreateDirectory($stage);Protect-Directory $stage $userSid
            [void][IO.Directory]::CreateDirectory((Join-Path $stage 'requests'));Protect-Directory (Join-Path $stage 'requests') $userSid -Queue
            Copy-Item -LiteralPath $WorkerPath -Destination (Join-Path $stage 'worker.exe')
            Copy-Item -LiteralPath $ClientPath -Destination (Join-Path $stage 'client.exe')
            if((Get-FileHash (Join-Path $stage 'worker.exe')).Hash.ToLowerInvariant() -cne $workerHash -or (Get-FileHash (Join-Path $stage 'client.exe')).Hash.ToLowerInvariant() -cne $clientHash){throw 'Staged executable identity differs.'}
            $installed=Invoke-Json (Join-Path $stage 'worker.exe') @('--format','json','initialize','--root',$stage,'--source-workspace',$repoRoot,'--yes')
            if($installed.owner_sid -cne $userSid -or $installed.worker_sha256 -cne $workerHash -or $installed.client_sha256 -cne $clientHash){throw 'Initialized installation identity differs.'}
            Move-Item -LiteralPath $stage -Destination $resolvedRoot;$published=$true
        }
        $worker=Join-Path $resolvedRoot 'worker.exe'
        if(-not $resumeComplete){
            $arguments=Get-WorkerTaskArguments $resolvedRoot
            $action=New-ScheduledTaskAction -Execute $worker -Argument $arguments
            $trigger=New-ScheduledTaskTrigger -AtLogOn -User $UserAccount
            $principal=New-ScheduledTaskPrincipal -UserId $UserAccount -LogonType Interactive -RunLevel Limited
            $settings=New-ScheduledTaskSettingsSet -StartWhenAvailable -MultipleInstances IgnoreNew -ExecutionTimeLimit ([TimeSpan]::Zero) -Hidden
            Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Description 'Protected qinrm fixed-capability worker for enrolled Codex projects' | Out-Null
            $taskRegistered=$true
            Assert-WorkerTaskXml (Export-ScheduledTask -TaskName $TaskName) $worker $arguments $userSid
            Start-ScheduledTask -TaskName $TaskName
        }
        $deadline=[DateTimeOffset]::UtcNow.AddSeconds(20);$status=$null
        do{Start-Sleep -Milliseconds 250;try{$status=Invoke-Json (Join-Path $resolvedRoot 'client.exe') @('--format','json','status','--root',$resolvedRoot)}catch{$status=$null}}while(($null -eq $status -or -not $status.service_healthy) -and [DateTimeOffset]::UtcNow -lt $deadline)
        if($null -eq $status -or -not $status.service_healthy -or $status.owner_sid -cne $userSid){throw 'Installed qinrm worker did not publish a healthy bound heartbeat.'}
        if(-not $resumeComplete){
            $receipt=[ordered]@{schema='rayman.global-install-receipt.v1';source_commit=$ExpectedCommit;goal_id=$GoalId;installed_at_utc=[DateTimeOffset]::UtcNow.ToString('O');owner_sid=$userSid;worker_sha256=$workerHash;client_sha256=$clientHash;task_name=$TaskName;run_level='LeastPrivilege';heartbeat=$status.heartbeat}
            [IO.File]::WriteAllText((Join-Path $resolvedRoot 'install-receipt.json'),($receipt|ConvertTo-Json -Depth 12),[Text.UTF8Encoding]::new($false))
        } else {$receipt=$retainedReceipt}
        $externalIntegrationStarted=$true
        $raymanInstall=Invoke-FixedProcess $pwsh @('-NoProfile','-File',(Join-Path $sourceRoot 'scripts\install-rayman.ps1'),'-Yes') $sourceRoot 1800000
        $raymanReceipt=Get-Content -LiteralPath (Join-Path $env:LOCALAPPDATA 'Rayman\install\receipt.json') -Raw|ConvertFrom-Json -Depth 12
        if($raymanReceipt.version -cne '2.13.0' -or $raymanReceipt.source -cne 'source_fresh_installer'){throw 'Installed Rayman product receipt differs'}
        $python=@(Get-Command python -CommandType Application -ErrorAction Stop|Select-Object -First 1);if($python.Count -ne 1){throw 'Python is required for the official SaveStatus publisher'}
        $saveInstall=Invoke-Json $python[0].Source @((Join-Path $saveRoot 'save-work-status\scripts\install_skill.py'),'--source-skill-dir',(Join-Path $saveRoot 'save-work-status'),'--target','codex') 1800000 $saveRoot
        if(-not $saveInstall.ok -or $saveInstall.skill_version -cne '2.8.0' -or $saveInstall.installed.codex.source_git_commit -cne $ExpectedSaveStatusCommit){throw 'Installed SaveStatus product identity differs'}
        $configure=Join-Path $PSScriptRoot 'configure-global-codex-execution.ps1'
        $configurationPlan=Invoke-Json $pwsh @('-NoProfile','-File',$configure,'-Plan')
        $configurationResult=Invoke-Json $pwsh @('-NoProfile','-File',$configure,'-Yes')
        $repair=Join-Path $env:USERPROFILE '.codex\skills\save-work-status\watchdog\Repair-CodexResumeRegistrar.ps1'
        if(Test-Path -LiteralPath $repair -PathType Leaf){
            [void](Invoke-Json $pwsh @('-NoProfile','-File',$repair,'-Simulate','-RefreshCodexTrust','-RefreshSaveRuntimeTrust') 900000)
            [void](Invoke-Json $pwsh @('-NoProfile','-File',$repair,'-RefreshCodexTrust','-RefreshSaveRuntimeTrust') 900000)
        }
        $skillResult=Invoke-Json (Join-Path $resolvedRoot 'client.exe') @('--format','json','publish-skill','--root',$resolvedRoot,'--yes')
        if(-not $skillResult.published){throw 'Global entrypoint publication did not verify.'}
        $enroll=Join-Path $PSScriptRoot 'enroll-global-codex-projects.ps1'
        $rollout=Invoke-Json $pwsh @('-NoProfile','-File',$enroll,'-Yes','-MigrateState','-Root',$resolvedRoot,'-RaymanSource',$sourceRoot,'-SaveStatusSource',$saveRoot) 3600000 $sourceRoot
        if(-not $rollout.applied -or -not $rollout.state_routes_published){throw 'Global project enrollment/state migration did not complete'}
        [ordered]@{schema='rayman.global-install-result.v2';installed=$true;resumed=$resumed;root=$resolvedRoot;task=$TaskName;owner_sid=$userSid;worker_sha256=$workerHash;client_sha256=$clientHash;rayman=$raymanReceipt;save_status=$saveInstall;rollout=$rollout;product_processes=@{rayman=$raymanInstall};restart_required=$true}|ConvertTo-Json -Depth 20
        return
    }catch{
        if($externalIntegrationStarted){throw "Global integration needs recovery; backend and Codex config retained together to preserve registrar trust. $($_.Exception.Message)"}
        if($resumeComplete){throw "Retained complete global installation failed preflight and was preserved for review. $($_.Exception.Message)"}
        if($taskRegistered){Stop-VerifiedWorkerTask $TaskName $resolvedRoot $userSid}
        if($resumeInitialized){throw "Initialized global installation failed during resume and was preserved for retry. $($_.Exception.Message)"}
        if($null -ne $configurationResult -and $configurationResult.changed -and $configurationResult.backup){
            $config=Join-Path $env:USERPROFILE '.codex\config.toml'
            $current=(Get-FileHash -LiteralPath $config).Hash.ToLowerInvariant()
            if($current -cne $configurationPlan.after_sha256){throw "Global installation failed after Codex config changed concurrently; backend retained for recovery. $($_.Exception.Message)"}
            $rollbackSpare=$config+'.global-install-rollback-'+$transaction
            [IO.File]::Replace([string]$configurationResult.backup,$config,$rollbackSpare,$true)
            if((Get-FileHash -LiteralPath $config).Hash.ToLowerInvariant() -cne $configurationPlan.before_sha256){throw "Global installation and config rollback both failed; backend retained. $($_.Exception.Message)"}
        }
        if($published -and (Test-Path -LiteralPath $resolvedRoot) -and -not(Test-Path -LiteralPath $failed)){Move-Item -LiteralPath $resolvedRoot -Destination $failed}
        elseif((Test-Path -LiteralPath $stage) -and -not(Test-Path -LiteralPath $failed)){Move-Item -LiteralPath $stage -Destination $failed}
        throw "Global installation failed; new artifacts were preserved at $failed. $($_.Exception.Message)"
    }
}

# Simulation owns a new directory and never changes a production task/config.
$simulationRoot=Join-Path ([IO.Path]::GetTempPath()) ('global-execution-simulation-'+[Guid]::NewGuid().ToString('N'))
Assert-OrdinaryPath $simulationRoot
New-Item -ItemType Directory -Path $simulationRoot | Out-Null
Protect-Directory $simulationRoot $identity.User.Value
$queue=Join-Path $simulationRoot 'requests';New-Item -ItemType Directory -Path $queue | Out-Null
Protect-Directory $queue $identity.User.Value -Queue
Copy-Item -LiteralPath $WorkerPath -Destination (Join-Path $simulationRoot 'worker.exe')
Copy-Item -LiteralPath $ClientPath -Destination (Join-Path $simulationRoot 'client.exe')
if((Get-FileHash (Join-Path $simulationRoot 'worker.exe')).Hash.ToLowerInvariant() -cne $workerHash -or (Get-FileHash (Join-Path $simulationRoot 'client.exe')).Hash.ToLowerInvariant() -cne $clientHash){throw 'Simulation copy identity differs'}
$installed=Invoke-Json (Join-Path $simulationRoot 'worker.exe') @('--format','json','initialize','--root',$simulationRoot,'--source-workspace',$repoRoot,'--yes')
if($installed.owner_sid -cne $identity.User.Value -or $installed.worker_sha256 -cne $workerHash){throw 'Simulation ownership or worker identity differs'}
$workspace=Join-Path $simulationRoot 'workspace';New-Item -ItemType Directory -Path $workspace | Out-Null
foreach($arguments in @(@('init','-b','main'),@('config','user.name','Fixture'),@('config','user.email','fixture@example.invalid'),@('config','core.autocrlf','false'))){
    & $GitPath -C $workspace @arguments | Out-Null;if($LASTEXITCODE -ne 0){throw 'Fixture Git setup failed'}
}
[IO.File]::WriteAllBytes((Join-Path $workspace 'source.txt'),[Text.UTF8Encoding]::new($false).GetBytes("old`n"))
[IO.File]::WriteAllText((Join-Path $workspace '.gitignore'),".RaymanCodingSkill/`n",[Text.UTF8Encoding]::new($false))
& $GitPath -C $workspace add -- source.txt .gitignore;if($LASTEXITCODE -ne 0){throw 'Fixture staging failed'}
& $GitPath -C $workspace -c commit.gpgsign=false commit -m initial | Out-Null;if($LASTEXITCODE -ne 0){throw 'Fixture baseline commit failed'}
$before=(& $GitPath -C $workspace rev-parse HEAD).Trim()
[IO.File]::WriteAllBytes((Join-Path $workspace 'source.txt'),[Text.UTF8Encoding]::new($false).GetBytes("new`n"))
$enrollment=Invoke-Json (Join-Path $simulationRoot 'worker.exe') @('--format','json','enroll','--root',$simulationRoot,'--workspace',$workspace,'--git',$GitPath,'--author-name','Fixture','--author-email','fixture@example.invalid','--formal-state','--yes')
$preview=Invoke-Json (Join-Path $simulationRoot 'client.exe') @('--format','json','commit','--root',$simulationRoot,'--workspace',$workspace,'--path','source.txt','--message','global pipeline smoke')
if(-not $preview.preview -or $preview.executed){throw 'Commit preview unexpectedly executed'}
$requestPath=Join-Path $queue ($preview.request.request_id+'.request.json')
# Same serializer shape as the client; this is isolated test input only.
$requestJson=$preview.request | ConvertTo-Json -Depth 30 -Compress
[IO.File]::WriteAllText($requestPath,$requestJson,[Text.UTF8Encoding]::new($false))
$processInfo=[Diagnostics.ProcessStartInfo]::new();$processInfo.FileName=Join-Path $simulationRoot 'worker.exe';$processInfo.UseShellExecute=$false;$processInfo.CreateNoWindow=$true;$processInfo.RedirectStandardOutput=$true;$processInfo.RedirectStandardError=$true
foreach($arg in @('serve','--root',$simulationRoot,'--once')){$processInfo.ArgumentList.Add($arg)}
$process=[Diagnostics.Process]::Start($processInfo)
try{$out=$process.StandardOutput.ReadToEndAsync();$err=$process.StandardError.ReadToEndAsync();if(-not $process.WaitForExit(30000)){$process.Kill($true);$process.WaitForExit();throw 'Simulation worker timed out'};$stderr=$err.GetAwaiter().GetResult();[void]$out.GetAwaiter().GetResult();if($process.ExitCode -ne 0){throw "Simulation worker failed: $stderr"}}finally{$process.Dispose()}
$reply=Get-Content -LiteralPath (Join-Path $simulationRoot ('result-'+$preview.request.request_id+'.json')) -Raw | ConvertFrom-Json -Depth 30
if(-not $reply.success -or $reply.output.result.status -cne 'effect_succeeded'){throw "Simulation commit failed: $($reply.error)"}
$queried=Invoke-Json (Join-Path $simulationRoot 'client.exe') @('--format','json','result','--root',$simulationRoot,'--request-id',$preview.request.request_id)
if(-not $queried.completed -or $queried.result.result.status -cne 'effect_succeeded'){throw 'Simulation client could not verify the worker result'}
$after=(& $GitPath -C $workspace rev-parse HEAD).Trim()
if($after -ceq $before -or @(& $GitPath -C $workspace status --porcelain).Count){throw 'Simulation commit did not leave an exact clean new HEAD'}
$stateBridgePassed=$false
if($RaymanPath){
    $app=[IO.Path]::GetFullPath($RaymanPath);Assert-OrdinaryPath $app
    Push-Location $workspace
    try{& $app workspace activate --skill-file (Join-Path $repoRoot 'SKILL.md') --yes | Out-Null;if($LASTEXITCODE -ne 0){throw 'Application fixture activation failed'}}finally{Pop-Location}
    [void](Invoke-Json (Join-Path $simulationRoot 'worker.exe') @('--format','json','enroll','--root',$simulationRoot,'--workspace',$workspace,'--git',$GitPath,'--author-name','Fixture','--author-email','fixture@example.invalid','--formal-state','--yes'))
    [void](Invoke-Json (Join-Path $simulationRoot 'worker.exe') @('--format','json','activate-rayman-state','--root',$simulationRoot,'--workspace',$workspace,'--yes'))
    $service=Start-Process -FilePath (Join-Path $simulationRoot 'worker.exe') -ArgumentList @('serve','--root',$simulationRoot) -WindowStyle Hidden -PassThru
    $previousEndpoint=$env:RAYMAN_GLOBAL_EXECUTION_ROOT
    try{
        $env:RAYMAN_GLOBAL_EXECUTION_ROOT=$simulationRoot
        $largeSource=(0..1999 | ForEach-Object {"pub fn sample_$_() -> usize { $_ }"}) -join "`n"
        [IO.File]::WriteAllText((Join-Path $workspace 'source.rs'),$largeSource,[Text.UTF8Encoding]::new($false))
        Push-Location $workspace
        try{
            & $app context refresh | Out-Null;if($LASTEXITCODE -ne 0){throw 'Application context bridge failed'}
            $goal=Invoke-Json $app @('--format','json','goal','start','Global storage simulation','--must-proof','documentation::simulation')
            & $app goal close $goal.id --status partial | Out-Null;if($LASTEXITCODE -ne 0){throw 'Application Goal bridge failed'}
            if((Get-Item -LiteralPath (Join-Path $workspace '.RaymanCodingSkill/context/index.json')).Length -le 65536){throw 'Large state fixture did not exercise chunking'}
            $stateBridgePassed=$true
        }finally{Pop-Location}
    }finally{
        $env:RAYMAN_GLOBAL_EXECUTION_ROOT=$previousEndpoint
        if(-not $service.HasExited){$service.Kill($true);$service.WaitForExit()};$service.Dispose()
    }
}
[ordered]@{schema='rayman.global-install-simulation.v1';initialization_passed=$true;registered_commit_passed=$true;application_state_bridge_passed=$stateBridgePassed;head_before=$before;head_after=$after;production_installed=$false;simulation_root=$simulationRoot;executor_sid=$identity.User.Value;installation=$installed;plan=$description} | ConvertTo-Json -Depth 10
