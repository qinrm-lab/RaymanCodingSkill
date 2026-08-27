# GitHub Actions workflow-context source

Status: active
Review date: 2026-08-27
Valid until: 2027-02-27

## SRC-GITHUB-ACTIONS-CONTEXT

Repository policy for `.github/workflows/ci.yml` binds context expressions to
the scope in which GitHub evaluates them. Runner-specific values such as
`runner.temp` belong to a step expression/run boundary, not a job-level `env`
mapping that is evaluated before a runner exists. The release-contract job
must also obtain activation through the product-owned
`workspace activate --skill-file ... --yes` command; a workflow must not
manufacture `workspace_skill.yaml` fields independently of the CLI schema.

The traceability checker compares the current manifest with immutable Git
history. Jobs that run it directly, through root tests, through coverage, or
through the repository gate must check out full history with `fetch-depth: 0`;
a shallow checkout cannot silently downgrade retirement verification.

The first-party test inventory contains Windows-only, Linux-only, general
Unix, and explicit non-Linux-Unix cases. The authoritative `check`,
`test-traceability`, and standalone `evals` matrices therefore retain Windows,
Linux, and macOS runners. Removing macOS would leave the non-Linux-Unix tests
present but unreachable from a real gate, which is a release-blocking orphan.

This is a repository-captured semantic policy, not proof that a YAML file will
be accepted by GitHub. The local workflow checker and a real GitHub Actions run
are separate evidence layers.
