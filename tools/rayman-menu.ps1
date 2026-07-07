#Requires -Version 5.1
<#
.SYNOPSIS
  Rayman 控制台：工作树快照（保存 / 恢复 / 自动保存计划任务）+ 运行 eval（菜单选后端）。

.DESCRIPTION
  一个交互式菜单，方便在 codex / claude / copilot 之间切换时保护、恢复工作状态，
  并快速用 yunyi / deepseek 等后端跑 A/B outcome eval。

  - 快照 = 当前工作树（gitignore 感知，跳过 target/node_modules 等）+ .RaymanCodingSkill 任务状态。
  - 自动保存用 Windows 计划任务实现：注册一次，登录后每 N 分钟自动跑，无需一直开着窗口；
    断电/重启后（StartWhenAvailable）会自动补跑。
  - 恢复是叠加式：覆盖同名文件，不删除工作区里多出来的文件。

.EXAMPLE
  pwsh -File tools\rayman-menu.ps1
#>

$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot
$EvalsDir = Join-Path $RepoRoot 'evals'
$Script:Workspace = (Get-Location).Path
$Script:Keep = 3

function Find-Rayman {
    $cmd = Get-Command rayman -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $local = Join-Path $env:LOCALAPPDATA 'Rayman\bin\rayman.exe'
    if (Test-Path $local) { return $local }
    return $null
}

function Assert-Rayman {
    $r = Find-Rayman
    if (-not $r) {
        Write-Host "找不到 rayman。请先安装（把 rayman.exe 放到 %LOCALAPPDATA%\Rayman\bin 或加进 PATH）。" -ForegroundColor Red
        return $null
    }
    return $r
}

function Get-TaskName {
    param([string]$Workspace)
    $leaf = Split-Path -Leaf $Workspace
    $safe = ($leaf -replace '[^A-Za-z0-9_-]', '_')
    return "RaymanCheckpoint-$safe"
}

function Invoke-SaveNow {
    $r = Assert-Rayman; if (-not $r) { return }
    Write-Host "在 $Script:Workspace 保存快照..." -ForegroundColor Cyan
    Push-Location $Script:Workspace
    try { & $r checkpoint save --keep $Script:Keep }
    finally { Pop-Location }
}

function Invoke-List {
    $r = Assert-Rayman; if (-not $r) { return }
    Push-Location $Script:Workspace
    try { & $r checkpoint list }
    finally { Pop-Location }
}

function Invoke-RestoreLatest {
    $r = Assert-Rayman; if (-not $r) { return }
    Push-Location $Script:Workspace
    try {
        & $r checkpoint status
        Write-Host ""
        Write-Host "恢复会用最近的快照覆盖工作区里的同名文件（不删除多出来的文件）。" -ForegroundColor Yellow
        $ans = Read-Host "确认恢复最近快照到 $Script:Workspace ？(y/N)"
        if ($ans -eq 'y' -or $ans -eq 'Y') {
            & $r checkpoint restore --yes
        } else {
            Write-Host "已取消。"
        }
    }
    finally { Pop-Location }
}

