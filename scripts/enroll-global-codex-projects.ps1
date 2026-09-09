[CmdletBinding(DefaultParameterSetName='Plan')]
param(
    [Parameter(ParameterSetName='Plan')][switch]$Plan,
    [Parameter(Mandatory,ParameterSetName='SelfTest')][switch]$SelfTest,
    [Parameter(Mandatory,ParameterSetName='Apply')][switch]$Yes,
    [string]$Root='C:\ProgramData\Rayman\CodexGlobalExecution',
    [string]$ConfigPath=(Join-Path $env:USERPROFILE '.codex\config.toml'),
    [string]$GitPath='C:\Program Files\Git\mingw64\bin\git.exe',
    [string]$AuthorName='qinrm',
    [string]$AuthorEmail='32691594@qq.com',
    [string]$RaymanPath=(Join-Path $env:LOCALAPPDATA 'Rayman\bin\rayman.exe'),
    [string]$SaveStatusPath=(Join-Path $env:USERPROFILE '.codex\skills\save-work-status\bin\save-work-status.exe'),
    [string]$RaymanSource='E:\rayman\software\AI\RaymanCodingSkill',
    [string]$SaveStatusSource='E:\rayman\software\AI\RaymanSaveStatusSkill',
    [Parameter(ParameterSetName='Apply')][switch]$MigrateState
)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$enrollmentMode=$PSCmdlet.ParameterSetName
if(-not $IsWindows -or $PSVersionTable.PSVersion.Major -lt 7){throw 'Native Windows PowerShell 7 is required.'}
. (Join-Path $PSScriptRoot 'configure-codex-validation-temp.ps1') -Library

