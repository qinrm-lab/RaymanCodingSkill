Implement the `parse_kv` function in `src/lib.rs` so that `cargo test` passes.

`parse_kv` parses a `key=value` line into `Some((key, value))`, trimming surrounding whitespace from both. It must split on the FIRST `=` only, and return `None` when there is no `=`. Do not modify the tests.
