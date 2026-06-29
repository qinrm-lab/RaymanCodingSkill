use std::process::Command;

use anyhow::Result;

pub fn install_required_tools(tools: &[String]) -> Result<Vec<(String, bool)>> {
    let mut out = Vec::new();
    for tool in tools {
        let ok = command_exists(tool);
        out.push((tool.clone(), ok));
    }
    Ok(out)
}

fn command_exists(tool: &str) -> bool {
    if cfg!(windows) {
        Command::new("where.exe")
            .arg(tool)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    } else {
        Command::new("which")
            .arg(tool)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }
}
