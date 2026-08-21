use anyhow::{Result, bail};
use chrono::Utc;
use serde::Serialize;

use crate::cli::{UpdateAction, UpdateCmd};
use rayman::update::{
    DEFAULT_AUTO_CHECK_INTERVAL_HOURS, OfficialReleaseProvider, UpdateObservation, UpdateState,
    check_for_update, compiled_release_version,
};

#[derive(Debug, Serialize)]
struct UpdateCommandReport {
    status: &'static str,
    current: rayman::update::ReleaseVersion,
    state: UpdateState,
    #[serde(skip_serializing_if = "Option::is_none")]
    observation: Option<UpdateObservation>,
    checked: bool,
    state_written: bool,
    install_authorized: bool,
    install_ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    worker_launch: Option<rayman::update::install::WorkerLaunch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    install_error: Option<String>,
}

pub fn run(json: bool, cmd: &UpdateCmd) -> Result<()> {
    let current = compiled_release_version();
    let report = match &cmd.action {
        UpdateAction::Status => {
            let state = load_state_read_only()?.unwrap_or_default();
            state.validate()?;
            UpdateCommandReport {
                status: "status",
                current,
                install_authorized: state.auto_install,
                state,
                observation: None,
                checked: false,
                state_written: false,
                install_ready: false,
                worker_launch: None,
                install_error: None,
            }
        }
        UpdateAction::Check => {
            let observation = check_for_update(&OfficialReleaseProvider, current.clone());
            let (state, state_error) = match load_state_read_only() {
                Ok(state) => (state.unwrap_or_default(), None),
                Err(error) => (UpdateState::default(), Some(format!("{error:#}"))),
            };
            UpdateCommandReport {
                status: "checked",
                current,
                state,
                observation: Some(observation),
                checked: true,
                state_written: false,
                install_authorized: false,
                install_ready: false,
                worker_launch: None,
                install_error: state_error,
            }
        }
        UpdateAction::Configure {
            auto_check,
            no_auto_check,
            auto_install,
            no_auto_install,
            interval_hours,
            yes,
        } => {
            if !yes {
                bail!(
                    "update configure writes only the user-level update preference; add --yes to confirm"
                );
            }
            let state_path = rayman::update::state::update_state_path(true)?
                .expect("create=true returns a state path");
            let _lock = rayman::state_lock::acquire_state_lock(&state_path)?;
            let mut state = rayman::update::state::load_update_state()?.unwrap_or_default();
            state.migrate()?;
            if *auto_check {
                state.enable_auto_check(
                    interval_hours.unwrap_or(DEFAULT_AUTO_CHECK_INTERVAL_HOURS),
                )?;
            } else if *no_auto_check {
                state.disable_auto_check();
            }
            if *auto_install {
                // Consent is persisted independently from checking.  The
                // trusted worker still refuses until a pinned production root,
                // signed manifest, verified bundle, and install receipt exist.
                state.enable_auto_install();
            } else if *no_auto_install {
                state.disable_auto_install();
            }
            state.validate()?;
            rayman::update::state::save_update_state(&state_path, &state)?;
            UpdateCommandReport {
                status: "configured",
                current,
                install_authorized: state.auto_install,
                state,
                observation: None,
                checked: false,
                state_written: true,
                install_ready: false,
                worker_launch: None,
                install_error: None,
            }
        }
        UpdateAction::Poll => {
            let poll = (|| -> Result<UpdateCommandReport> {
                if let Some(launch) = rayman::update::install::pending_recovery_launch()? {
                    let state = rayman::update::state::load_update_state()
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    return Ok(UpdateCommandReport {
                        status: "recovery_required",
                        current: current.clone(),
                        state,
                        observation: None,
                        checked: false,
                        state_written: false,
                        install_authorized: false,
                        install_ready: true,
                        worker_launch: Some(launch),
                        install_error: None,
                    });
                }
                let state_path = rayman::update::state::update_state_path(true)?
                    .expect("create=true returns a state path");
                let _lock = rayman::state_lock::acquire_state_lock(&state_path)?;
                let mut state = rayman::update::state::load_update_state()?.unwrap_or_default();
                state.migrate()?;
                let observation =
                    state.poll_if_due(Utc::now(), &OfficialReleaseProvider, current.clone())?;
                let checked = observation.is_some();
                if checked {
                    rayman::update::state::save_update_state(&state_path, &state)?;
                }
                let mut worker_launch = None;
                let mut install_error = None;
                if state.auto_install
                    && rayman::update::install::trusted_install_available()
                    && let Some(UpdateObservation {
                        status: rayman::update::UpdateStatus::UpdateAvailable { latest },
                        ..
                    }) = observation.as_ref()
                {
                    match rayman::update::install::prepare_worker_request(
                        latest.clone(),
                        Utc::now(),
                    ) {
                        Ok(launch) => worker_launch = Some(launch),
                        Err(error) => install_error = Some(format!("{error:#}")),
                    }
                }
                Ok(UpdateCommandReport {
                    status: if checked { "polled" } else { "not_due" },
                    current: current.clone(),
                    observation,
                    checked,
                    state_written: checked,
                    install_authorized: state.auto_install,
                    state,
                    install_ready: worker_launch.is_some(),
                    worker_launch,
                    install_error,
                })
            })();
            match poll {
                Ok(report) => report,
                Err(error) => UpdateCommandReport {
                    status: "state_error",
                    current,
                    state: UpdateState::default(),
                    observation: None,
                    checked: false,
                    state_written: false,
                    install_authorized: false,
                    install_ready: false,
                    worker_launch: None,
                    install_error: Some(format!("{error:#}")),
                },
            }
        }
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_text(&report);
    }
    Ok(())
}

fn print_text(report: &UpdateCommandReport) {
    println!(
        "Rayman update: {} (current={}, auto_check={}, auto_install={})",
        report.status, report.current, report.state.auto_check, report.state.auto_install
    );
    if let Some(observation) = &report.observation {
        println!("  discovery: {}", observation.status.as_str());
        if let Some(prompt) = observation.prompt() {
            println!(
                "  update available: {} -> {} ({})",
                prompt.current, prompt.latest, prompt.release_page
            );
        }
    } else if let Some(observation) = report.state.current_cached_observation(&report.current)
        && let Some(prompt) = observation.prompt()
    {
        println!(
            "  cached update available: {} -> {} ({})",
            prompt.current, prompt.latest, prompt.release_page
        );
    }
    if report.install_authorized && !report.install_ready {
        println!("  automatic installation consent is recorded; no verified install plan is ready");
    }
    if let Some(launch) = &report.worker_launch {
        println!(
            "  verified worker: {} {}",
            launch.program.display(),
            launch.arguments.join(" ")
        );
    }
    if let Some(error) = &report.install_error {
        println!("  automatic installation unavailable: {error}");
    }
}

fn load_state_read_only() -> Result<Option<UpdateState>> {
    rayman::update::state::load_update_state()
}
