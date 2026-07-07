`src/lib.rs` contains a private helper function that is never used, which produces a `dead_code` warning.

Remove the unused/dead code so that `cargo clippy --all-targets -- -D warnings` passes cleanly, without breaking the existing public API or the tests. Do not delete the `greeting` function or the test.
