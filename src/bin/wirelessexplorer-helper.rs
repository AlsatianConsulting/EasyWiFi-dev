use anyhow::Result;
use std::io::{self, BufRead, Write};
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::thread;
use std::time::Duration;
use wirelessexplorer::capture;
use wirelessexplorer::privilege::{HelperRequest, HelperResponse};

fn main() -> Result<()> {
    install_parent_death_signal()?;
    start_origin_parent_watchdog();
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("daemon") => run_daemon(),
        Some("hop") => run_channel_hop(args.collect()),
        Some("tshark") => run_passthrough("tshark", args.collect()),
        _ => {
            eprintln!("usage: wirelessexplorer-helper daemon | hop <iface> <dwell_ms> <ht_mode> <channel...> | tshark <args...>");
            std::process::exit(2);
        }
    }
}

fn install_parent_death_signal() -> Result<()> {
    unsafe {
        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
            return Err(io::Error::last_os_error().into());
        }
        if libc::getppid() == 1 {
            std::process::exit(1);
        }
    }
    Ok(())
}

fn configure_parent_death_signal(command: &mut Command) {
    let parent_pid = std::process::id();
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::getppid() as u32 != parent_pid {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "helper parent exited before child exec",
                ));
            }
            Ok(())
        });
    }
}

fn start_origin_parent_watchdog() {
    let Some(parent_pid) = std::env::var("WIRELESSEXPLORER_PARENT_PID")
        .ok()
        .or_else(|| std::env::var("SIMPLESTG_PARENT_PID").ok())
        .and_then(|value| value.parse::<u32>().ok())
    else {
        return;
    };

    thread::spawn(move || loop {
        let status = unsafe { libc::kill(parent_pid as i32, 0) };
        if status != 0 {
            let err = io::Error::last_os_error().raw_os_error();
            if err == Some(libc::ESRCH) {
                std::process::exit(0);
            }
        }
        thread::sleep(Duration::from_millis(500));
    });
}

fn run_daemon() -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let request = match serde_json::from_str::<HelperRequest>(&line) {
            Ok(req) => req,
            Err(err) => {
                write_response(
                    &mut stdout,
                    &HelperResponse::err(format!("invalid request: {err}")),
                )?;
                continue;
            }
        };

        let response = match request {
            HelperRequest::Ping => HelperResponse::ok(Some("pong".to_string())),
            HelperRequest::CurrentInterfaceType { interface } => {
                HelperResponse::ok(capture::current_interface_type(&interface))
            }
            HelperRequest::SetMonitorMode {
                interface,
                monitor_name,
            } => match capture::set_interface_monitor_mode_direct(
                &interface,
                monitor_name.as_deref(),
            ) {
                Ok(active) => HelperResponse::ok(Some(active)),
                Err(err) => HelperResponse::err(err.to_string()),
            },
            HelperRequest::SetChannel {
                interface,
                channel,
                ht_mode,
            } => match capture::set_channel_with_ht_direct(&interface, channel, &ht_mode) {
                Ok(()) => HelperResponse::ok(Some("ok".to_string())),
                Err(err) => HelperResponse::err(err.to_string()),
            },
            HelperRequest::SetInterfaceType { interface, if_type } => {
                match capture::set_interface_type_direct(&interface, &if_type) {
                    Ok(()) => HelperResponse::ok(Some("ok".to_string())),
                    Err(err) => HelperResponse::err(err.to_string()),
                }
            }
            HelperRequest::Shutdown => {
                write_response(&mut stdout, &HelperResponse::ok(Some("bye".to_string())))?;
                return Ok(());
            }
        };

        write_response(&mut stdout, &response)?;
    }

    Ok(())
}

fn write_response(stdout: &mut dyn Write, response: &HelperResponse) -> Result<()> {
    serde_json::to_writer(&mut *stdout, response)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

fn run_passthrough(program: &str, args: Vec<String>) -> Result<()> {
    let mut command = Command::new(program);
    command.args(&args);
    configure_parent_death_signal(&mut command);
    let status = command.status()?;
    std::process::exit(status.code().unwrap_or(1));
}

fn run_channel_hop(args: Vec<String>) -> Result<()> {
    if args.len() < 4 {
        anyhow::bail!(
            "usage: wirelessexplorer-helper hop <iface> <dwell_ms> <ht_mode> <channel...>"
        );
    }

    let interface = &args[0];
    let dwell_ms = args[1].parse::<u64>().unwrap_or(200).max(50);
    let ht_mode = &args[2];
    let mut channels = args[3..]
        .iter()
        .filter_map(|value| value.parse::<u16>().ok())
        .collect::<Vec<_>>();

    if channels.is_empty() {
        anyhow::bail!("no valid channels provided for hop mode");
    }

    let mut index = 0usize;
    loop {
        let channel = channels[index % channels.len()];
        if let Err(err) = capture::set_channel_with_ht_direct(interface, channel, ht_mode) {
            eprintln!(
                "channel hop set failed on {} channel {} ({}): {}",
                interface, channel, ht_mode, err
            );
            channels.retain(|candidate| *candidate != channel);
            if channels.is_empty() {
                anyhow::bail!(
                    "channel hopper exhausted all channels on {} after removing invalid channel {}",
                    interface,
                    channel
                );
            }
            eprintln!(
                "removed invalid channel {} from hopper on {}; {} channels remain",
                channel,
                interface,
                channels.len()
            );
            if index >= channels.len() {
                index = 0;
            }
        }
        index += 1;
        thread::sleep(Duration::from_millis(dwell_ms));
    }
}
