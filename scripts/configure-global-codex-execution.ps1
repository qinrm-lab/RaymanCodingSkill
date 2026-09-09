[CmdletBinding(DefaultParameterSetName='Plan')]
param(
 [Parameter(ParameterSetName='Plan')][switch]$Plan,
 [Parameter(Mandatory,ParameterSetName='SelfTest')][switch]$SelfTest,
 [Parameter(Mandatory,ParameterSetName='Apply')][switch]$Yes,
 [string]$ConfigPath=(Join-Path $env:USERPROFILE '.codex/config.toml')
)
$globalConfigMode=$PSCmdlet.ParameterSetName
$globalConfigPath=$ConfigPath
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'configure-codex-validation-temp.ps1') -Library
$script:GlobalEndpoint='C:\ProgramData\Rayman\CodexGlobalExecution'
$script:GlobalQueue=Join-Path $script:GlobalEndpoint 'requests'
function Add-GlobalAssignment($Lines,[string[]]$Table,[string]$Key,[string]$Value,[string]$NewLine){
 $structure=Get-TomlStructure -Lines $Lines
 $current=Get-SingleAssignment -Structure $structure -TablePath $Table -KeyPath @($Key) -Label ($Table -join '.') -Optional
 if($null -ne $current){if((ConvertFrom-TomlStringValue $current.ValueText) -cne $Value){throw 'Existing global routing or queue permission differs; explicit migration is required.'};return}
 $headers=@($structure.Headers | Where-Object {Test-PathSegmentsEqual -Actual $_.Path -Expected $Table})
 if($headers.Count -gt 1 -or ($headers.Count -eq 1 -and $headers[0].IsArray)){throw 'Ambiguous global configuration table.'}
 $line=(New-TomlBasicString $Key)+' = '+(New-TomlBasicString $Value)
 if($headers.Count -eq 0){
  $header='['+(($Table | ForEach-Object {New-TomlBasicString $_}) -join '.')+']'
  Add-TomlLines -Lines $Lines -Index $Lines.Count -Content @('',$header,$line) -NewLine $NewLine
 }else{
  $next=@($structure.Headers | Where-Object {$_.LineIndex -gt $headers[0].LineIndex} | Sort-Object LineIndex | Select-Object -First 1)
  $index=if($next.Count){$next[0].LineIndex}else{$Lines.Count}
  Add-TomlLines -Lines $Lines -Index $index -Content @($line) -NewLine $NewLine
 }
}
function Assert-GlobalSemanticDelta([string]$Before,[string]$After){
 $program=(Get-Command python -CommandType Application | Select-Object -First 1).Source
 $code=@"
import copy,json,sys,tomllib
p=json.load(sys.stdin)
a=tomllib.loads(p['before']);b=tomllib.loads(p['after']);e=copy.deepcopy(a)
profile=a['default_permissions']
assert a['windows']['sandbox']=='elevated'
assert a['permissions'][profile]['extends']==':workspace'
e.setdefault('shell_environment_policy',{}).setdefault('set',{})['RAYMAN_GLOBAL_EXECUTION_ROOT']=p['root']
e['permissions'][profile].setdefault('filesystem',{})[p['queue']]='write'
assert b==e,'configuration changed outside the two allowed keys'
"@
 $start=[Diagnostics.ProcessStartInfo]::new();$start.FileName=$program;$start.UseShellExecute=$false;$start.CreateNoWindow=$true;$start.RedirectStandardInput=$true;$start.RedirectStandardOutput=$true;$start.RedirectStandardError=$true
 $start.ArgumentList.Add('-c');$start.ArgumentList.Add($code)
 $child=[Diagnostics.Process]::Start($start)
 try{
  $out=$child.StandardOutput.ReadToEndAsync();$err=$child.StandardError.ReadToEndAsync()
  $payload=@{before=$Before;after=$After;root=$script:GlobalEndpoint;queue=$script:GlobalQueue}|ConvertTo-Json -Compress
  $child.StandardInput.Write($payload);$child.StandardInput.Close()
  if(-not $child.WaitForExit(15000)){$child.Kill($true);$child.WaitForExit();throw 'TOML verification timed out.'}
  [void]$out.GetAwaiter().GetResult();[void]$err.GetAwaiter().GetResult()
  if($child.ExitCode -ne 0){throw 'TOML validation failed or unrelated configuration changed.'}
 }finally{$child.Dispose()}
}
function Get-GlobalCandidate([string]$Text,[string]$NewLine){
 $lines=ConvertTo-TomlLines -Text $Text
 $structure=Get-TomlStructure -Lines $lines
 $sandbox=Get-SingleAssignment -Structure $structure -TablePath @('windows') -KeyPath @('sandbox') -Label 'windows.sandbox'
 if((ConvertFrom-TomlStringValue $sandbox.ValueText) -cne 'elevated'){throw 'Keep the elevated Windows sandbox.'}
 $default=Get-SingleAssignment -Structure $structure -TablePath @() -KeyPath @('default_permissions') -Label 'default_permissions'
 $profile=ConvertFrom-TomlStringValue $default.ValueText
 $parent=Get-SingleAssignment -Structure $structure -TablePath @('permissions',$profile) -KeyPath @('extends') -Label 'profile.extends'
 if((ConvertFrom-TomlStringValue $parent.ValueText) -cne ':workspace'){throw 'A named :workspace profile is required.'}
 Add-GlobalAssignment $lines @('shell_environment_policy','set') 'RAYMAN_GLOBAL_EXECUTION_ROOT' $script:GlobalEndpoint $NewLine
 Add-GlobalAssignment $lines @('permissions',$profile,'filesystem') $script:GlobalQueue 'write' $NewLine
 $candidate=ConvertFrom-TomlLines -Lines $lines
 Assert-GlobalSemanticDelta $Text $candidate
 return $candidate
}
function Test-GlobalConfig-PreservesUnrelatedBytes {
 $fixture="default_permissions = 'work'`n[windows]`nsandbox = 'elevated'`n[permissions.work]`nextends = ':workspace'`n[permissions.work.filesystem]`n'C:\private' = 'deny'`n[shell_environment_policy.set]`nOTHER = '原样 # unchanged'`n[model_providers.example]`nbase_url = 'https://example.invalid'`n"
 foreach($ending in @("`n","`r`n")){
  $source=$fixture.Replace("`n",$ending);$candidate=Get-GlobalCandidate $source $ending
  if((Get-GlobalCandidate $candidate $ending) -cne $candidate){throw 'Global config is not idempotent.'}
  $without=($candidate -split [regex]::Escape($ending) | Where-Object {$_ -notmatch '^"RAYMAN_GLOBAL_EXECUTION_ROOT" =' -and $_ -notmatch '^"C:.*CodexGlobalExecution.*requests" ='}) -join $ending
  if($without -cne $source){throw 'Unrelated bytes changed.'}
 }
}
function Test-GlobalConfig-RejectsConflictingPolicy {
 $fixture="default_permissions = 'work'`n[windows]`nsandbox = 'elevated'`n[permissions.work]`nextends = ':workspace'`n[shell_environment_policy.set]`nRAYMAN_GLOBAL_EXECUTION_ROOT='C:\unexpected'`n"
 $rejected=$false;try{[void](Get-GlobalCandidate $fixture "`n")}catch{$rejected=$true};if(-not $rejected){throw 'Conflicting endpoint was accepted.'}
 $rejected=$false;try{[void](Get-GlobalCandidate ($fixture.Replace('elevated','unelevated')) "`n")}catch{$rejected=$true};if(-not $rejected){throw 'Sandbox downgrade was accepted.'}
}
if($globalConfigMode -eq 'SelfTest'){
 Test-GlobalConfig-PreservesUnrelatedBytes
 Test-GlobalConfig-RejectsConflictingPolicy
 Write-Output 'configure-global-codex-execution: PASS (2 suites)'
 return
}
$document=Read-StrictUtf8Document $globalConfigPath
$candidate=Get-GlobalCandidate $document.Text $document.NewLine
$bytes=ConvertTo-StrictUtf8Bytes -Text $candidate -HasBom $document.HasBom
$changed=(Get-BytesSha256 $bytes) -cne $document.Sha256
if($globalConfigMode -eq 'Plan'){
 [ordered]@{schema='rayman.global-config-plan.v1';config=$document.Path;changed=$changed;before_sha256=$document.Sha256;after_sha256=(Get-BytesSha256 $bytes);endpoint=$script:GlobalEndpoint;queue=$script:GlobalQueue;unrelated_settings_preserved=$true;applied=$false;restart_required=$changed}|ConvertTo-Json
 return
}
Assert-ConfigOwnerIsCurrentPrincipal $document.Path
$client=Join-Path $script:GlobalEndpoint 'client.exe'
if(-not(Test-Path -LiteralPath $client -PathType Leaf)){throw 'Install and verify the protected backend before enabling global routing.'}
$status=& $client --format json status --root $script:GlobalEndpoint | ConvertFrom-Json
if($LASTEXITCODE -ne 0 -or -not $status.service_healthy -or $status.owner_sid -cne [Security.Principal.WindowsIdentity]::GetCurrent().User.Value){throw 'Protected backend identity or live service verification failed.'}
$result=Set-ConfigFileTransactional -Path $document.Path -OriginalBytes $document.Bytes -CandidateBytes $bytes
[ordered]@{changed=$result.Changed;backup=$result.BackupPath;restart_required=$result.Changed;endpoint=$script:GlobalEndpoint}|ConvertTo-Json