function Invoke-GlobalJson([string]$Program,[string[]]$Arguments,[string]$WorkingDirectory=(Get-Location).ProviderPath,[int]$TimeoutMilliseconds=60000){
    $start=[Diagnostics.ProcessStartInfo]::new()
    $start.FileName=$Program;$start.WorkingDirectory=$WorkingDirectory;$start.UseShellExecute=$false;$start.CreateNoWindow=$true
    $start.RedirectStandardOutput=$true;$start.RedirectStandardError=$true
    foreach($argument in $Arguments){$start.ArgumentList.Add($argument)}
    $process=[Diagnostics.Process]::Start($start)
    try{
        $stdout=$process.StandardOutput.ReadToEndAsync();$stderr=$process.StandardError.ReadToEndAsync()
        if(-not $process.WaitForExit($TimeoutMilliseconds)){$process.Kill($true);$process.WaitForExit();throw 'Global enrollment command timed out.'}
        $out=$stdout.GetAwaiter().GetResult();$err=$stderr.GetAwaiter().GetResult()
        if($process.ExitCode -ne 0){throw $err.Trim()}
        return $out | ConvertFrom-Json -Depth 30
    }finally{$process.Dispose()}
}
function Get-ProductAdapterSpecifications([string]$Profile,[string]$LocalAppData){
    $raymanSkill=Join-Path $Profile '.codex\skills\raymancodingskill'
    $saveSkill=Join-Path $Profile '.codex\skills\save-work-status'
    $rayman=[ordered]@{adapter_id='rayman';targets=[ordered]@{
        '01_cli'=Join-Path $LocalAppData 'Rayman\bin\rayman.exe'
        '02_update_worker'=Join-Path $LocalAppData 'Rayman\bin\rayman-update-worker-{version}.exe'
        '10_skill'=Join-Path $raymanSkill 'SKILL.md'
        '11_agent_contract'=Join-Path $raymanSkill 'AGENTS.md'
        '12_workflow_contract'=Join-Path $raymanSkill 'references\workflow-contract.md'
        '99_receipt'=Join-Path $LocalAppData 'Rayman\install\receipt.json'
    }}
    $saveTargets=[ordered]@{}
    $saveFiles=@(
        'SKILL.md','agents\openai.yaml','scripts\checkpoint_vault.py','scripts\status_checkpoint.py',
        'scripts\workspace_activation.py','scripts\install_skill.py','bin\save-work-status.exe',
        'bin\save-work-status-task-launcher.exe','watchdog\README.md','watchdog\CodexResumeWatchdog.psm1',
        'watchdog\Install-CodexResumeWatchdog.ps1','watchdog\Invoke-CodexResumeWatchdog.ps1',
        'watchdog\Uninstall-CodexResumeWatchdog.ps1','watchdog\CodexResumeRegistrar.psm1',
        'watchdog\Install-CodexResumeRegistrar.ps1','watchdog\Invoke-CodexResumeRegistrar.ps1',
        'watchdog\Repair-CodexResumeRegistrar.ps1','install-manifest.json'
    )
    for($index=0;$index -lt $saveFiles.Count;$index++){
        $saveTargets[('save_{0:D2}' -f $index)]=Join-Path $saveSkill $saveFiles[$index]
    }
    return @(
        [pscustomobject]@{name='rayman';source=[IO.Path]::GetFullPath($RaymanSource);specification=$rayman},
        [pscustomobject]@{name='save-work-status';source=[IO.Path]::GetFullPath($SaveStatusSource);specification=[ordered]@{adapter_id='save-work-status';targets=$saveTargets}}
    )
}
function Write-ExactJson([string]$Path,$Value){
    $text=($Value|ConvertTo-Json -Depth 12)+"`n"
    if(Test-Path -LiteralPath $Path -PathType Leaf){
        $old=Get-Content -LiteralPath $Path -Raw|ConvertFrom-Json -Depth 12
        if(($old|ConvertTo-Json -Depth 12 -Compress) -cne ($Value|ConvertTo-Json -Depth 12 -Compress)){throw "Existing adapter specification differs: $Path"}
        return
    }
    [IO.File]::WriteAllText($Path,$text,[Text.UTF8Encoding]::new($false))
}
function Get-JsonFile([string]$Path){
    $item=Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    if(-not $item.PSIsContainer -and -not($item.Attributes -band [IO.FileAttributes]::ReparsePoint)){
        return Get-Content -LiteralPath $Path -Raw|ConvertFrom-Json -Depth 20
    }
    throw "Expected an ordinary JSON file: $Path"
}
function Get-TrustedProjects([string]$Path){
    $document=Read-StrictUtf8Document $Path
    $lines=ConvertTo-TomlLines $document.Text
    $structure=Get-TomlStructure $lines
    $projects=[Collections.Generic.List[string]]::new()
    foreach($header in $structure.Headers){
        if($header.IsArray -or $header.Path.Count -ne 2 -or $header.Path[0] -cne 'projects'){continue}
        $next=@($structure.Headers | Where-Object LineIndex -gt $header.LineIndex | Sort-Object LineIndex | Select-Object -First 1)
        $end=if($next.Count){[int]$next[0].LineIndex}else{$lines.Count}
        $trustValues=[Collections.Generic.List[string]]::new()
        for($index=$header.LineIndex+1;$index -lt $end;$index++){
            $code=(Split-TomlComment ([string]$lines[$index].Content)).Code
            if([string]::IsNullOrWhiteSpace($code)){continue}
            $equals=Find-TomlEquals $code;if($equals -lt 1){continue}
            $keys=@(ConvertFrom-TomlKeyPath $code.Substring(0,$equals).Trim())
            if($keys.Count -eq 1 -and $keys[0] -ceq 'trust_level'){$trustValues.Add((ConvertFrom-TomlStringValue $code.Substring($equals+1).Trim()))}
        }
        if($trustValues.Count -ne 1 -or $trustValues[0] -cne 'trusted'){continue}
        $candidate=[IO.Path]::GetFullPath($header.Path[1])
        if(-not(Test-Path -LiteralPath $candidate -PathType Container)){continue}
        Assert-OrdinaryPath $candidate
        $projects.Add($candidate)
    }
    return @($projects | Sort-Object -Unique)
}
function Assert-OrdinaryPath([string]$Path){
    $cursor=[IO.Path]::GetFullPath($Path)
    while($cursor){
        if(Test-Path -LiteralPath $cursor){
            $item=Get-Item -LiteralPath $cursor -Force
            if($item.Attributes -band [IO.FileAttributes]::ReparsePoint){throw "Reparse project path refused: $cursor"}
        }
        $parent=[IO.Path]::GetDirectoryName($cursor);if($parent -eq $cursor){break};$cursor=$parent
    }
}
if($enrollmentMode -eq 'SelfTest'){
    $base=Join-Path ([IO.Path]::GetTempPath()) ('global-enrollment-test-'+[Guid]::NewGuid().ToString('N'))
    $first=Join-Path $base 'first';$second=Join-Path $base 'second';$config=Join-Path $base 'config.toml'
    try{
        [void][IO.Directory]::CreateDirectory($first);[void][IO.Directory]::CreateDirectory($second)
        $text="[projects.'$first']`ntrust_level='trusted'`n[projects.'$second']`ntrust_level='untrusted'`n"
        [IO.File]::WriteAllText($config,$text,[Text.UTF8Encoding]::new($false))
        $found=@(Get-TrustedProjects $config)
        if($found.Count -ne 1 -or -not $found[0].Equals($first,[StringComparison]::OrdinalIgnoreCase)){throw "Trusted-project selection differs: expected=$first actual=$($found -join ',')"}
        $specs=Get-ProductAdapterSpecifications 'C:\Users\fixture' 'C:\Users\fixture\AppData\Local'
        if($specs.Count -ne 2 -or $specs[0].specification.targets.Count -ne 6 -or $specs[1].specification.targets.Count -ne 18){throw 'Product adapter target sets differ'}
        if(@($specs[0].specification.targets.Values|Where-Object {$_ -like '*{version}*'}).Count -ne 1){throw 'Rayman versioned worker target differs'}
        Write-Output 'enroll-global-codex-projects: PASS (trusted project selection and fixed product adapters)'
        return
    }finally{
        if(Test-Path -LiteralPath $base){Remove-Item -LiteralPath $base -Recurse -Force}
    }
}
$worker=Join-Path ([IO.Path]::GetFullPath($Root)) 'worker.exe'
$client=Join-Path ([IO.Path]::GetFullPath($Root)) 'client.exe'
foreach($path in @($worker,$client,$GitPath)){if(-not(Test-Path -LiteralPath $path -PathType Leaf)){throw "Required executable is missing: $path"}}
$status=Invoke-GlobalJson $client @('--format','json','status','--root',$Root)
$currentSid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value
if(-not $status.service_healthy -or $status.owner_sid -cne $currentSid){throw 'Global worker is not healthy under the current qinrm identity.'}
$pwsh=(Get-Command pwsh -CommandType Application -ErrorAction Stop).Source
$aclRepair=Join-Path $PSScriptRoot 'repair-codex-workspace-acl.ps1'
$aclPlan=Invoke-GlobalJson $pwsh @('-NoProfile','-File',$aclRepair,'-Plan','-OwnerAccount',([Security.Principal.WindowsIdentity]::GetCurrent().Name))
if(-not $aclPlan.safe){
    throw "Trusted project Git ACL preflight is not safe: needs_prepare=$($aclPlan.needs_prepare_count), blocked=$($aclPlan.blocked_count). Run repair-codex-workspace-acl.ps1 -Prepare -Yes from a healthy Codex thread first."
}
$projects=Get-TrustedProjects $ConfigPath
$plans=[Collections.Generic.List[object]]::new()
foreach($project in $projects){
    $gitMarker=Join-Path $project '.git'
    $hasGit=Test-Path -LiteralPath $gitMarker
    $raymanState=Test-Path -LiteralPath (Join-Path $project '.RaymanCodingSkill\workspace_skill.yaml') -PathType Leaf
    $checkpointState=Test-Path -LiteralPath (Join-Path $project '.agent-checkpoints\status.sqlite3') -PathType Leaf
    $formal=$raymanState -or $checkpointState
    if(-not $hasGit -and -not $formal){continue}
    $arguments=[Collections.Generic.List[string]]::new()
    foreach($value in @('--format','json','enroll','--root',$Root,'--workspace',$project)){$arguments.Add($value)}
    if($hasGit){foreach($value in @('--git',$GitPath,'--author-name',$AuthorName,'--author-email',$AuthorEmail)){$arguments.Add($value)}}
    if($formal){$arguments.Add('--formal-state')}
    $arguments.Add('--preflight')
    $result=Invoke-GlobalJson $worker $arguments.ToArray()
    $managedDirtyBefore=[Collections.Generic.List[string]]::new()
    if($hasGit){foreach($name in @('AGENTS.md','CLAUDE.md')){if(@(& $GitPath -c "safe.directory=$project" -C $project status --porcelain=v1 -- $name).Count){$managedDirtyBefore.Add($name)}}}
    $plans.Add([pscustomobject]@{workspace=$project;git=$hasGit;formal_state=$formal;rayman_state=$raymanState;checkpoint_state=$checkpointState;managed_dirty_before=@($managedDirtyBefore);project_id=$result.registration.project_id;worktree_id=$result.registration.worktree_id})
}
$adapterPlans=Get-ProductAdapterSpecifications $env:USERPROFILE $env:LOCALAPPDATA
foreach($adapter in $adapterPlans){
    if(-not @($plans|Where-Object {$_.workspace.Equals($adapter.source,[StringComparison]::OrdinalIgnoreCase)}).Count){throw "Product source is not an enrolled trusted project: $($adapter.source)"}
    foreach($destination in $adapter.specification.targets.Values){
        $parent=Split-Path -Parent $destination
        Assert-OrdinaryPath $parent
        if(-not(Test-Path -LiteralPath $parent -PathType Container)){throw "Product installation parent is missing: $parent"}
    }
}
if($enrollmentMode -ne 'Apply'){
    [ordered]@{schema='rayman.global-enrollment-plan.v2';owner_sid=$currentSid;author_name=$AuthorName;author_email=$AuthorEmail;project_count=$plans.Count;projects=$plans;install_adapters=$adapterPlans;state_migration_requested=$false;applied=$false}|ConvertTo-Json -Depth 12
    return
}
$applied=[Collections.Generic.List[object]]::new()
foreach($plan in $plans){
    $arguments=[Collections.Generic.List[string]]::new()
    foreach($value in @('--format','json','enroll','--root',$Root,'--workspace',$plan.workspace)){$arguments.Add($value)}
    if($plan.git){foreach($value in @('--git',$GitPath,'--author-name',$AuthorName,'--author-email',$AuthorEmail)){$arguments.Add($value)}}
    if($plan.formal_state){$arguments.Add('--formal-state')}
    $arguments.Add('--yes')
    $result=Invoke-GlobalJson $worker $arguments.ToArray()
    if($result.registration.worktree_id -cne $plan.worktree_id){throw "Enrollment identity changed after preflight: $($plan.workspace)"}
    $applied.Add($plan)
}
$specRoot=Join-Path ([IO.Path]::GetFullPath($Root)) 'adapter-specifications'
if(-not(Test-Path -LiteralPath $specRoot)){[void][IO.Directory]::CreateDirectory($specRoot)}
$adapterResults=[Collections.Generic.List[object]]::new()
foreach($adapter in $adapterPlans){
    $specPath=Join-Path $specRoot ($adapter.name+'.json')
    Write-ExactJson $specPath $adapter.specification
    $registration=Invoke-GlobalJson $client @('--format','json','register-install-adapter','--root',$Root,'--workspace',$adapter.source,'--specification',$specPath,'--yes')
    $adapterResults.Add([pscustomobject]@{name=$adapter.name;source=$adapter.source;adapter_id=$registration.adapter_id;worktree_id=$registration.worktree_id})
}
$routes=[Collections.Generic.List[object]]::new()
if($MigrateState){
    foreach($path in @($RaymanPath,$SaveStatusPath)){if(-not(Test-Path -LiteralPath $path -PathType Leaf)){throw "Installed state runtime is missing: $path"};Assert-OrdinaryPath $path}
    $env:RAYMAN_GLOBAL_EXECUTION_ROOT=[IO.Path]::GetFullPath($Root)
    $saveSkill=Split-Path -Parent (Split-Path -Parent $SaveStatusPath)
    foreach($plan in $plans){
        $raymanRoute=$false;$checkpointRoute=$false;$checkpointTransaction=$null
        if($plan.rayman_state){
            $status=Invoke-GlobalJson $RaymanPath @('--format','json','workspace','status') $plan.workspace
            if(-not $status.active){
                if(-not $status.rebind_eligible){throw "Rayman activation cannot be safely rebound: $($plan.workspace)"}
                [void](Invoke-GlobalJson $RaymanPath @('--format','json','workspace','rebind','--yes') $plan.workspace)
            }
            $route=Invoke-GlobalJson $client @('--format','json','activate-rayman-state','--root',$Root,'--workspace',$plan.workspace,'--yes')
            if($route.worktree_id -cne $plan.worktree_id){throw "Rayman route identity differs: $($plan.workspace)"}
            $raymanRoute=$true
        }
        if($plan.checkpoint_state){
            $transition=Join-Path $plan.workspace '.agent-checkpoints\global-state-transition.json'
            $marker=Join-Path $plan.workspace '.agent-checkpoints\global-state.json'
            if(Test-Path -LiteralPath $transition -PathType Leaf){
                $control=Get-JsonFile $transition
                $checkpointTransaction=[string]$control.transaction_id
                if($checkpointTransaction -notmatch '^[0-9a-f]{32}$'){throw "Checkpoint transition identity is invalid: $($plan.workspace)"}
                $migration=Invoke-GlobalJson $client @('--format','json','publish-checkpoint-migration','--root',$Root,'--workspace',$plan.workspace,'--transaction-id',$checkpointTransaction,'--yes')
            } elseif(Test-Path -LiteralPath $marker -PathType Leaf){
                $control=Get-JsonFile $marker
                if($control.schema -cne 'rayman.global-state-routing.v1' -or $control.worktree_id -cne $plan.worktree_id){throw "Checkpoint route identity differs: $($plan.workspace)"}
                $checkpointTransaction=[string]$control.transaction_id
                if($checkpointTransaction -notmatch '^[0-9a-f]{32}$'){throw "Checkpoint route transaction is invalid: $($plan.workspace)"}
                $activation=Invoke-GlobalJson $SaveStatusPath @('activation-status','--workspace',$plan.workspace) $plan.workspace
                if(-not $activation.active -or -not $activation.runtime_present){throw "Existing checkpoint route runtime is not usable: $($plan.workspace)"}
                $migration=$control
            } else {
                [void](Invoke-GlobalJson $SaveStatusPath @('install','--workspace',$plan.workspace,'--source-skill-dir',$saveSkill) $plan.workspace 180000)
                $activation=Invoke-GlobalJson $SaveStatusPath @('activation-status','--workspace',$plan.workspace) $plan.workspace
                if(-not $activation.active -or -not $activation.runtime_present -or [string]::IsNullOrWhiteSpace([string]$activation.runtime_sha256)){throw "SaveStatus runtime activation differs: $($plan.workspace)"}
                $prepared=Invoke-GlobalJson $client @('--format','json','prepare-checkpoint-migration','--root',$Root,'--workspace',$plan.workspace,'--runtime-sha256',[string]$activation.runtime_sha256,'--yes') $plan.workspace 180000
                $checkpointTransaction=[string]$prepared.transaction_id
                $migration=Invoke-GlobalJson $client @('--format','json','publish-checkpoint-migration','--root',$Root,'--workspace',$plan.workspace,'--transaction-id',$checkpointTransaction,'--yes') $plan.workspace 180000
            }
            $migrationState=if($migration.PSObject.Properties.Name -contains 'state'){[string]$migration.state}else{'committed'}
            if($migrationState -cne 'committed'){throw "Checkpoint route did not commit: $($plan.workspace)"}
            $checkpointRoute=$true
        }
        $managedCommit=$null
        if($plan.git){
            $managedChanged=[Collections.Generic.List[string]]::new()
            foreach($name in @('AGENTS.md','CLAUDE.md')){
                if($plan.managed_dirty_before -notcontains $name -and @(& $GitPath -c "safe.directory=$($plan.workspace)" -C $plan.workspace status --porcelain=v1 -- $name).Count){$managedChanged.Add($name)}
            }
            if($managedChanged.Count){
                $arguments=[Collections.Generic.List[string]]::new()
                foreach($value in @('--format','json','commit','--root',$Root,'--workspace',$plan.workspace)){$arguments.Add($value)}
                foreach($name in $managedChanged){$arguments.Add('--path');$arguments.Add($name)}
                foreach($value in @('--message','chore: refresh global state runtime','--yes','--timeout-seconds','300')){$arguments.Add($value)}
                $managedCommit=Invoke-GlobalJson $client $arguments.ToArray() $plan.workspace 330000
                if($managedCommit.result.status -cne 'effect_succeeded'){throw "Managed runtime commit failed: $($plan.workspace)"}
            }
        }
        $gitStatus=if($plan.git){@(& $GitPath -c "safe.directory=$($plan.workspace)" -C $plan.workspace status --porcelain=v1 --untracked-files=all)}else{@()}
        $routes.Add([pscustomobject]@{workspace=$plan.workspace;rayman_state=$raymanRoute;checkpoint_state=$checkpointRoute;checkpoint_transaction=$checkpointTransaction;managed_commit=$managedCommit;preserved_managed_dirty=@($plan.managed_dirty_before);git_status_after=$gitStatus})
    }
}
[ordered]@{schema='rayman.global-enrollment-result.v2';owner_sid=$currentSid;author_name=$AuthorName;author_email=$AuthorEmail;project_count=$applied.Count;projects=$applied;install_adapters=$adapterResults;state_routes=$routes;applied=$true;state_routes_published=[bool]$MigrateState}|ConvertTo-Json -Depth 12
