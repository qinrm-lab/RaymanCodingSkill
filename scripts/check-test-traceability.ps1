[CmdletBinding()]
param(
    [string]$ManifestPath = (Join-Path (Split-Path -Parent $PSScriptRoot) 'governance/test-traceability.json'),
    [string]$InventoryPath = (Join-Path (Split-Path -Parent $PSScriptRoot) 'governance/first-party-test-inventory.json'),
    [switch]$SelfTest,
    [switch]$RuntimeInventory,
    [switch]$ListInventory,
    [switch]$Generate,
    [string]$DraftInventoryPath,
    [string]$DraftManifestPath,
    [string]$OutputDirectory,
    [switch]$Summary
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Keep the repository gate entrypoint stable while the implementation lives in
# a separately hash-bound file. The delegate receives structured parameters;
# no command text is constructed or evaluated.
$implementation = Join-Path $PSScriptRoot 'check-test-traceability-v2.ps1'
if (-not (Test-Path -LiteralPath $implementation -PathType Leaf)) {
    throw "Traceability implementation is missing: $implementation"
}
$implementationHash = (Get-FileHash -LiteralPath $implementation -Algorithm SHA256).Hash.ToLowerInvariant()
if ($implementationHash -cne '432df5031f08dae7cdab583ac2698e9841f9f8f2f8538df315903aaec7ac5a13') {
    throw "Traceability implementation hash drifted: $implementationHash"
}
& $implementation @PSBoundParameters
