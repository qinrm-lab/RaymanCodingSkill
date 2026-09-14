# Retired PowerShell broker

The single-repository PowerShell broker has been replaced by the installed
Codex global execution backend. Its worker and installer are no longer shipped
or executed by repository validation. Old installer commands do not qualify as
Rayman installation proof, even when an old script remains on disk.

Use the installed `codex-global-execution` skill and protected client at
`C:\ProgramData\Rayman\CodexGlobalExecution\client.exe`. The current backend
supports enrolled workspaces, exact local commits, typed workflow persistence
and registered file publication. Preview the specific operation, inspect its
bound result, and use its recovery command for interrupted transactions.
Worker health alone does not prove an operation succeeded or the latest source
is installed. Push and publication retain separate authorization.

## Existing installations

Source retirement does not uninstall a Windows task or delete ProgramData.
Preserve old receipts, transaction journals, result history and backups. An
unresolved legacy transaction must be investigated against its exact installed
version; do not feed it to the new backend or force-delete it. The committed
historical version remains available in Git for that investigation.

Before separately authorized host cleanup, verify the replacement on the owner
desktop, positively identify the old task and installed tuple, and resolve all
legacy recovery debt. A missing task result from a sandbox is insufficient.
Do not reinstall or restart the retired broker as a fallback.
