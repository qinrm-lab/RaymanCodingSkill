/// Collapse runs of whitespace so the tokenizer sees a tidy stream.
pub fn normalize_spaces(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}
