use std::fs;
use std::io::{self, Read};
use std::os::unix::process::ExitStatusExt;
use std::sync::{
    atomic::{AtomicI32, Ordering},
    Arc,
};

use log::debug;

use crate::job_control::{
    set_process_group_explicit, wait_for_process_group, SignalMaskGuard, TerminalGuard,
    TermiosGuard, WaitOutcome, WaitResult,
};
use crate::parse::CommandSpec;

mod redirection;
mod sandbox;
mod spawning;

pub use sandbox::{
    apply_sandbox_directive, sandbox_options_for_command, SandboxBackend, SandboxConfig,
    SandboxOptions,
};
pub use spawning::{
    build_command, run_command_in_foreground, spawn_command_background, spawn_command_sandboxed,
    spawn_pipeline_background, spawn_pipeline_sandboxed, wrap_spawn_error,
};

use sandbox::apply_sandbox;
use spawning::build_pipeline_command;

pub struct ForegroundResult {
    pub outcome: WaitOutcome,
    pub status_code: Option<i32>,
    pub pipefail_status: Option<i32>,
    pub pgid: i32,
    pub last_pid: i32,
}

pub struct CaptureResult {
    pub output: String,
    pub status_code: i32,
}

pub fn run_pipeline_capture(
    pipeline: &[CommandSpec],
    fg_pgid: &Arc<AtomicI32>,
    trace: bool,
    sandbox: &SandboxConfig,
) -> io::Result<CaptureResult> {
    debug!("job event=capture start count={}", pipeline.len());
    let mut children = Vec::with_capacity(pipeline.len());
    let mut prev_stdout = None;
    let mut capture_stdout = None;
    let mut pgid: Option<i32> = None;
    let mut last_pid: Option<i32> = None;

    for (idx, cmd) in pipeline.iter().enumerate() {
        let last = idx + 1 == pipeline.len();
        let mut command = build_pipeline_command(cmd, prev_stdout.take(), last, true)?;

        if let Some(id) = pgid {
            set_process_group_explicit(&mut command, id);
        } else {
            set_process_group_explicit(&mut command, 0);
        }
        if let Some(options) = sandbox_options_for_command(cmd, sandbox, trace) {
            apply_sandbox(&mut command, &options)?;
        }
        let mut child = command
            .spawn()
            .map_err(|err| wrap_spawn_error(&cmd.args[0], err))?;
        if trace {
            let pid = child.id();
            let pgid = pgid.unwrap_or(pid as i32);
            eprintln!("trace: spawn sub pid {pid} pgid {pgid}");
        }
        debug!(
            "job event=spawn kind=substitution idx={} pid={} pgid={}",
            idx,
            child.id(),
            pgid.unwrap_or(child.id() as i32)
        );
        if pgid.is_none() {
            let id = child.id() as i32;
            pgid = Some(id);
            fg_pgid.store(id, Ordering::SeqCst);
        }
        if last {
            last_pid = Some(child.id() as i32);
        }
        if last {
            capture_stdout = child.stdout.take();
        } else {
            prev_stdout = child.stdout.take();
        }
        children.push(child);
    }

    let mut output = String::new();
    if let Some(mut stdout) = capture_stdout {
        stdout.read_to_string(&mut output)?;
    }

    let mut status_code = 0;
    for mut child in children {
        let status = child.wait()?;
        if Some(child.id() as i32) == last_pid {
            status_code = exit_status_code(status);
        }
        if !status.success() {
            eprintln!("process exited with {status}");
        }
    }

    fg_pgid.store(0, Ordering::SeqCst);
    debug!("job event=capture done status={}", status_code);
    Ok(CaptureResult {
        output,
        status_code,
    })
}

