//! Optional data-only bridge. Application validation remains in this process;
//! only recognized state files and their locks are routed to the global owner.
use anyhow::{Result, bail};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone)]
struct Route {
    root: PathBuf,
    workspace: PathBuf,
    object: String,
}
thread_local! {
    static LEASES:RefCell<BTreeMap<(PathBuf,String),String>>=const {RefCell::new(BTreeMap::new())};
    static READS:RefCell<BTreeMap<(PathBuf,String),String>>=const {RefCell::new(BTreeMap::new())};
}

fn route(path: &Path) -> Result<Option<Route>> {
    let Some(state) = path
        .ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == ".RaymanCodingSkill"))
    else {
        return Ok(None);
    };
    let Some(workspace) = state.parent() else {
        return Ok(None);
    };
    let relative = path
        .strip_prefix(state)?
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("state path is not Unicode"))?
        .replace('\\', "/");
    let object = match relative.as_str() {
        "pending.json" => "pending".to_string(),
        "context/index.json" => "context".to_string(),
        "goals/.store" => "goals-store".to_string(),
        _ => {
            let Some(id) = relative
                .strip_prefix("goals/")
                .and_then(|s| s.strip_suffix(".json"))
            else {
                return Ok(None);
            };
            if !id.strip_prefix("goal_").is_some_and(|v| {
                v.len() == 10
                    && v.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            }) {
                return Ok(None);
            };
            id.to_string()
        }
    };
    // The marker is a routing hint, never authorization; the global client
    // independently checks the protected registration and physical workspace.
    if !state.join("global-state.json").is_file() {
        return Ok(None);
    };
    let root = std::env::var_os("RAYMAN_GLOBAL_EXECUTION_ROOT")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            anyhow::anyhow!("global state is enabled but its endpoint is not configured")
        })?;
    if !root.is_absolute() {
        bail!("global state endpoint must be absolute");
    }
    Ok(Some(Route {
        root,
        workspace: std::fs::canonicalize(workspace)?,
        object,
    }))
}

