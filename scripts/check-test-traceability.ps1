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
$implementationHash = (Get-FileHash -LiteralPath $implementation -Algorithm SHA256).Hash.ToLowerInvariant()
if ($implementationHash -cne '895acacff0846f443b75a93e789126a9c2c7bddb5e7317ee23cb12cf67949da1') {
    throw "Traceability implementation hash drifted: $implementationHash"
}
& $implementation @PSBoundParameters
