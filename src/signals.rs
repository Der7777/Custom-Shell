use std::io;

use log::debug;
use nix::errno::Errno;
use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};
use nix::unistd::{getpgrp, getpid, getsid, setpgid, setsid, tcsetpgrp, Pid};
use std::os::fd::AsFd;

pub fn install_signal_handlers() -> io::Result<()> {
    let action = SigAction::new(SigHandler::SigIgn, SaFlags::SA_RESTART, SigSet::empty());
    install_action(Signal::SIGINT, &action)?;
    install_action(Signal::SIGTSTP, &action)?;
    install_action(Signal::SIGQUIT, &action)?;
    install_action(Signal::SIGTTIN, &action)?;
    install_action(Signal::SIGTTOU, &action)?;
    debug!("signal event=install mode=ignore");
    Ok(())
}

pub fn init_session(interactive: bool) -> io::Result<i32> {
    let pid = getpid();
    if !interactive {
        let sid = getsid(None).map_err(|err| io::Error::other(err.to_string()))?;
        if sid != pid {
            if let Err(err) = setsid() {
                if err != Errno::EPERM {
                    return Err(io::Error::other(err.to_string()));
                }
            }
        }
    }
    let pgid = getpgrp();
    if interactive && pgid != pid {
        setpgid(Pid::from_raw(0), Pid::from_raw(0))
            .map_err(|err| io::Error::other(err.to_string()))?;
    }
    let pgid = getpgrp();
    let stdin = std::io::stdin();
    let fd = stdin.as_fd();
    if let Err(err) = tcsetpgrp(fd, pgid) {
        if err != Errno::ENOTTY {
            return Err(io::Error::other(err.to_string()));
        }
    }
    Ok(pgid.as_raw())
}

fn install_action(signal: Signal, action: &SigAction) -> io::Result<()> {
    unsafe { sigaction(signal, action) }
        .map(|_| ())
        .map_err(|err| io::Error::other(err.to_string()))
}
