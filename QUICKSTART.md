# Quickstart

## 1. Build

```text
cargo build --release
```

Add `target/release` to `PATH`, or call the binary through Cargo while developing:

```text
cargo run -p rayman-cli --bin rayman -- session status
```

After building a release, refresh the installed CLI and host skill entries:

```text
target/release/rayman agent-skill sync
target/release/rayman agent-skill status
```

## 2. Configure Secrets

Create `.env` from `.env.example` and fill the provider keys you use:

```text
OPENAI_API_KEY=replace_me
ANTHROPIC_API_KEY=replace_me
RAYMAN_API_KEY=replace_me
THIRD_PARTY_A_API_KEY=replace_me
THIRD_PARTY_B_API_KEY=replace_me
```

Provider base URLs and model names live in `config/default_config.yaml` and `config/models.yaml`.

## 3. First Commands

```text
rayman session status
rayman workspace-skill mark-used -m "explicit raymancodingskill use"
rayman route-models --task code_review
rayman list-models
rayman research start "Investigate validation risk"
```

## 4. Generate, Review, And Test

```text
rayman generate "Create a checksum helper" -l rust -o checksum.rs
rayman review checksum.rs -l rust
rayman test checksum.rs -l rust -o checksum_tests.rs
```

## 5. Run The API

```text
rayman api serve --host 127.0.0.1 --port 8000
```

Protected `/api/*` endpoints require `RAYMAN_API_KEY` or `RAYMAN_API_TOKEN`.

## 6. Validate The Repository

```text
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
rayman audit
```
