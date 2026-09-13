[CmdletBinding()]
param([switch]$Library)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
[Console]::InputEncoding=[Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)

function Invoke-WorktreeProduct([string]$Program,[string[]]$Arguments,[string]$Workspace){
    $start=[Diagnostics.ProcessStartInfo]::new()
    $start.FileName=$Program;$start.WorkingDirectory=$Workspace
    $start.UseShellExecute=$false;$start.CreateNoWindow=$true
    $start.RedirectStandardOutput=$true;$start.RedirectStandardError=$true
    foreach($arg in $Arguments){$start.ArgumentList.Add($arg)}
    $process=[Diagnostics.Process]::Start($start)
    try{
        $stdout=$process.StandardOutput.ReadToEndAsync();$stderr=$process.StandardError.ReadToEndAsync()
        # The caller owns the kill-on-close process-tree job and total timeout.
        $process.WaitForExit()
        $out=$stdout.GetAwaiter().GetResult();$err=$stderr.GetAwaiter().GetResult()
        if($process.ExitCode -ne 0){throw ('Worktree initialization command failed: '+$err)}
        return $out|ConvertFrom-Json -Depth 40
    }finally{$process.Dispose()}
}

function Assert-WorktreeRoute($Marker,[string]$Installation,[string]$Worktree){
    if($Marker.schema -cne 'rayman.global-state-routing.v1' -or $Marker.installation_id -cne $Installation -or $Marker.worktree_id -cne $Worktree){
        throw 'A copied or replaced route must not be adopted by another worktree'
    }
}

