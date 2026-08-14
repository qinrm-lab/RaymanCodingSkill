[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Root', 'Evals')]
    [string]$Suite
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Canonical Cargo quality command declarations shared by the fast repository
# gate and the complete audit. Keep the command table in this independent
# script scope so callers in ConstrainedLanguage never need to dot-source code.
$commands = if ($Suite -eq 'Root') {
    @(
        [ordered]@{
            name = 'fmt'
            argv = [string[]]@('fmt', '--all', '--check')
        },
        [ordered]@{
            name = 'clippy'
            argv = [string[]]@(
                'clippy', '--locked', '--workspace', '--all-targets',
                '--all-features', '--', '-D', 'warnings'
            )
        },
        [ordered]@{
            name = 'test'
            argv = [string[]]@(
                'test', '--locked', '--workspace', '--all-targets'
            )
        }
    )
} else {
    @(
        [ordered]@{
            name = 'fmt'
            argv = [string[]]@(
                'fmt', '--manifest-path', 'evals/Cargo.toml', '--all', '--check'
            )
        },
        [ordered]@{
            name = 'clippy'
            argv = [string[]]@(
                'clippy', '--manifest-path', 'evals/Cargo.toml', '--locked',
                '--all-targets', '--all-features', '--', '-D', 'warnings'
            )
        },
        [ordered]@{
            name = 'test'
            argv = [string[]]@(
                'test', '--manifest-path', 'evals/Cargo.toml', '--locked',
                '--all-targets'
            )
        }
    )
}

$document = [ordered]@{
    schema = 'rayman.repository-quality.commands.v1'
    suite = $Suite
    commands = $commands
}
Write-Output ($document | ConvertTo-Json -Depth 5 -Compress)
