Implement the `parse_duration` function in `src/lib.rs` so that `cargo test` passes.

`parse_duration` parses a duration string made of a numeric prefix and a single-letter unit (`s` = seconds, `m` = minutes, `h` = hours) into total whole seconds, e.g. `"30s"` -> `Some(30)`, `"5m"` -> `Some(300)`. Return `None` for malformed input (missing/unknown unit, non-numeric prefix, empty string). Do not modify the tests.
