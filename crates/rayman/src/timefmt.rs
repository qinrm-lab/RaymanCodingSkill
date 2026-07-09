//! Timestamp formatting helpers.

use chrono::{SecondsFormat, Utc};

pub fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_iso_uses_utc_timestamp() {
        let timestamp = now_iso();
        assert!(timestamp.contains('T'));
        assert!(timestamp.ends_with('Z'));
    }
}