fn call(
    route: &Route,
    action: &str,
    lease: Option<&str>,
    expected: Option<&str>,
    bytes: Option<&[u8]>,
    no_wait: bool,
) -> Result<serde_json::Value> {
    let program = route.root.join("client.exe");
    let mut command = std::process::Command::new(program);
    command
        .args(["--format", "json", "app-state", "--root"])
        .arg(&route.root)
        .arg("--workspace")
        .arg(&route.workspace)
        .args(["--action", action, "--object", &route.object]);
    if let Some(lease) = lease {
        command.args(["--lease-id", lease]);
    }
    if let Some(expected) = expected {
        command.args(["--expected-sha256", expected]);
    }
    if no_wait {
        command.arg("--no-wait");
    }
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("state client stdin unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("state client stdout unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("state client stderr unavailable"))?;
    let output = std::thread::scope(|scope| -> Result<std::process::Output> {
        let writer = scope.spawn(move || {
            let mut stdin = stdin;
            stdin.write_all(bytes.unwrap_or_default())
        });
        let reader = |stream: Box<dyn Read + Send>| -> std::io::Result<Vec<u8>> {
            let mut out = Vec::new();
            stream.take(2 * 1024 * 1024 + 1).read_to_end(&mut out)?;
            Ok(out)
        };
        let out = scope.spawn(move || reader(Box::new(stdout)));
        let err = scope.spawn(move || reader(Box::new(stderr)));
        let limit = std::time::Duration::from_secs(if action == "write" { 300 } else { 15 });
        let started = std::time::Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if started.elapsed() < limit => {
                    std::thread::sleep(std::time::Duration::from_millis(20))
                }
                result => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(anyhow::anyhow!(
                        "global state client failed or timed out during {action}: {result:?}"
                    ));
                }
            }
        };
        let written = writer
            .join()
            .map_err(|_| anyhow::anyhow!("state input thread failed"))?;
        let stdout = out
            .join()
            .map_err(|_| anyhow::anyhow!("state output thread failed"))??;
        let stderr = err
            .join()
            .map_err(|_| anyhow::anyhow!("state diagnostic thread failed"))??;
        let status = status?;
        if stdout.len() > 2 * 1024 * 1024 || stderr.len() > 2 * 1024 * 1024 {
            bail!("global state client output exceeded its bound");
        }
        if status.success() {
            written?;
        }
        Ok(std::process::Output {
            status,
            stdout,
            stderr,
        })
    })?;
    if !output.status.success() {
        bail!(
            "global state operation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

pub(crate) struct RemoteLock {
    route: Route,
    lease: String,
    stop: std::sync::mpsc::Sender<()>,
    renewal: Option<std::thread::JoinHandle<Result<()>>>,
    // The active lease registry is thread-local; transferring a guard to
    // another thread would release the wrong registry and strand its writer.
    _thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl Drop for RemoteLock {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(renewal) = self.renewal.take() {
            match renewal.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => eprintln!("Global state lease renewal failed: {error:#}"),
                Err(_) => eprintln!("Global state lease renewal thread failed"),
            }
        }
        LEASES.with_borrow_mut(|leases| {
            let key = (self.route.workspace.clone(), self.route.object.clone());
            if leases.get(&key) == Some(&self.lease) {
                leases.remove(&key);
            }
        });
        if let Err(error) = call(&self.route, "release", Some(&self.lease), None, None, true) {
            eprintln!(
                "Global state lease release could not be queued; its bounded lease will expire: {error:#}"
            );
        }
    }
}

pub(crate) fn acquire(path: &Path) -> Result<Option<RemoteLock>> {
    let Some(route) = route(path)? else {
        return Ok(None);
    };
    let response = call(&route, "acquire", None, None, None, false)?;
    let lease = response["lease_id"]
        .as_str()
        .filter(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| anyhow::anyhow!("invalid global lease response"))?
        .to_string();
    LEASES.with_borrow_mut(|leases| {
        leases.insert(
            (route.workspace.clone(), route.object.clone()),
            lease.clone(),
        );
    });
    let (stop, receiver) = std::sync::mpsc::channel();
    let renew_route = route.clone();
    let renew_lease = lease.clone();
    let renewal = std::thread::spawn(move || {
        renew_until_stopped(receiver, std::time::Duration::from_secs(30), || {
            call(&renew_route, "renew", Some(&renew_lease), None, None, false)?;
            Ok(())
        })
    });
    Ok(Some(RemoteLock {
        route,
        lease,
        stop,
        renewal: Some(renewal),
        _thread_bound: std::marker::PhantomData,
    }))
}

fn renew_until_stopped(
    receiver: std::sync::mpsc::Receiver<()>,
    interval: std::time::Duration,
    mut renew: impl FnMut() -> Result<()>,
) -> Result<()> {
    loop {
        match receiver.recv_timeout(interval) {
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => renew()?,
        }
    }
}

pub(crate) fn observe(path: &Path, bytes: &[u8]) {
    if let Ok(Some(route)) = route(path) {
        READS.with_borrow_mut(|reads| {
            reads.insert(
                (route.workspace, route.object),
                crate::hash::sha256_bytes(bytes),
            );
        });
    }
}

pub(crate) fn write(path: &Path, text: &str) -> Result<bool> {
    let Some(route) = route(path)? else {
        return Ok(false);
    };
    let key = (route.workspace.clone(), route.object.clone());
    // Context refresh historically relies on snapshot/CAS validation rather
    // than an outer StateLock. Acquire its physical write lease here.
    let _context_lock =
        if route.object == "context" && !LEASES.with_borrow(|leases| leases.contains_key(&key)) {
            acquire(path)?
        } else {
            None
        };
    let lease = LEASES
        .with_borrow(|leases| {
            leases
                .get(&key)
                .or_else(|| {
                    if route.object.starts_with("goal_") {
                        leases.get(&(route.workspace.clone(), "goals-store".into()))
                    } else {
                        None
                    }
                })
                .cloned()
        })
        .ok_or_else(|| anyhow::anyhow!("global state write requires this thread's active lease"))?;
    let expected = READS.with_borrow(|reads| reads.get(&key).cloned());
    if expected.is_none() && path.try_exists()? {
        bail!("global state overwrite requires a prior verified read");
    }
    let expected_new = crate::hash::sha256_bytes(text.as_bytes());
    let response = call(
        &route,
        "write",
        Some(&lease),
        expected.as_deref(),
        Some(text.as_bytes()),
        false,
    )?;
    if response["written"] != true || response["sha256"].as_str() != Some(expected_new.as_str()) {
        bail!("global state write receipt differs");
    }
    let (observed, _) =
        crate::file_io::read_handle_bound_file(path, "global state write readback")?;
    if observed != text.as_bytes() {
        bail!("global state bytes differ after publication");
    }
    READS.with_borrow_mut(|reads| {
        reads.insert(key, expected_new);
    });
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn state_lease_renewal_stops_promptly_and_propagates_failure() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let count = Arc::new(AtomicUsize::new(0));
        let observed = count.clone();
        let (stop, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            renew_until_stopped(receive, std::time::Duration::from_millis(5), || {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        });
        let started = std::time::Instant::now();
        while count.load(Ordering::SeqCst) < 2 {
            assert!(started.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        stop.send(()).unwrap();
        worker.join().unwrap().unwrap();
        let at_stop = count.load(Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(at_stop, count.load(Ordering::SeqCst));
        let (_stop, receive) = std::sync::mpsc::channel();
        assert!(
            renew_until_stopped(receive, std::time::Duration::from_millis(1), || bail!(
                "renewal rejected"
            ))
            .unwrap_err()
            .to_string()
            .contains("renewal rejected")
        );
    }
}