function Get-WorktreeCheckpointTransaction([string]$Root,[string]$Worktree){
    $found=@(Get-ChildItem -LiteralPath $Root -File -Filter ('checkpoint-migration-'+$Worktree+'-*.json'))
    $pending=@()
    foreach($file in $found){
        if($file.Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Reparse migration record refused'}
        $record=Get-Content -LiteralPath $file.FullName -Raw|ConvertFrom-Json -Depth 20
        if($record.schema -cne 'rayman.checkpoint-migration.v1' -or $record.worktree_id -cne $Worktree -or $record.transaction_id -cnotmatch '^[a-f0-9]{32}$' -or $file.Name -cne ('checkpoint-migration-'+$Worktree+'-'+$record.transaction_id+'.json')){throw 'Checkpoint migration journal identity differs'}
        if($record.state -notin @('prepared','publishing','committed','rolled_back')){throw 'Checkpoint migration requires explicit recovery'}
        if($record.state -in @('prepared','publishing')){$pending+=,$record}
    }
    if($pending.Count -gt 1){throw 'Ambiguous checkpoint migration requires explicit recovery'}
    if($pending.Count){return [string]$pending[0].transaction_id}
    return $null
}

function Invoke-WorktreeBootstrap($Contract){
    $sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    if($sid -cne $Contract.owner_sid){throw 'Worktree state initialization requires the existing desktop-owner hook context'}
    $principal=[Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
    if($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)){throw 'Worktree initialization requires a least-privilege desktop token'}
    $root=[string]$Contract.root;$workspace=[string]$Contract.workspace
    $client=Join-Path $root 'client.exe'
    $global=@('--format','json')
    $status=Invoke-WorktreeProduct $client ($global+@('status','--root',$root)) $workspace
    if(-not $status.installation_verified -or -not $status.service_healthy -or $status.owner_sid -cne $sid){throw 'Owner worker is not healthy'}
    $worktree=[string]$Contract.worktree_id
    if(-not $Contract.already_enrolled){
        $result=Invoke-WorktreeProduct $client ($global+@('enroll-linked-worktree','--root',$root,'--workspace',$workspace,'--yes','--timeout-seconds','120')) $workspace
        if(-not $result.result.details.registered){throw 'Linked worktree registration did not complete'}
        $worktree=[string]$result.result.details.worktree_id
    }
    if($worktree -cnotmatch '^[a-f0-9]{32}$'){throw 'Invalid target worktree registration'}
    $guard=[IO.File]::Open((Join-Path $root ('.worktree-bootstrap-'+$worktree+'.lock')),[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
    try{
        if($null -ne $Contract.policy.rayman){
            $product=$Contract.policy.rayman
            $route=Join-Path $workspace '.RaymanCodingSkill/global-state.json'
            if(Test-Path -LiteralPath $route){
                Assert-WorktreeRoute (Get-Content -LiteralPath $route -Raw|ConvertFrom-Json) $status.heartbeat.installation_id $worktree
                $state=Invoke-WorktreeProduct $product.program @('--format','json','workspace','status') $workspace
                if(-not $state.active){throw 'Existing routed Rayman activation is not active'}
            }else{
                $activation=Join-Path $workspace '.RaymanCodingSkill/workspace_skill.yaml'
                if(Test-Path -LiteralPath $activation){
                    $state=Invoke-WorktreeProduct $product.program @('--format','json','workspace','status') $workspace
                    if(-not $state.active){throw 'Existing Rayman activation requires its explicit repair; no state copied'}
                }else{
                    [void](Invoke-WorktreeProduct $product.program @('--format','json','workspace','activate','--skill-file',(Join-Path $product.source_skill 'SKILL.md'),'--yes') $workspace)
                }
                $marker=Invoke-WorktreeProduct $client ($global+@('activate-rayman-state','--root',$root,'--workspace',$workspace,'--yes')) $workspace
                Assert-WorktreeRoute $marker $status.heartbeat.installation_id $worktree
            }
        }
        if($null -ne $Contract.policy.checkpoint){
            $product=$Contract.policy.checkpoint
            $route=Join-Path $workspace '.agent-checkpoints/global-state.json'
            if(Test-Path -LiteralPath $route){
                $marker=Get-Content -LiteralPath $route -Raw|ConvertFrom-Json
                Assert-WorktreeRoute $marker $status.heartbeat.installation_id $worktree
                if($marker.PSObject.Properties.Name -contains 'migration_state' -and $marker.migration_state -eq 'preparing'){
                    $transaction=Get-WorktreeCheckpointTransaction $root $worktree
                    if(-not $transaction -or $transaction -cne $marker.transaction_id){throw 'Preparing checkpoint marker has no matching protected transaction'}
                    [void](Invoke-WorktreeProduct $client ($global+@('publish-checkpoint-migration','--root',$root,'--workspace',$workspace,'--transaction-id',$transaction,'--yes')) $workspace)
                    $marker=Get-Content -LiteralPath $route -Raw|ConvertFrom-Json
                    Assert-WorktreeRoute $marker $status.heartbeat.installation_id $worktree
                }
            }else{
                $transaction=Get-WorktreeCheckpointTransaction $root $worktree
                if(-not $transaction){
                    $database=Join-Path $workspace '.agent-checkpoints/status.sqlite3'
                    if(Test-Path -LiteralPath $database){
                        $active=Invoke-WorktreeProduct $product.program @('activation-status','--workspace',$workspace) $workspace
                        if(-not $active.active -or -not $active.runtime_present){throw 'Existing checkpoint data requires explicit recovery; no parent history is adopted'}
                    }else{
                        [void](Invoke-WorktreeProduct $product.program @('install','--workspace',$workspace,'--source-skill-dir',$product.source_skill,'--local-only') $workspace)
                    }
                    $active=Invoke-WorktreeProduct $product.program @('activation-status','--workspace',$workspace) $workspace
                    if(-not $active.active -or -not $active.runtime_present -or $active.runtime_sha256 -cnotmatch '^[a-f0-9]{64}$'){throw 'Checkpoint runtime initialization failed'}
                    $prepared=Invoke-WorktreeProduct $client ($global+@('prepare-checkpoint-migration','--root',$root,'--workspace',$workspace,'--runtime-sha256',[string]$active.runtime_sha256,'--yes')) $workspace
                    $transaction=[string]$prepared.transaction_id
                }
                if($transaction -cnotmatch '^[a-f0-9]{32}$'){throw 'Invalid checkpoint migration identity'}
                [void](Invoke-WorktreeProduct $client ($global+@('publish-checkpoint-migration','--root',$root,'--workspace',$workspace,'--transaction-id',$transaction,'--yes')) $workspace)
                $marker=Get-Content -LiteralPath $route -Raw|ConvertFrom-Json
                Assert-WorktreeRoute $marker $status.heartbeat.installation_id $worktree
            }
            $runtime=Join-Path $workspace '.agent-checkpoints/runtime/save-work-status.exe'
            if(($marker.PSObject.Properties.Name -contains 'migration_state' -and $marker.migration_state -eq 'preparing') -or $marker.runtime_sha256 -cne $product.program_sha256 -or (Get-FileHash -LiteralPath $runtime).Hash.ToLowerInvariant() -cne $marker.runtime_sha256){throw 'Published checkpoint runtime or migration state differs'}
        }
        return [pscustomobject]@{initialized=$true;worktree_id=$worktree;parent_history_copied=$false;rayman=($null -ne $Contract.policy.rayman);checkpoint=($null -ne $Contract.policy.checkpoint)}
    }finally{$guard.Dispose()}
}

if(-not $Library){
    $contract=[Console]::In.ReadToEnd()|ConvertFrom-Json -Depth 30
    Invoke-WorktreeBootstrap $contract|ConvertTo-Json -Depth 8
}
