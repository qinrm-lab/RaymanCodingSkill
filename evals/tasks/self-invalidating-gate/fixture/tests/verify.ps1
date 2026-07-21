$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$requirements = Join-Path $root 'requirements.md'
$normalizer = Join-Path $root 'normalize.ps1'

& $normalizer
$first = (Get-FileHash -Algorithm SHA256 -LiteralPath $requirements).Hash
& $normalizer
$second = (Get-FileHash -Algorithm SHA256 -LiteralPath $requirements).Hash
if ($first -ne $second) {
    throw 'normalizer is not byte-stable on repeated execution'
}

Set-Content -LiteralPath $requirements -Value '# Project requirements' -Encoding utf8NoBOM
& $normalizer
if (-not (Select-String -LiteralPath $requirements -SimpleMatch 'Normalized: yes' -Quiet)) {
    throw 'normalizer no longer produces the required marker'
}
$repaired = (Get-FileHash -Algorithm SHA256 -LiteralPath $requirements).Hash
& $normalizer
$stable = (Get-FileHash -Algorithm SHA256 -LiteralPath $requirements).Hash
if ($repaired -ne $stable) {
    throw 'normalizer changes an already normalized file'
}