function Install-AutosaveTask {
    $r = Assert-Rayman; if (-not $r) { return }
    $intervalRaw = Read-Host "自动保存间隔（分钟，回车=30）"
    $interval = 30
    if ($intervalRaw -and ($intervalRaw -as [int])) { $interval = [int]$intervalRaw }
    if ($interval -lt 1) { $interval = 1 }

    $taskName = Get-TaskName -Workspace $Script:Workspace
    $action = New-ScheduledTaskAction -Execute $r `
        -Argument "checkpoint save --keep $Script:Keep" `
        -WorkingDirectory $Script:Workspace
    # 每 interval 分钟重复一次，持续约 10 年（近似无限）；再加一个登录触发器，重启后自动接着跑。
    $repeat = New-ScheduledTaskTrigger -Once -At ((Get-Date).AddMinutes(1)) `
        -RepetitionInterval (New-TimeSpan -Minutes $interval) `
        -RepetitionDuration (New-TimeSpan -Days 3650)
    $atLogon = New-ScheduledTaskTrigger -AtLogOn
    # StartWhenAvailable：断电/关机错过的运行，开机后补跑一次。
    $settings = New-ScheduledTaskSettingsSet -StartWhenAvailable `
        -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
        -MultipleInstances IgnoreNew
    $principal = New-ScheduledTaskPrincipal -UserId "$env:USERDOMAIN\$env:USERNAME" `
        -LogonType Interactive -RunLevel Limited

    Register-ScheduledTask -TaskName $taskName `
        -Action $action -Trigger @($repeat, $atLogon) `
        -Settings $settings -Principal $principal -Force `
        -Description "Rayman 工作树自动快照：$Script:Workspace（每 $interval 分钟）" | Out-Null

    Write-Host "已注册计划任务 '$taskName'：每 $interval 分钟快照 $Script:Workspace。" -ForegroundColor Green
    Write-Host "可在『任务计划程序』里查看/停用，或用菜单 [5] 卸载。"
}

function Uninstall-AutosaveTask {
    $taskName = Get-TaskName -Workspace $Script:Workspace
    $existing = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
    if (-not $existing) {
        Write-Host "没有找到计划任务 '$taskName'。" -ForegroundColor Yellow
        return
    }
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false
    Write-Host "已卸载计划任务 '$taskName'。" -ForegroundColor Green
}

function Get-Backends {
    $cfg = Join-Path $EvalsDir 'backends.json'
    if (-not (Test-Path $cfg)) {
        Write-Host "找不到 $cfg。请先按 evals/README.md 配置后端。" -ForegroundColor Red
        return @()
    }
    try {
        $json = Get-Content -Raw $cfg | ConvertFrom-Json
        return @($json.backends.PSObject.Properties.Name)
    } catch {
        Write-Host "解析 backends.json 失败：$_" -ForegroundColor Red
        return @()
    }
}

function Invoke-RunEval {
    $names = Get-Backends
    if (-not $names -or $names.Count -eq 0) { return }
    Write-Host "可用后端：" -ForegroundColor Cyan
    for ($i = 0; $i -lt $names.Count; $i++) {
        Write-Host ("  [{0}] {1}" -f ($i + 1), $names[$i])
    }
    $pick = Read-Host "选择后端编号（回车取消）"
    if (-not $pick) { return }
    $idx = ($pick -as [int]) - 1
    if ($idx -lt 0 -or $idx -ge $names.Count) { Write-Host "无效选择。" -ForegroundColor Yellow; return }
    $backend = $names[$idx]

    $trialsRaw = Read-Host "每格重复次数 trials（回车=1）"
    $trials = 1
    if ($trialsRaw -and ($trialsRaw -as [int])) { $trials = [int]$trialsRaw }

    $taskRaw = Read-Host "只跑某个任务名？（回车=全部任务）"

    Write-Host "运行：--backend $backend --trials $trials $(if ($taskRaw) { "--task $taskRaw" })" -ForegroundColor Cyan
    Write-Host "（这会消耗对应后端的额度）" -ForegroundColor Yellow
    Push-Location $EvalsDir
    try {
        $argv = @('run', '--', '--backend', $backend, '--trials', "$trials")
        if ($taskRaw) { $argv += @('--task', $taskRaw) }
        & cargo @argv
    }
    finally { Pop-Location }
}

function Set-Workspace {
    $p = Read-Host "输入要快照的工作区路径（回车=当前 $Script:Workspace）"
    if ($p) {
        if (Test-Path $p) { $Script:Workspace = (Resolve-Path $p).Path; Write-Host "工作区已设为 $Script:Workspace" -ForegroundColor Green }
        else { Write-Host "路径不存在：$p" -ForegroundColor Red }
    }
}

function Show-Menu {
    Write-Host ""
    Write-Host "==== Rayman 控制台 ====" -ForegroundColor Magenta
    Write-Host "  工作区: $Script:Workspace"
    Write-Host "  保留快照数(keep): $Script:Keep"
    Write-Host "-----------------------"
    Write-Host "  [1] 立即保存快照"
    Write-Host "  [2] 查看快照列表"
    Write-Host "  [3] 恢复最近快照"
    Write-Host "  [4] 安装/更新 自动保存计划任务"
    Write-Host "  [5] 卸载 自动保存计划任务"
    Write-Host "  [6] 运行 eval（选择 yunyi/deepseek 等后端）"
    Write-Host "  [W] 切换工作区"
    Write-Host "  [0] 退出"
    Write-Host ""
}

while ($true) {
    Show-Menu
    $choice = Read-Host "选择"
    switch ($choice) {
        '1' { Invoke-SaveNow }
        '2' { Invoke-List }
        '3' { Invoke-RestoreLatest }
        '4' { Install-AutosaveTask }
        '5' { Uninstall-AutosaveTask }
        '6' { Invoke-RunEval }
        'W' { Set-Workspace }
        'w' { Set-Workspace }
        '0' { break }
        default { Write-Host "无效选择。" -ForegroundColor Yellow }
    }
}
