use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use clap::ValueEnum;

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum Language {
    /// Follow RAYMAN_LANG, the process locale, and finally the OS user locale.
    Auto,
    /// Simplified Chinese user interface.
    #[value(name = "zh-CN", alias = "zh", alias = "zh-cn", alias = "zh_CN")]
    ZhCn,
    /// English user interface.
    #[value(name = "en", alias = "en-US", alias = "en-us", alias = "en_US")]
    En,
}

impl std::fmt::Display for Language {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Auto => "auto",
            Self::ZhCn => "zh-CN",
            Self::En => "en",
        })
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum ActiveLanguage {
    ZhCn = 1,
    En = 2,
}

static ACTIVE_LANGUAGE: AtomicU8 = AtomicU8::new(ActiveLanguage::ZhCn as u8);
static JSON_OUTPUT: AtomicBool = AtomicBool::new(false);

pub fn configure(requested: Language, json_output: bool) -> bool {
    let resolved = resolve(requested);
    ACTIVE_LANGUAGE.store(resolved as u8, Ordering::Relaxed);
    JSON_OUTPUT.store(json_output, Ordering::Relaxed);
    json_output
}

fn resolve(requested: Language) -> ActiveLanguage {
    match requested {
        Language::ZhCn => ActiveLanguage::ZhCn,
        Language::En => ActiveLanguage::En,
        Language::Auto => resolve_auto_language(),
    }
}

fn resolve_auto_language() -> ActiveLanguage {
    for variable in ["RAYMAN_LANG", "LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(variable)
            && let Some(language) = language_from_locale(&value)
        {
            return language;
        }
    }

    #[cfg(windows)]
    if let Some(locale) = windows_user_locale()
        && let Some(language) = language_from_locale(&locale)
    {
        return language;
    }

    // Chinese is the fail-safe default when a host exposes no locale metadata.
    ActiveLanguage::ZhCn
}

fn language_from_locale(locale: &str) -> Option<ActiveLanguage> {
    let normalized = locale.trim().replace('_', "-").to_ascii_lowercase();
    if normalized.is_empty() || normalized == "auto" {
        return None;
    }
    if normalized == "zh" || normalized.starts_with("zh-") {
        Some(ActiveLanguage::ZhCn)
    } else {
        // English is the deterministic fallback until another catalog exists.
        Some(ActiveLanguage::En)
    }
}

#[cfg(windows)]
fn windows_user_locale() -> Option<String> {
    const LOCALE_NAME_MAX_LENGTH: usize = 85;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserDefaultLocaleName(locale_name: *mut u16, locale_name_count: i32) -> i32;
    }

    let mut buffer = [0_u16; LOCALE_NAME_MAX_LENGTH];
    // SAFETY: the writable buffer and exact capacity are passed to the API.
    let count =
        unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), LOCALE_NAME_MAX_LENGTH as i32) };
    if count <= 1 || count as usize > buffer.len() {
        return None;
    }
    String::from_utf16(&buffer[..count as usize - 1]).ok()
}

pub fn localize_line(line: String) -> String {
    let language = match ACTIVE_LANGUAGE.load(Ordering::Relaxed) {
        value if value == ActiveLanguage::En as u8 => ActiveLanguage::En,
        _ => ActiveLanguage::ZhCn,
    };
    localize_line_for(line, language, JSON_OUTPUT.load(Ordering::Relaxed))
}

fn localize_line_for(line: String, language: ActiveLanguage, json_output: bool) -> String {
    if json_output {
        return line;
    }

    let indentation_end = line
        .char_indices()
        .find_map(|(index, character)| (!character.is_whitespace()).then_some(index))
        .unwrap_or(line.len());
    let (indentation, content) = line.split_at(indentation_end);

    for &(chinese, english) in UI_PREFIXES {
        let (source, target) = match language {
            ActiveLanguage::ZhCn => (english, chinese),
            ActiveLanguage::En => (chinese, english),
        };
        if let Some(remainder) = content.strip_prefix(source) {
            return format!("{indentation}{target}{remainder}");
        }
    }
    line
}

