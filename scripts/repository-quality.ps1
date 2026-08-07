# Canonical Cargo quality command declarations shared by the fast repository
# gate and the complete audit. Callers retain their own native-identity and
# phase-reporting policies; only argv is centralized here.

function Get-RepositoryQualityCommands {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet('Root', 'Evals')]
        [string]$Suite
    )

    if ($Suite -eq 'Root') {
        return @(
            [pscustomobject]@{
                Name = 'fmt'
                Arguments = [string[]]@('fmt', '--all', '--check')
            },
            [pscustomobject]@{
                Name = 'clippy'
                Arguments = [string[]]@(
                    'clippy', '--locked', '--workspace', '--all-targets',
                    '--all-features', '--', '-D', 'warnings'
                )
            },
            [pscustomobject]@{
                Name = 'test'
                Arguments = [string[]]@(
                    'test', '--locked', '--workspace', '--all-targets'
                )
            }
        )
    }

    return @(
        [pscustomobject]@{
            Name = 'fmt'
            Arguments = [string[]]@(
                'fmt', '--manifest-path', 'evals/Cargo.toml', '--all', '--check'
            )
        },
        [pscustomobject]@{
            Name = 'clippy'
            Arguments = [string[]]@(
                'clippy', '--manifest-path', 'evals/Cargo.toml', '--locked',
                '--all-targets', '--all-features', '--', '-D', 'warnings'
            )
        },
        [pscustomobject]@{
            Name = 'test'
            Arguments = [string[]]@(
                'test', '--manifest-path', 'evals/Cargo.toml', '--locked',
                '--all-targets'
            )
        }
    )
}
