use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Subcommand, ValueEnum};
use rayman_core::eval::{AgentEvalManager, AgentEvalProfile};
use rayman_core::gate::{GateManager, GateOptions};
use rayman_core::release::{ReleaseEvidenceManager, ReleaseEvidenceOptions};
use rayman_core::security::SecurityAuditManager;

use crate::runtime::{root, write_or_print};

#[derive(Subcommand)]
pub(crate) enum EvalCommand {
    Run {
        #[arg(long = "profile", value_enum, default_value = "core")]
        profile: EvalRunProfile,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EvalRunProfile {
    Core,
    Full,
}

impl From<EvalRunProfile> for AgentEvalProfile {
    fn from(value: EvalRunProfile) -> Self {
        match value {
            EvalRunProfile::Core => AgentEvalProfile::Core,
            EvalRunProfile::Full => AgentEvalProfile::Full,
        }
    }
}

#[derive(Subcommand)]
pub(crate) enum SecurityCommand {
    Audit,
}

#[derive(Subcommand)]
pub(crate) enum ReleaseCommand {
    Evidence {
        #[arg(long = "label", default_value = "manual")]
        label: String,
        #[arg(short = 'o', long = "output")]
        output: Option<PathBuf>,
        #[arg(long = "no-write")]
        no_write: bool,
        #[arg(long = "sbom")]
        sbom: Option<PathBuf>,
        #[arg(long = "attestation")]
        attestation: Option<PathBuf>,
        #[arg(long = "signed")]
        signed: bool,
        #[arg(long = "require-provenance")]
        require_provenance: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum GateCommand {
    Status {
        #[arg(long = "check")]
        check: bool,
        #[arg(long = "format", value_enum, default_value = "text")]
        format: GateOutputFormat,
        #[arg(long = "require-provenance")]
        require_provenance: bool,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GateOutputFormat {
    Text,
    Json,
}

pub(crate) fn cmd_eval(command: EvalCommand) -> Result<()> {
    match command {
        EvalCommand::Run { profile } => {
            let manager = AgentEvalManager::new(root()?)?;
            let report = manager.assert_passed(profile.into())?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
    }
    Ok(())
}

pub(crate) fn cmd_security(command: SecurityCommand) -> Result<()> {
    match command {
        SecurityCommand::Audit => {
            let manager = SecurityAuditManager::new(root()?)?;
            let report = manager.assert_passed()?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
    }
    Ok(())
}

pub(crate) fn cmd_release(command: ReleaseCommand) -> Result<()> {
    match command {
        ReleaseCommand::Evidence {
            label,
            output,
            no_write,
            sbom,
            attestation,
            signed,
            require_provenance,
        } => {
            let manager = ReleaseEvidenceManager::new(root()?)?;
            let report = manager.generate_with_options(ReleaseEvidenceOptions {
                label,
                write_default: !no_write && output.is_none(),
                sbom_path: sbom,
                attestation_path: attestation,
                signed,
                require_provenance,
            })?;
            let text = serde_json::to_string_pretty(&report)?;
            if let Some(output) = output {
                write_or_print(Some(&output), "release evidence written", &text)?;
            } else {
                println!("{text}");
            }
            if report.status != "ready" {
                bail!(
                    "release evidence is partial: {}",
                    report.required_actions.join("; ")
                );
            }
        }
    }
    Ok(())
}

pub(crate) fn cmd_gate(command: GateCommand) -> Result<()> {
    match command {
        GateCommand::Status {
            check,
            format,
            require_provenance,
        } => {
            let manager = GateManager::new(root()?)?;
            let options = GateOptions { require_provenance };
            let report = if check {
                match format {
                    GateOutputFormat::Text => {
                        eprintln!("RaymanCodingSkill readiness gate: running checks");
                        manager.assert_passed_with_progress(options, |id, title| {
                            eprintln!("  running {id}: {title}");
                        })?
                    }
                    GateOutputFormat::Json => manager.assert_passed(options)?,
                }
            } else {
                manager.status(options)?
            };
            match format {
                GateOutputFormat::Json => println!("{}", serde_json::to_string_pretty(&report)?),
                GateOutputFormat::Text => print_gate_report(&report),
            }
        }
    }
    Ok(())
}

fn print_gate_report(report: &rayman_core::gate::GateReport) {
    println!("RaymanCodingSkill readiness gate: {}", report.status);
    println!("  工作区: {}", report.workspace_path);
    println!(
        "  checks={} blockers={} warnings={}",
        report.check_count, report.blocking_count, report.warning_count
    );
    for check in &report.checks {
        println!(
            "  [{}] {} ({}) - {}",
            check.status, check.id, check.severity, check.summary
        );
        for action in &check.required_actions {
            println!("    - {action}");
        }
    }
}