pub fn run_pipeline(
    pipeline: &[CommandSpec],
    fg_pgid: &Arc<AtomicI32>,
    shell_pgid: i32,
    trace: bool,
    sandbox: &SandboxConfig,
) -> io::Result<ForegroundResult> {
    debug!("job event=pipeline start count={}", pipeline.len());
    let mut prev_stdout = None;
    let mut pgid: Option<i32> = None;
    let mut last_pid: Option<i32> = None;
    let mut handoff_guard: Option<SignalMaskGuard> = None;

    for (idx, cmd) in pipeline.iter().enumerate() {
        let last = idx + 1 == pipeline.len();
        let mut command = build_pipeline_command(cmd, prev_stdout.take(), last, false)?;

        if let Some(id) = pgid {
            set_process_group_explicit(&mut command, id);
        } else {
            set_process_group_explicit(&mut command, 0);
        }
        if let Some(options) = sandbox_options_for_command(cmd, sandbox, trace) {
            apply_sandbox(&mut command, &options)?;
        }
        let mut child = command
            .spawn()
            .map_err(|err| wrap_spawn_error(&cmd.args[0], err))?;
        if trace {
            let pid = child.id();
            let pgid = pgid.unwrap_or(pid as i32);
            eprintln!("trace: spawn pid {pid} pgid {pgid}");
        }
        debug!(
            "job event=spawn kind=foreground idx={} pid={} pgid={}",
            idx,
            child.id(),
            pgid.unwrap_or(child.id() as i32)
        );
        if pgid.is_none() {
            // Block SIGINT/SIGCHLD until the process group is established.
            // Block SIGCHLD during process-group handoff to avoid races.
            handoff_guard = Some(SignalMaskGuard::new()?);
            let id = child.id() as i32;
            pgid = Some(id);
            fg_pgid.store(id, Ordering::SeqCst);
        }
        if idx + 1 == pipeline.len() {
            last_pid = Some(child.id() as i32);
        }
        prev_stdout = child.stdout.take();
    }

    let outcome = if let Some(id) = pgid {
        let _termios_guard = TermiosGuard::new();
        let mut tty_guard = TerminalGuard::new(shell_pgid);
        tty_guard.set_foreground(id)?;
        drop(handoff_guard.take());
        wait_for_process_group(id, pipeline.len(), last_pid.unwrap_or(id))?
    } else {
        WaitResult {
            outcome: WaitOutcome::Exited,
            status_code: Some(0),
            pipefail_status: Some(0),
        }
    };

    fg_pgid.store(0, Ordering::SeqCst);
    debug!(
        "job event=pipeline done pgid={} last_pid={} status={:?}",
        pgid.unwrap_or(0),
        last_pid.unwrap_or(0),
        outcome.status_code
    );
    Ok(ForegroundResult {
        outcome: outcome.outcome,
        status_code: outcome.status_code,
        pipefail_status: outcome.pipefail_status,
        pgid: pgid.unwrap_or(0),
        last_pid: last_pid.unwrap_or(0),
    })
}

pub fn status_from_error(err: &io::Error) -> i32 {
    match err.kind() {
        io::ErrorKind::NotFound => 127,
        io::ErrorKind::PermissionDenied => 126,
        _ => 1,
    }
}

pub fn exit_status_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        code
    } else if let Some(sig) = status.signal() {
        128 + sig
    } else {
        1
    }
}

pub(crate) fn spawn_error_message(cmd: &str, err: &io::Error) -> (String, io::ErrorKind) {
    match err.kind() {
        io::ErrorKind::NotFound => (format!("{cmd}: command not found"), io::ErrorKind::NotFound),
        io::ErrorKind::PermissionDenied => (
            format!("{cmd}: permission denied"),
            io::ErrorKind::PermissionDenied,
        ),
        _ => {
            if cmd.contains('/') {
                if let Ok(meta) = fs::metadata(cmd) {
                    if meta.is_dir() {
                        return (
                            format!("{cmd}: is a directory"),
                            io::ErrorKind::PermissionDenied,
                        );
                    }
                }
            }
            (format!("{cmd}: {err}"), err.kind())
        }
    }
}
