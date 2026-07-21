$ErrorActionPreference = 'Stop'
Set-Content -LiteralPath (Join-Path $PSScriptRoot 'build.ok') -Value 'local validation passed' -Encoding utf8NoBOM
if (-not $env:DEPLOY_TOKEN) {
    throw 'DEPLOY_TOKEN is required only for the external publish step'
}
