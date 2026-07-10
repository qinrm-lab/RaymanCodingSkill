//! Compatibility exports for filesystem and persistence helpers.
//! 只做转发，不复制实现——同一函数只允许一份实现体。

pub use crate::file_io::{read_json, read_text, write_atomic, write_json};
pub use crate::hash::sha256_file;
pub use crate::pathfmt::display_path;
pub use crate::timefmt::now_iso;
