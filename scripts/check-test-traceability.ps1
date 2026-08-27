[CmdletBinding()]
param(
    [string]$ManifestPath = (Join-Path (Split-Path -Parent $PSScriptRoot) 'governance/test-traceability.json'),
    [string]$InventoryPath = (Join-Path (Split-Path -Parent $PSScriptRoot) 'governance/first-party-test-inventory.json'),
    [switch]$SelfTest,
    [switch]$RuntimeInventory,
    [switch]$ListInventory
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
& $implementation @PSBoundParameters
