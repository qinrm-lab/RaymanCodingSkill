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
    [Parameter(ParameterSetName='Install')][string]$WorkerSourceRoot,
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
    $document=$Report.document
    $source=if($null -ne $document){$document.source}else{$null}
    $task=if($null -ne $document){$document.task}else{$null}
    $errors=@(if($null -ne $document -and $document.PSObject.Properties['errors']){$document.errors})
    $taskIssues=@(if($null -ne $task -and $task.PSObject.Properties['issues']){$task.issues})
    if($Report.exit_code -notin @(0,1) -or $null -eq $document -or $document.profile -cne 'release' -or
       $null -eq $task -or $task.goal_id -cne $GoalId -or -not $task.ready -or $taskIssues.Count -or
       $null -eq $source -or -not $source.clean -or $source.head -cne $ExpectedCommit -or
       ($source.PSObject.Properties['error'] -and $null -ne $source.error) -or $errors.Count){
        throw "Source Goal is not ready: exit=$($Report.exit_code) stdout=$($Report.stdout) stderr=$($Report.stderr)"
    }
    # Exit 1 is the aggregate signal for unrelated current Goals or pending
    # packages. The selected Goal and exact source must still be fully ready.
    $document
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
function Get-ElementChildren($Node){@($Node.ChildNodes|Where-Object NodeType -eq ([Xml.XmlNodeType]::Element))}
function Assert-TaskChildren($Node,[string[]]$Allowed,[string]$Label,[string]$Namespace){
    foreach($child in @(Get-ElementChildren $Node)){
        if($child.NamespaceURI -cne $Namespace -or $child.LocalName -notin $Allowed){throw "Live worker task contains an invalid $Label element: $($child.LocalName)"}
    }
}
function Resolve-WorkerAccountSid([string]$Account){
    if([string]::IsNullOrWhiteSpace($Account)){throw 'Task account is empty.'}
    if($Account -match '^S-1-'){
        return [Security.Principal.SecurityIdentifier]::new($Account).Value
    }
    return ([Security.Principal.NTAccount]::new($Account)).Translate([Security.Principal.SecurityIdentifier]).Value
}
function Assert-WorkerTaskXml([string]$Xml,[string]$Program,[string]$Arguments,[string]$Account){
    [xml]$task=$Xml
    $taskNamespace='http://schemas.microsoft.com/windows/2004/02/mit/task'
    if($task.DocumentElement.LocalName -cne 'Task' -or $task.DocumentElement.NamespaceURI -cne $taskNamespace){throw 'Live worker task has an invalid root or namespace.'}
    $ns=[Xml.XmlNamespaceManager]::new($task.NameTable);$ns.AddNamespace('t',$taskNamespace)
    $one={param([string]$XPath,[string]$Label);$nodes=@($task.SelectNodes($XPath,$ns));if($nodes.Count -ne 1){throw "Live worker task must contain exactly one $Label"};$nodes[0]}
    foreach($setting in @('DisallowStartIfOnBatteries','StopIfGoingOnBatteries')){
        $node=& $one ('/t:Task/t:Settings/t:'+$setting) $setting
        if([string]$node.InnerText -cne 'false'){throw ('Worker battery continuity setting is unsafe: '+$setting)}
    }
    $principals=& $one '/t:Task/t:Principals' 'Principals';$principal=& $one '/t:Task/t:Principals/t:Principal' 'Principal'
    $actions=& $one '/t:Task/t:Actions' 'Actions';$exec=& $one '/t:Task/t:Actions/t:Exec' 'Exec action'
    $triggers=& $one '/t:Task/t:Triggers' 'Triggers';$logonTrigger=& $one '/t:Task/t:Triggers/t:LogonTrigger' 'LogonTrigger'
    Assert-TaskChildren $principals @('Principal') 'Principals' $taskNamespace
    Assert-TaskChildren $principal @('UserId','LogonType','RunLevel','DisplayName') 'Principal' $taskNamespace
    Assert-TaskChildren $actions @('Exec') 'Actions' $taskNamespace
    Assert-TaskChildren $exec @('Command','Arguments','WorkingDirectory') 'Exec' $taskNamespace
    Assert-TaskChildren $triggers @('LogonTrigger') 'Triggers' $taskNamespace
    foreach($foreign in @($principals.SelectNodes('.//*')+$actions.SelectNodes('.//*')+$triggers.SelectNodes('.//*'))){if($foreign.NamespaceURI -cne $taskNamespace){throw 'Live worker task contains a foreign-namespace contract element.'}}
    $read={param([string]$XPath,[string]$Label);[string](& $one $XPath $Label).InnerText}
    $command=& $read '/t:Task/t:Actions/t:Exec/t:Command' 'Command';$actualArguments=& $read '/t:Task/t:Actions/t:Exec/t:Arguments' 'Arguments'
    $user=& $read '/t:Task/t:Principals/t:Principal/t:UserId' 'principal UserId';$logon=& $read '/t:Task/t:Principals/t:Principal/t:LogonType' 'LogonType'
    $triggerUser=& $read '/t:Task/t:Triggers/t:LogonTrigger/t:UserId' 'trigger UserId'
    $runLevels=@($task.SelectNodes('/t:Task/t:Principals/t:Principal/t:RunLevel',$ns))
    if($runLevels.Count -gt 1 -or ($runLevels.Count -eq 1 -and [string]$runLevels[0].InnerText -cne 'LeastPrivilege')){throw 'Live worker task has an elevated, duplicate, or invalid RunLevel.'}
    $working=@($task.SelectNodes('/t:Task/t:Actions/t:Exec/t:WorkingDirectory',$ns))
    if($working.Count -gt 1 -or ($working.Count -eq 1 -and -not [string]::IsNullOrWhiteSpace([string]$working[0].InnerText))){throw 'Live worker task has an unexpected working directory.'}
    $principalId=[string]$principal.GetAttribute('id');$context=[string]$actions.GetAttribute('Context')
    if(($principalId.Length -eq 0) -ne ($context.Length -eq 0) -or ($principalId.Length -gt 0 -and $context -cne $principalId)){throw 'Live worker task action context differs from its principal.'}
    $expectedSid=Resolve-WorkerAccountSid $Account
    $principalSid=Resolve-WorkerAccountSid $user
    $triggerSid=Resolve-WorkerAccountSid $triggerUser
    if(-not $command.Equals($Program,[StringComparison]::OrdinalIgnoreCase) -or $actualArguments -cne $Arguments -or
       $principalSid -cne $expectedSid -or $triggerSid -cne $expectedSid -or $logon -cne 'InteractiveToken'){
        throw 'Live worker task differs from its fixed least-privilege contract.'
    }
}
function Get-WorkerTaskProbe([string]$Name){
    try{return [pscustomobject]@{kind='present';task=Get-ScheduledTask -TaskName $Name -ErrorAction Stop;error=$null}}
    catch{
        if($_.CategoryInfo.Category -eq [Management.Automation.ErrorCategory]::ObjectNotFound -or $_.Exception.HResult -eq -2147024894){return [pscustomobject]@{kind='absent';task=$null;error=$null}}
        return [pscustomobject]@{kind='unavailable';task=$null;error=$_.Exception.Message}
    }
}
function Assert-WorkerSourceInputs([string]$Original,[string]$Current,[string]$Git,[string]$WorkerHash,[string]$ClientHash){
    foreach($rootPath in @($Original,$Current)){
        Assert-OrdinaryPath $rootPath
        $dirty=@(& $Git -C $rootPath status --porcelain=v1 --untracked-files=all)
        if($LASTEXITCODE -ne 0 -or $dirty.Count){throw "Worker source must be readable and clean: $rootPath"}
    }
    # Include the worker crate and its compile-time included resources as well
    # as workspace dependency/build configuration. Compare raw working bytes.
    $inputs=@('Cargo.toml','Cargo.lock','.cargo','crates/codex-global-execution','scripts/install-rayman.ps1','global-skill')
    $originalFiles=@(& $Git -C $Original ls-files -- @inputs)
    if($LASTEXITCODE -ne 0 -or $originalFiles.Count -eq 0){throw 'Original worker source inventory is unavailable.'}
    $currentFiles=@(& $Git -C $Current ls-files -- @inputs)
    if($LASTEXITCODE -ne 0 -or (($originalFiles|Sort-Object) -join "`n") -cne (($currentFiles|Sort-Object) -join "`n")){throw 'Worker source inventory differs.'}
    foreach($relative in $originalFiles){
        $oldPath=Join-Path $Original $relative;$newPath=Join-Path $Current $relative
        Assert-OrdinaryPath $oldPath;Assert-OrdinaryPath $newPath
        if((Get-FileHash -LiteralPath $oldPath).Hash -cne (Get-FileHash -LiteralPath $newPath).Hash){throw "Worker build input differs: $relative"}
    }
    foreach($pair in @(@('target/release/rayman-global-worker.exe',$WorkerHash),@('target/release/rayman-global.exe',$ClientHash))){
        $artifact=Join-Path $Original $pair[0];Assert-OrdinaryPath $artifact
        if((Get-FileHash -LiteralPath $artifact).Hash.ToLowerInvariant() -cne $pair[1]){throw 'Original worker build artifact differs from the retained installation.'}
    }
}
function Get-InitializedResume([string]$InstallRoot,[string]$OwnerSid,[string]$ExpectedWorkerHash,[string]$ExpectedClientHash){
    Assert-OrdinaryPath $InstallRoot
    $known=@('.installation.rayman.lock','.worker.rayman.lock','.registry.rayman.lock','client.exe','heartbeat.json','install-progress.json','install-receipt.json','installation.json','queue-health.json','requests','worker.exe')
    $required=@('.installation.rayman.lock','client.exe','installation.json','requests','worker.exe')
    $actual=@(Get-ChildItem -LiteralPath $InstallRoot -Force|ForEach-Object Name|Sort-Object)
    foreach($name in $required){if($name -notin $actual){throw "Global installation is missing required entry: $name"}}
    foreach($name in $actual){if($name -notin $known){throw "Global installation contains an unexpected entry: $name"}}
    if(-not(Test-Path -LiteralPath (Join-Path $InstallRoot 'requests') -PathType Container) -or @(Get-ChildItem -LiteralPath (Join-Path $InstallRoot 'requests') -Force).Count){throw 'Global installation request queue is missing or non-empty.'}
    $actualOwner=([Security.Principal.NTAccount]::new((Get-Acl -LiteralPath $InstallRoot).Owner)).Translate([Security.Principal.SecurityIdentifier]).Value
    if($actualOwner -cne $OwnerSid){throw 'Global installation owner differs.'}
    if((Get-FileHash (Join-Path $InstallRoot 'worker.exe')).Hash.ToLowerInvariant() -cne $ExpectedWorkerHash -or (Get-FileHash (Join-Path $InstallRoot 'client.exe')).Hash.ToLowerInvariant() -cne $ExpectedClientHash){throw 'Global installation executable identity differs.'}
    $installed=Get-Content -Raw -LiteralPath (Join-Path $InstallRoot 'installation.json')|ConvertFrom-Json -Depth 12
    if($installed.schema_version -ne 1 -or $installed.owner_sid -cne $OwnerSid -or $installed.worker_sha256 -cne $ExpectedWorkerHash -or $installed.client_sha256 -cne $ExpectedClientHash -or
       [string]$installed.installation_id -cnotmatch '^[0-9a-f]{32}$' -or [string]$installed.root_identity -cnotmatch '^[0-9a-f]{64}$' -or [string]$installed.source_project_id -cnotmatch '^[0-9a-f]{32}$'){throw 'Global installation receipt differs.'}
    [pscustomobject]@{installation=$installed;entries=$actual}
}
function Assert-RuntimeStatus($Status,$Installation,[string]$OwnerSid){
    if($Status.installation_verified -ne $true -or $Status.owner_sid -cne $OwnerSid){throw 'Global client did not attest the installation owner, DACL and root identity.'}
    if($null -ne $Status.queue -and @($Status.queue.problems).Count){throw 'Global request queue reports an unresolved problem.'}
    if($null -ne $Status.heartbeat -and ($Status.heartbeat.schema -cne 'rayman.global-heartbeat.v1' -or $Status.heartbeat.installation_id -cne $Installation.installation_id -or
       $Status.heartbeat.executor_sid -cne $OwnerSid -or [int64]$Status.heartbeat.pid -le 0 -or $null -ne $Status.heartbeat.error)){throw 'Global worker heartbeat differs from the exact installation identity.'}
}
function Get-IntegrationProgress([string]$InstallRoot,[string]$ExpectedCommit,[string]$GoalId,[string]$WorkerHash,[string]$ClientHash){
    $path=Join-Path $InstallRoot 'install-progress.json'
    if(-not(Test-Path -LiteralPath $path -PathType Leaf)){return [pscustomobject]@{completed_steps=@()}}
    $progress=Get-Content -LiteralPath $path -Raw|ConvertFrom-Json -Depth 12;$allowed=@('rayman','save_status','config','registrar','skill','enrollment');$steps=@($progress.completed_steps)
    if($progress.schema -cne 'rayman.global-install-progress.v1' -or $progress.source_commit -cne $ExpectedCommit -or $progress.goal_id -cne $GoalId -or
       $progress.worker_sha256 -cne $WorkerHash -or $progress.client_sha256 -cne $ClientHash -or $steps.Count -gt $allowed.Count -or (($steps -join ',') -cne (($allowed|Select-Object -First $steps.Count) -join ','))){throw 'Global integration progress is malformed, reordered, or bound to another rollout.'}
    $progress
}
function Resolve-GlobalInstallPhase([bool]$RootPresent,[string]$TaskKind,[int]$RuntimeArtifacts,[bool]$ReceiptPresent,[int]$CompletedSteps,[bool]$ServiceHealthy){
    if($TaskKind -eq 'unavailable'){throw 'Task Scheduler state is unavailable; absence cannot be inferred.'}
    if(-not $RootPresent){if($TaskKind -ne 'absent'){throw 'Global execution task exists without its exact installation root.'};return 'fresh'}
    if($RuntimeArtifacts -notin @(0,3)){throw 'Global runtime artifacts are only partially present.'}
    if($CompletedSteps -gt 0 -and -not $ReceiptPresent){throw 'Global integration progress exists without a final backend receipt.'}
    if($TaskKind -eq 'absent'){
        if($RuntimeArtifacts -ne 0 -or $ReceiptPresent -or $CompletedSteps){throw 'Global installation has runtime evidence without its scheduled task.'}
        return 'initialized'
    }
    if($ReceiptPresent){
        if($RuntimeArtifacts -ne 3){throw 'Global backend receipt exists without complete runtime evidence.'}
        if($CompletedSteps -ge 6){return 'complete'}
        if($CompletedSteps -gt 0){return 'integration_started'}
        return 'receipt_published'
    }
    if($RuntimeArtifacts -eq 3 -and $ServiceHealthy){return 'worker_started'}
    'task_registered'
}
function Get-GlobalInstallState([string]$InstallRoot,[string]$Name,[string]$OwnerSid,[string]$ExpectedWorkerHash,[string]$ExpectedClientHash,[string]$ExpectedCommit,[string]$GoalId,[string]$SourceRoot){
    $probe=Get-WorkerTaskProbe $Name
    if($probe.kind -eq 'unavailable'){throw "Task Scheduler query failed closed: $($probe.error)"}
    $present=Test-Path -LiteralPath $InstallRoot -PathType Container
    if(-not $present){return [pscustomobject]@{phase=(Resolve-GlobalInstallPhase $false $probe.kind 0 $false 0 $false);task_probe=$probe;installation=$null;status=$null;progress=[pscustomobject]@{completed_steps=@()};receipt=$null}}
    $core=Get-InitializedResume $InstallRoot $OwnerSid $ExpectedWorkerHash $ExpectedClientHash
    $entries=@($core.entries);$runtimeCount=@('.worker.rayman.lock','heartbeat.json','queue-health.json'|Where-Object {$_ -in $entries}).Count;$receiptPresent='install-receipt.json' -in $entries
    if($probe.kind -eq 'present'){Assert-WorkerTaskXml (Export-ScheduledTask -TaskName $Name) (Join-Path $InstallRoot 'worker.exe') (Get-WorkerTaskArguments $InstallRoot) $OwnerSid}
    $status=$null
    if($runtimeCount -eq 3){$status=Invoke-Json (Join-Path $InstallRoot 'client.exe') @('--format','json','status','--root',$InstallRoot);Assert-RuntimeStatus $status $core.installation $OwnerSid}
    $source=Invoke-Json (Join-Path $InstallRoot 'client.exe') @('--format','json','enroll','--root',$InstallRoot,'--workspace',$SourceRoot,'--preflight')
    if($source.registration.project_id -cne $core.installation.source_project_id){throw 'Global installation source project identity differs from the explicitly verified worker source Git common directory.'}
    $receipt=$null
    if($receiptPresent){
        $receipt=Get-Content -LiteralPath (Join-Path $InstallRoot 'install-receipt.json') -Raw|ConvertFrom-Json -Depth 12
        if($receipt.schema -cne 'rayman.global-install-receipt.v1' -or $receipt.source_commit -cne $ExpectedCommit -or $receipt.goal_id -cne $GoalId -or $receipt.owner_sid -cne $OwnerSid -or
           $receipt.worker_sha256 -cne $ExpectedWorkerHash -or $receipt.client_sha256 -cne $ExpectedClientHash -or $receipt.task_name -cne $Name -or $receipt.run_level -cne 'LeastPrivilege'){throw 'Retained global installation receipt differs from this rollout.'}
    }
    $progress=Get-IntegrationProgress $InstallRoot $ExpectedCommit $GoalId $ExpectedWorkerHash $ExpectedClientHash
    $phase=Resolve-GlobalInstallPhase $true $probe.kind $runtimeCount $receiptPresent @($progress.completed_steps).Count ($null -ne $status -and $status.service_healthy)
    [pscustomobject]@{phase=$phase;task_probe=$probe;installation=$core.installation;status=$status;progress=$progress;receipt=$receipt}
}
function Write-IntegrationProgress([string]$InstallRoot,[string]$ExpectedCommit,[string]$GoalId,[string]$WorkerHash,[string]$ClientHash,[string[]]$CompletedSteps){
    $value=[ordered]@{schema='rayman.global-install-progress.v1';source_commit=$ExpectedCommit;goal_id=$GoalId;worker_sha256=$WorkerHash;client_sha256=$ClientHash;completed_steps=@($CompletedSteps);updated_at_utc=[DateTimeOffset]::UtcNow.ToString('O')}
    [IO.File]::WriteAllText((Join-Path $InstallRoot 'install-progress.json'),($value|ConvertTo-Json -Depth 8),[Text.UTF8Encoding]::new($false))
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
    & {
        $fixture=Join-Path ([IO.Path]::GetTempPath()) ('worker-source-test-'+[guid]::NewGuid().ToString('N'))
        [void][IO.Directory]::CreateDirectory($fixture)
        try{
            foreach($name in @('original','current')){
                $dir=Join-Path $fixture $name;[void][IO.Directory]::CreateDirectory($dir)
                & $GitPath -C $dir init -q
                if($LASTEXITCODE -ne 0){throw 'Fixture Git initialization failed.'}
                [IO.File]::WriteAllText((Join-Path $dir 'Cargo.toml'),"# fixed source`n",[Text.UTF8Encoding]::new($false))
                [IO.File]::WriteAllText((Join-Path $dir '.gitignore'),"target/`n",[Text.UTF8Encoding]::new($false))
                & $GitPath -C $dir add -- Cargo.toml .gitignore
                if($LASTEXITCODE -ne 0){throw 'Fixture staging failed.'}
                & $GitPath -C $dir -c user.name=Fixture -c user.email=fixture@example.invalid -c commit.gpgsign=false commit -qm baseline
                if($LASTEXITCODE -ne 0){throw 'Fixture commit failed.'}
                [void][IO.Directory]::CreateDirectory((Join-Path $dir 'target/release'))
                foreach($binary in @('rayman-global-worker.exe','rayman-global.exe')){[IO.File]::WriteAllBytes((Join-Path $dir ('target/release/'+$binary)),[byte[]](1,2,3))}
            }
            $original=Join-Path $fixture 'original';$current=Join-Path $fixture 'current'
            $hash=(Get-FileHash (Join-Path $original 'target/release/rayman-global.exe')).Hash.ToLowerInvariant()
            Assert-WorkerSourceInputs $original $current $GitPath $hash $hash
            $rejected=$false;try{Assert-WorkerSourceInputs $original $current $GitPath ('0'*64) $hash}catch{$rejected=$true}
            if(-not $rejected){throw 'Mismatched original build artifact accepted.'}
            [IO.File]::AppendAllText((Join-Path $current 'Cargo.toml'),"# drift`n")
            $rejected=$false;try{Assert-WorkerSourceInputs $original $current $GitPath $hash $hash}catch{$rejected=$true}
            if(-not $rejected){throw 'Dirty worker source accepted.'}
            & $GitPath -C $current add -- Cargo.toml
            if($LASTEXITCODE -ne 0){throw 'Fixture staging failed.'}
            & $GitPath -C $current -c user.name=Fixture -c user.email=fixture@example.invalid -c commit.gpgsign=false commit -qm drift
            if($LASTEXITCODE -ne 0){throw 'Fixture commit failed.'}
            $rejected=$false;try{Assert-WorkerSourceInputs $original $current $GitPath $hash $hash}catch{$rejected=$true}
            if(-not $rejected){throw 'Different committed worker source accepted.'}
        }finally{
            $base=[IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')+'\'
            if(-not [IO.Path]::GetFullPath($fixture).StartsWith($base,[StringComparison]::OrdinalIgnoreCase)){throw 'Unsafe source fixture cleanup path.'}
            Assert-OrdinaryPath $fixture
            Remove-Item -LiteralPath $fixture -Recurse -Force
        }
    }
    $fixtureAccount=[Security.Principal.WindowsIdentity]::GetCurrent().Name
    $arguments=Get-WorkerTaskArguments 'C:\ProgramData\Rayman\CodexGlobalExecution'
    if($arguments -cne 'serve --root "C:\ProgramData\Rayman\CodexGlobalExecution"'){throw 'Task argument rendering differs'}
    $xml='<?xml version="1.0"?><Task xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task"><Settings><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries></Settings><Triggers><LogonTrigger><UserId>QIN5521\qinrm</UserId></LogonTrigger></Triggers><Principals><Principal id="Author"><UserId>QIN5521\qinrm</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals><Actions Context="Author"><Exec><Command>C:\ProgramData\Rayman\CodexGlobalExecution\worker.exe</Command><Arguments>serve --root "C:\ProgramData\Rayman\CodexGlobalExecution"</Arguments></Exec></Actions></Task>'
    $xml=$xml.Replace('QIN5521\qinrm',$fixtureAccount)
    Assert-WorkerTaskXml $xml 'C:\ProgramData\Rayman\CodexGlobalExecution\worker.exe' $arguments $fixtureAccount
    Assert-WorkerTaskXml ($xml.Replace('<RunLevel>LeastPrivilege</RunLevel>','')) 'C:\ProgramData\Rayman\CodexGlobalExecution\worker.exe' $arguments $fixtureAccount
    $fixtureSid=Resolve-WorkerAccountSid $fixtureAccount
    $mixedXml=$xml.Replace(('<Principal id="Author"><UserId>'+$fixtureAccount+'</UserId>'),('<Principal id="Author"><UserId>'+$fixtureSid+'</UserId>')).Replace('<RunLevel>LeastPrivilege</RunLevel>','')
    Assert-WorkerTaskXml $mixedXml 'C:\ProgramData\Rayman\CodexGlobalExecution\worker.exe' $arguments $fixtureSid
    Assert-WorkerTaskXml ($xml.Replace($fixtureAccount,$fixtureSid)) 'C:\ProgramData\Rayman\CodexGlobalExecution\worker.exe' $arguments $fixtureAccount
    foreach($badAccount in @('S-1-5-18','S-1-5-','RaymanNonexistentTaskAccount-19da')){
        $badXml=$mixedXml.Replace(('<LogonTrigger><UserId>'+$fixtureAccount+'</UserId>'),('<LogonTrigger><UserId>'+$badAccount+'</UserId>'))
        $rejected=$false
        try{Assert-WorkerTaskXml $badXml 'C:\ProgramData\Rayman\CodexGlobalExecution\worker.exe' $arguments $fixtureSid}catch{$rejected=$true}
        if(-not $rejected){throw 'Mismatched or unresolved trigger account was accepted.'}
    }
    foreach($invalidXml in @(
        $xml.Replace('LeastPrivilege','HighestAvailable'),$xml.Replace('</RunLevel>','</RunLevel><RunLevel>LeastPrivilege</RunLevel>'),
        $xml.Replace('<Command>C:\ProgramData\Rayman\CodexGlobalExecution\worker.exe</Command>',''),
        $xml.Replace('</Exec>','</Exec><Exec><Command>x</Command><Arguments>y</Arguments></Exec>'),
        $xml.Replace('<Actions Context="Author">','<Actions Context="Other">'),
        $xml.Replace('<RunLevel>LeastPrivilege</RunLevel>','<RunLevel xmlns="urn:foreign">LeastPrivilege</RunLevel>'),
        $xml.Replace('</Principals>','<Principal><UserId>other</UserId><LogonType>InteractiveToken</LogonType></Principal></Principals>'),
        $xml.Replace('</LogonTrigger>',('</LogonTrigger><LogonTrigger><UserId>'+$fixtureAccount+'</UserId></LogonTrigger>'))
    )){$rejected=$false;try{Assert-WorkerTaskXml $invalidXml 'C:\ProgramData\Rayman\CodexGlobalExecution\worker.exe' $arguments $fixtureAccount}catch{$rejected=$true};if(-not $rejected){throw 'Invalid materialized task XML was accepted'}}
    foreach($setting in @('DisallowStartIfOnBatteries','StopIfGoingOnBatteries')){
        $good='<'+$setting+'>false</'+$setting+'>'
        foreach($badXml in @($xml.Replace($good,$good.Replace('false','true')),$xml.Replace($good,''),$xml.Replace($good,$good+$good),$xml.Replace($good,$good.Replace('false','invalid')))){
            $rejected=$false
            try{Assert-WorkerTaskXml $badXml 'C:\ProgramData\Rayman\CodexGlobalExecution\worker.exe' $arguments $fixtureAccount}catch{$rejected=$true}
            if(-not $rejected){throw ('Unsafe battery task setting accepted: '+$setting)}
        }
    }
    $cases=@(
        @{root=$false;task='absent';runtime=0;receipt=$false;steps=0;healthy=$false;phase='fresh'},
        @{root=$true;task='absent';runtime=0;receipt=$false;steps=0;healthy=$false;phase='initialized'},
        @{root=$true;task='present';runtime=0;receipt=$false;steps=0;healthy=$false;phase='task_registered'},
        @{root=$true;task='present';runtime=3;receipt=$false;steps=0;healthy=$false;phase='task_registered'},
        @{root=$true;task='present';runtime=3;receipt=$false;steps=0;healthy=$true;phase='worker_started'},
        @{root=$true;task='present';runtime=3;receipt=$true;steps=0;healthy=$true;phase='receipt_published'},
        @{root=$true;task='present';runtime=3;receipt=$true;steps=3;healthy=$true;phase='integration_started'},
        @{root=$true;task='present';runtime=3;receipt=$true;steps=6;healthy=$true;phase='complete'}
    )
    foreach($case in $cases){$actual=Resolve-GlobalInstallPhase $case.root $case.task $case.runtime $case.receipt $case.steps $case.healthy;if($actual -cne $case.phase){throw "State simulation differs: expected=$($case.phase) actual=$actual"}}
    $invalidCases=@(
        @($false,'present',0,$false,0,$false),@($false,'unavailable',0,$false,0,$false),@($true,'unavailable',0,$false,0,$false),
        @($true,'absent',1,$false,0,$false),@($true,'absent',3,$false,0,$false),@($true,'absent',0,$true,0,$false),
        @($true,'present',1,$false,0,$false),@($true,'present',2,$false,0,$false),@($true,'present',0,$true,0,$false),
        @($true,'present',3,$false,1,$true),@($true,'absent',0,$false,1,$false)
    )
    foreach($case in $invalidCases){$rejected=$false;try{[void](Resolve-GlobalInstallPhase @case)}catch{$rejected=$true};if(-not $rejected){throw "Invalid state combination was accepted: $($case -join ',')"}}
    & {
        $fixture=Join-Path ([IO.Path]::GetTempPath()) ('global-install-state-test-'+[guid]::NewGuid().ToString('N'));[void][IO.Directory]::CreateDirectory($fixture)
        try{
            $owner=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value;[void][IO.Directory]::CreateDirectory((Join-Path $fixture 'requests'))
            [IO.File]::WriteAllText((Join-Path $fixture '.installation.rayman.lock'),'');[IO.File]::WriteAllBytes((Join-Path $fixture 'worker.exe'),[byte[]](1,2,3));[IO.File]::WriteAllBytes((Join-Path $fixture 'client.exe'),[byte[]](4,5,6))
            $wh=(Get-FileHash (Join-Path $fixture 'worker.exe')).Hash.ToLowerInvariant();$ch=(Get-FileHash (Join-Path $fixture 'client.exe')).Hash.ToLowerInvariant()
            $installation=[ordered]@{schema_version=1;installation_id='a'*32;owner_sid=$owner;root_identity='b'*64;worker_sha256=$wh;client_sha256=$ch;source_project_id='c'*32}
            [IO.File]::WriteAllText((Join-Path $fixture 'installation.json'),($installation|ConvertTo-Json),[Text.UTF8Encoding]::new($false))
            if((Get-InitializedResume $fixture $owner $wh $ch).installation.installation_id -cne 'a'*32){throw 'Initialized fixture was not accepted'}
            [IO.File]::WriteAllText((Join-Path $fixture 'unexpected.txt'),'drift');$rejected=$false;try{[void](Get-InitializedResume $fixture $owner $wh $ch)}catch{$rejected=$true};Remove-Item (Join-Path $fixture 'unexpected.txt') -Force;if(-not $rejected){throw 'Unexpected entry was accepted'}
            [IO.File]::WriteAllText((Join-Path $fixture 'requests\request.json'),'{}');$rejected=$false;try{[void](Get-InitializedResume $fixture $owner $wh $ch)}catch{$rejected=$true};Remove-Item (Join-Path $fixture 'requests\request.json') -Force;if(-not $rejected){throw 'Non-empty queue was accepted'}
            $progress=[ordered]@{schema='rayman.global-install-progress.v1';source_commit='d'*40;goal_id='goal_0123456789';worker_sha256=$wh;client_sha256=$ch;completed_steps=@('rayman','save_status')}
            [IO.File]::WriteAllText((Join-Path $fixture 'install-progress.json'),($progress|ConvertTo-Json),[Text.UTF8Encoding]::new($false))
            if(@((Get-IntegrationProgress $fixture ('d'*40) 'goal_0123456789' $wh $ch).completed_steps).Count -ne 2){throw 'Valid progress was not accepted'}
            $progress.completed_steps=@('save_status','rayman');[IO.File]::WriteAllText((Join-Path $fixture 'install-progress.json'),($progress|ConvertTo-Json),[Text.UTF8Encoding]::new($false))
            $rejected=$false;try{[void](Get-IntegrationProgress $fixture ('d'*40) 'goal_0123456789' $wh $ch)}catch{$rejected=$true};if(-not $rejected){throw 'Reordered progress was accepted'}
        }finally{Remove-Item -LiteralPath $fixture -Recurse -Force}
    }
    $readyHead='a'*40;$readyDocument=[pscustomobject]@{profile='release';task=[pscustomobject]@{goal_id='goal_0123456789';ready=$true;issues=@()};source=[pscustomobject]@{clean=$true;head=$readyHead;error=$null};errors=@()}
    [void](Assert-SourceGoalReady ([pscustomobject]@{exit_code=1;document=$readyDocument;stdout='ready';stderr='other-goal-pending'}) 'goal_0123456789' $readyHead)
    foreach($invalid in @(
        [pscustomobject]@{exit_code=2;document=$readyDocument;stdout='';stderr=''},
        [pscustomobject]@{exit_code=1;document=[pscustomobject]@{profile='release';task=[pscustomobject]@{goal_id='goal_0123456789';ready=$true;issues=@('quality')};source=$readyDocument.source;errors=@()};stdout='';stderr=''},
        [pscustomobject]@{exit_code=1;document=[pscustomobject]@{profile='release';task=$readyDocument.task;source=[pscustomobject]@{clean=$true;head='b'*40;error=$null};errors=@()};stdout='';stderr=''}
    )){$rejected=$false;try{[void](Assert-SourceGoalReady $invalid 'goal_0123456789' $readyHead)}catch{$rejected=$true};if(-not $rejected){throw 'Invalid source Goal report was accepted'}}
    Write-Output 'install-global-codex-execution: PASS (8 legal states, 11 illegal combinations, strict task XML, progress ordering, source readiness)'
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
    $workerSource=if($WorkerSourceRoot){[IO.Path]::GetFullPath($WorkerSourceRoot)}else{$sourceRoot}
    Assert-WorkerSourceInputs $workerSource $sourceRoot $git.Source $workerHash $clientHash
    $existingState=Get-GlobalInstallState $resolvedRoot $TaskName $userSid $workerHash $clientHash $ExpectedCommit $GoalId $workerSource
    $initialPhase=$existingState.phase;$resumeExisting=$initialPhase -ne 'fresh';$resumeInitialized=$initialPhase -eq 'initialized';$retainedReceipt=$existingState.receipt
    $completedSteps=[Collections.Generic.List[string]]::new();foreach($step in @($existingState.progress.completed_steps)){$completedSteps.Add([string]$step)}
    $parent=Split-Path -Parent $resolvedRoot
    [void][IO.Directory]::CreateDirectory($parent)
    Assert-OrdinaryPath $parent
    $transaction=[Guid]::NewGuid().ToString('N')
    $stage=Join-Path $parent ((Split-Path -Leaf $resolvedRoot)+'.install-'+$transaction)
    $failed=Join-Path $parent ((Split-Path -Leaf $resolvedRoot)+'.failed-'+$transaction)
    $taskRegistered=$false;$published=$resumeExisting;$configurationPlan=$null;$configurationResult=$null;$externalIntegrationStarted=$initialPhase -in @('integration_started','complete');$resumed=$resumeExisting
    try{
        if(-not $resumeExisting){
            [void][IO.Directory]::CreateDirectory($stage);Protect-Directory $stage $userSid
            [void][IO.Directory]::CreateDirectory((Join-Path $stage 'requests'));Protect-Directory (Join-Path $stage 'requests') $userSid -Queue
            Copy-Item -LiteralPath $WorkerPath -Destination (Join-Path $stage 'worker.exe')
            Copy-Item -LiteralPath $ClientPath -Destination (Join-Path $stage 'client.exe')
            if((Get-FileHash (Join-Path $stage 'worker.exe')).Hash.ToLowerInvariant() -cne $workerHash -or (Get-FileHash (Join-Path $stage 'client.exe')).Hash.ToLowerInvariant() -cne $clientHash){throw 'Staged executable identity differs.'}
            $installed=Invoke-Json (Join-Path $stage 'worker.exe') @('--format','json','initialize','--root',$stage,'--source-workspace',$workerSource,'--yes')
            if($installed.owner_sid -cne $userSid -or $installed.worker_sha256 -cne $workerHash -or $installed.client_sha256 -cne $clientHash){throw 'Initialized installation identity differs.'}
            Move-Item -LiteralPath $stage -Destination $resolvedRoot;$published=$true
        }
        $worker=Join-Path $resolvedRoot 'worker.exe';$arguments=Get-WorkerTaskArguments $resolvedRoot
        if($initialPhase -in @('fresh','initialized')){
            $action=New-ScheduledTaskAction -Execute $worker -Argument $arguments
            $trigger=New-ScheduledTaskTrigger -AtLogOn -User $UserAccount
            $principal=New-ScheduledTaskPrincipal -UserId $UserAccount -LogonType Interactive -RunLevel Limited
            $settings=New-ScheduledTaskSettingsSet -StartWhenAvailable -MultipleInstances IgnoreNew -ExecutionTimeLimit ([TimeSpan]::Zero) -Hidden -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
            Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Description 'Protected qinrm fixed-capability worker for enrolled Codex projects' | Out-Null
            $taskRegistered=$true
        }
        $task=Get-ScheduledTask -TaskName $TaskName -ErrorAction Stop
        Assert-WorkerTaskXml (Export-ScheduledTask -TaskName $TaskName) $worker $arguments $userSid
        if([string]$task.State -notin @('Running','Queued')){Start-ScheduledTask -TaskName $TaskName}
        $deadline=[DateTimeOffset]::UtcNow.AddSeconds(20);$status=$null
        do{Start-Sleep -Milliseconds 250;try{$status=Invoke-Json (Join-Path $resolvedRoot 'client.exe') @('--format','json','status','--root',$resolvedRoot)}catch{$status=$null}}while(($null -eq $status -or -not $status.service_healthy) -and [DateTimeOffset]::UtcNow -lt $deadline)
        if($null -eq $status -or -not $status.service_healthy){throw 'Installed qinrm worker did not publish a healthy bound heartbeat.'}
        Assert-RuntimeStatus $status (Get-Content -LiteralPath (Join-Path $resolvedRoot 'installation.json') -Raw|ConvertFrom-Json -Depth 12) $userSid
        if($null -eq $retainedReceipt){
            $receipt=[ordered]@{schema='rayman.global-install-receipt.v1';source_commit=$ExpectedCommit;goal_id=$GoalId;installed_at_utc=[DateTimeOffset]::UtcNow.ToString('O');owner_sid=$userSid;worker_sha256=$workerHash;client_sha256=$clientHash;task_name=$TaskName;run_level='LeastPrivilege';heartbeat=$status.heartbeat}
            [IO.File]::WriteAllText((Join-Path $resolvedRoot 'install-receipt.json'),($receipt|ConvertTo-Json -Depth 12),[Text.UTF8Encoding]::new($false))
        } else {$receipt=$retainedReceipt}
        $externalIntegrationStarted=$true
        if('rayman' -in $completedSteps){
            $raymanReceipt=Get-Content -LiteralPath (Join-Path $env:LOCALAPPDATA 'Rayman\install\receipt.json') -Raw|ConvertFrom-Json -Depth 12
            if($raymanReceipt.version -cne '2.13.0' -or $raymanReceipt.source -cne 'source_fresh_installer'){throw 'Completed Rayman progress no longer matches the installed product.'}
            $raymanInstall=[ordered]@{exit_code=0;resumed_verified=$true}
        }else{
            $raymanInstall=Invoke-FixedProcess $pwsh @('-NoProfile','-File',(Join-Path $sourceRoot 'scripts\install-rayman.ps1'),'-Yes') $sourceRoot 1800000
            $raymanReceipt=Get-Content -LiteralPath (Join-Path $env:LOCALAPPDATA 'Rayman\install\receipt.json') -Raw|ConvertFrom-Json -Depth 12
            if($raymanReceipt.version -cne '2.13.0' -or $raymanReceipt.source -cne 'source_fresh_installer'){throw 'Installed Rayman product receipt differs'}
            $completedSteps.Add('rayman');Write-IntegrationProgress $resolvedRoot $ExpectedCommit $GoalId $workerHash $clientHash $completedSteps
        }
        $python=@(Get-Command python -CommandType Application -ErrorAction Stop|Select-Object -First 1);if($python.Count -ne 1){throw 'Python is required for the official SaveStatus publisher'}
        if('save_status' -in $completedSteps){
            $saveManifest=Get-Content -LiteralPath (Join-Path $env:USERPROFILE '.codex\skills\save-work-status\install-manifest.json') -Raw|ConvertFrom-Json -Depth 12
            if($saveManifest.skill_version -cne '2.8.0' -or $saveManifest.source_git_commit -cne $ExpectedSaveStatusCommit){throw 'Completed SaveStatus progress no longer matches the installed product.'}
            $saveInstall=[ordered]@{ok=$true;skill_version='2.8.0';resumed_verified=$true;installed=[ordered]@{codex=[ordered]@{source_git_commit=$ExpectedSaveStatusCommit}}}
        }else{
            $saveInstall=Invoke-Json $python[0].Source @((Join-Path $saveRoot 'save-work-status\scripts\install_skill.py'),'--source-skill-dir',(Join-Path $saveRoot 'save-work-status'),'--target','codex') 1800000 $saveRoot
            if(-not $saveInstall.ok -or $saveInstall.skill_version -cne '2.8.0' -or $saveInstall.installed.codex.source_git_commit -cne $ExpectedSaveStatusCommit){throw 'Installed SaveStatus product identity differs'}
            $completedSteps.Add('save_status');Write-IntegrationProgress $resolvedRoot $ExpectedCommit $GoalId $workerHash $clientHash $completedSteps
        }
        $configure=Join-Path $PSScriptRoot 'configure-global-codex-execution.ps1'
        $configurationPlan=Invoke-Json $pwsh @('-NoProfile','-File',$configure,'-Plan')
        if('config' -in $completedSteps){if($configurationPlan.changed){throw 'Completed config progress no longer matches the fixed global endpoint.'};$configurationResult=[ordered]@{changed=$false;resumed_verified=$true}}
        else{$configurationResult=Invoke-Json $pwsh @('-NoProfile','-File',$configure,'-Yes');$completedSteps.Add('config');Write-IntegrationProgress $resolvedRoot $ExpectedCommit $GoalId $workerHash $clientHash $completedSteps}
        $repair=Join-Path $env:USERPROFILE '.codex\skills\save-work-status\watchdog\Repair-CodexResumeRegistrar.ps1'
        if('registrar' -in $completedSteps){
            if(Test-Path -LiteralPath $repair -PathType Leaf){[void](Invoke-Json $pwsh @('-NoProfile','-File',$repair,'-Simulate','-RefreshCodexTrust','-RefreshSaveRuntimeTrust') 900000)}
        }elseif(Test-Path -LiteralPath $repair -PathType Leaf){
            [void](Invoke-Json $pwsh @('-NoProfile','-File',$repair,'-Simulate','-RefreshCodexTrust','-RefreshSaveRuntimeTrust') 900000)
            [void](Invoke-Json $pwsh @('-NoProfile','-File',$repair,'-RefreshCodexTrust','-RefreshSaveRuntimeTrust') 900000)
        }
        if('registrar' -notin $completedSteps){$completedSteps.Add('registrar');Write-IntegrationProgress $resolvedRoot $ExpectedCommit $GoalId $workerHash $clientHash $completedSteps}
        if('skill' -in $completedSteps){$skillResult=[ordered]@{published=$true;resumed_verified=$true}}
        else{$skillResult=Invoke-Json (Join-Path $resolvedRoot 'client.exe') @('--format','json','publish-skill','--root',$resolvedRoot,'--yes');if(-not $skillResult.published){throw 'Global entrypoint publication did not verify.'};$completedSteps.Add('skill');Write-IntegrationProgress $resolvedRoot $ExpectedCommit $GoalId $workerHash $clientHash $completedSteps}
        $enroll=Join-Path $PSScriptRoot 'enroll-global-codex-projects.ps1'
        if('enrollment' -in $completedSteps){$rollout=[ordered]@{applied=$true;state_routes_published=$true;resumed_verified=$true}}
        else{$rollout=Invoke-Json $pwsh @('-NoProfile','-File',$enroll,'-Yes','-MigrateState','-Root',$resolvedRoot,'-RaymanSource',$sourceRoot,'-SaveStatusSource',$saveRoot) 3600000 $sourceRoot;if(-not $rollout.applied -or -not $rollout.state_routes_published){throw 'Global project enrollment/state migration did not complete'};$completedSteps.Add('enrollment');Write-IntegrationProgress $resolvedRoot $ExpectedCommit $GoalId $workerHash $clientHash $completedSteps}
        [ordered]@{schema='rayman.global-install-result.v2';installed=$true;resumed=$resumed;root=$resolvedRoot;task=$TaskName;owner_sid=$userSid;worker_sha256=$workerHash;client_sha256=$clientHash;rayman=$raymanReceipt;save_status=$saveInstall;rollout=$rollout;product_processes=@{rayman=$raymanInstall};restart_required=$true}|ConvertTo-Json -Depth 20
        return
    }catch{
        if($externalIntegrationStarted -or $null -ne $retainedReceipt){throw "Global integration phase $initialPhase needs recovery; backend and Codex config were preserved. $($_.Exception.Message)"}
        if($taskRegistered){Stop-VerifiedWorkerTask $TaskName $resolvedRoot $userSid}
        if($resumeInitialized){throw "Initialized global installation failed during resume and was preserved for retry. $($_.Exception.Message)"}
        if($resumeExisting){throw "Retained global installation phase $initialPhase failed before integration and was preserved for retry. $($_.Exception.Message)"}
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
