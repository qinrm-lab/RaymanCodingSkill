/// Reads the retry-limit setting from the deploy-time config file. `raw` comes
/// straight from an operator-edited config file, not from anything validated
/// upstream.
pub fn load_retry_limit(raw: &str) -> u32 {
    raw.trim().parse::<u32>().unwrap()
}