// Prefixes are anchored after indentation. Dynamic values are not searched or
// globally replaced, so goal titles and Unicode paths remain byte-for-byte intact.
const UI_PREFIXES: &[(&str, &str)] = &[
    (
        "当前工作区暂无快照。运行 `rayman checkpoint save` 创建一个。",
        "No workspace checkpoint exists. Run `rayman checkpoint save` to create one.",
    ),
    (
        "资产扫描: 干净（无过时候选、无未完成标记）。",
        "Asset scan: clean (no stale candidates or work-in-progress markers).",
    ),
    (
        "发布交接状态: 未检查（本结果仅是工作区 strict-quality）",
        "Release handoff: not checked (workspace strict-quality only)",
    ),
    (
        "运行 `rayman context refresh` 更新索引。",
        "Run `rayman context refresh` to update the index.",
    ),
    ("当前工作区暂无快照。", "No workspace checkpoint exists."),
    (
        "无托管临时目录可清理。",
        "No managed temp directory to clean.",
    ),
    ("已清理托管临时目录。", "Managed temp directory cleaned."),
    ("无待完成项。", "No pending items."),
    ("暂无 current 目标。", "No current goals."),
    ("暂无目标。", "No goals."),
    (
        "过时资产候选（提示，不自动删除）:",
        "Stale asset candidates (advisory; never auto-deleted):",
    ),
    (
        "候选相关测试(启发式):",
        "Candidate related tests (heuristic):",
    ),
    (
        "RaymanCodingSkill 工作区激活:",
        "RaymanCodingSkill workspace activation:",
    ),
    ("工作区就绪:", "Workspace readiness:"),
    ("任务准备完成:", "Task preparation complete:"),
    ("上下文索引:", "Context index:"),
    ("索引已刷新:", "Index refreshed:"),
    ("托管临时目录:", "Managed temp directory:"),
    ("受管状态审计:", "Managed state audit:"),
    ("项目地图已刷新:", "Project map refreshed:"),
    ("快照（旧→新）:", "Checkpoints (oldest to newest):"),
    ("最近完整快照:", "Latest complete checkpoint:"),
    ("已创建目标", "Goal created"),
    ("已记录待完成项", "Pending item recorded"),
    ("已解决待完成项。", "Pending item resolved."),
    ("待完成项:", "Pending items:"),
    ("资产扫描:", "Asset scan:"),
    ("未完成标记:", "Work-in-progress markers:"),
    ("项目地图:", "Project map:"),
    ("文件:", "File:"),
    ("符号匹配:", "Symbol matches:"),
    ("未找到符号:", "Symbol not found:"),
    ("影响分析:", "Impact analysis:"),
    ("变更计划:", "Change plan:"),
    ("项目质量(", "Project quality("),
    ("建议验证:", "Recommended validation:"),
    ("建议依据:", "Recommendation basis:"),
    ("文件分组:", "File groups:"),
    ("风险提示:", "Risk summary:"),
    ("风险:", "Risks:"),
    ("符号:", "Symbols:"),
    ("错误:", "Error:"),
    ("问题:", "Issue:"),
    ("源码错误:", "Source error:"),
    ("源码:", "Source:"),
    ("激活:", "Activation:"),
    ("命令:", "Command:"),
    ("阻断:", "BLOCKER:"),
    ("任务阻断:", "TASK BLOCKER:"),
    ("警告:", "Warning:"),
    ("包:", "Package:"),
    ("阻断项:", "Blockers:"),
    ("警告项:", "Warnings:"),
    ("按角色统计:", "Findings by role:"),
    ("未删除任何文件", "No files were deleted"),
];

macro_rules! println {
    () => {
        std::println!()
    };
    ($($argument:tt)*) => {{
        let rendered = format!($($argument)*);
        std::println!("{}", crate::i18n::localize_line(rendered));
    }};
}

macro_rules! eprintln {
    () => {
        std::eprintln!()
    };
    ($($argument:tt)*) => {{
        let rendered = format!($($argument)*);
        std::eprintln!("{}", crate::i18n::localize_line(rendered));
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_parser_prefers_chinese_for_all_zh_variants() {
        assert_eq!(
            language_from_locale("zh_CN.UTF-8"),
            Some(ActiveLanguage::ZhCn)
        );
        assert_eq!(language_from_locale("zh-TW"), Some(ActiveLanguage::ZhCn));
        assert_eq!(
            language_from_locale("en_US.UTF-8"),
            Some(ActiveLanguage::En)
        );
    }

    #[test]
    fn prefix_translation_preserves_dynamic_unicode_values() {
        assert_eq!(
            localize_line_for("文件: 中文目录/项目🙂.rs".into(), ActiveLanguage::En, false,),
            "File: 中文目录/项目🙂.rs"
        );
        assert_eq!(
            localize_line_for(
                "  Source error: 中文内容🙂".into(),
                ActiveLanguage::ZhCn,
                false,
            ),
            "  源码错误: 中文内容🙂"
        );
    }

    #[test]
    fn json_output_is_never_localized() {
        assert_eq!(
            localize_line_for(
                r#"{"Error:":"File: 中文"}"#.into(),
                ActiveLanguage::ZhCn,
                true,
            ),
            r#"{"Error:":"File: 中文"}"#
        );
    }
}
