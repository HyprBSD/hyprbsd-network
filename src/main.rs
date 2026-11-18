//! hyprbsd-network: Wi-Fi and network settings for FreeBSD desktops.
//!
//!   hyprbsd-network                         the window; asks an open one to come forward
//!   hyprbsd-network list [--scan]           networks in range, strongest first; scans unless
//!                                           joined, or with --scan
//!   hyprbsd-network join [--hidden] name    join a network; its password is the first line
//!                                           of standard input, unless that's a terminal
//!   hyprbsd-network forget name             forget a saved network
//!   hyprbsd-network status [--follow]       how the machine is connected; --follow prints
//!                                           Waybar's JSON line at each change
//!
//! SPDX-License-Identifier: BSD-2-Clause

use hyprbsd_network::iface::IfaceError;
use hyprbsd_network::instance::{self, Instance};
use hyprbsd_network::status;
use hyprbsd_network::ui;
use hyprbsd_network::wifi::{self, Key, Network, Security, Ssid, Strength, Wait, WifiError};
use hyprbsd_network::wpa::{self, Socket, WpaError};
use std::ffi::OsString;
use std::io::{self, BufRead, IsTerminal};
use std::os::unix::ffi::OsStrExt;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const USAGE: &str = "\
usage: hyprbsd-network
       hyprbsd-network list [--scan]
       hyprbsd-network join [--hidden] name
       hyprbsd-network forget name
       hyprbsd-network status [--follow]";

/// How long `list` and `join` wait for a scan.
const SCAN_WAIT: Duration = Duration::from_secs(15);
/// The signals that end the program, which stop a join instead.
const ENDING: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Window,
    List { scan: bool },
    Join { name: Vec<u8>, hidden: bool },
    Forget { name: Vec<u8> },
    Status { follow: bool },
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("{USAGE}")]
    Usage,
    #[error("\"{0}\" isn't in range (a hidden network: hyprbsd-network join --hidden)")]
    NotInRange(String),
    #[error("\"{name}\": {why}")]
    CantJoin { name: String, why: &'static str },
    #[error("\"{0}\" needs a password: join it from the window, or pipe the password to standard input")]
    NeedsPassword(String),
    #[error(
        "\"{0}\" is open here, yet a password was given: it may not be the network you saved (leave the password out to join it)"
    )]
    OpenHere(String),
    #[error(transparent)]
    Wifi(#[from] WifiError),
    #[error(transparent)]
    Wpa(#[from] WpaError),
    #[error(transparent)]
    Iface(#[from] IfaceError),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("couldn't open the window: {0}")]
    Window(#[from] eframe::Error),
}

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match parse(&args).and_then(run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Error::Usage) => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
        Err(e) => {
            eprintln!("hyprbsd-network: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse(args: &[OsString]) -> Result<Command, Error> {
    let words: Vec<&[u8]> = args.iter().map(|a| a.as_bytes()).collect();
    Ok(match words.as_slice() {
        [] => Command::Window,
        [b"list"] => Command::List { scan: false },
        [b"list", b"--scan"] => Command::List { scan: true },
        [b"join", b"--hidden"] => return Err(Error::Usage),
        [b"join", b"--hidden", name] => Command::Join { name: name.to_vec(), hidden: true },
        [b"join", name] => Command::Join { name: name.to_vec(), hidden: false },
        [b"forget", name] => Command::Forget { name: name.to_vec() },
        [b"status"] => Command::Status { follow: false },
        [b"status", b"--follow"] => Command::Status { follow: true },
        _ => return Err(Error::Usage),
    })
}

fn run(command: Command) -> Result<(), Error> {
    match command {
        Command::Window => window(),
        Command::List { scan } => list(scan),
        Command::Join { name, hidden } => join(&Ssid::new(&name)?, hidden),
        Command::Forget { name } => forget(&Ssid::new(&name)?),
        Command::Status { follow: false } => {
            println!("{}", status::current()?.lines().join("\n"));
            Ok(())
        }
        Command::Status { follow: true } => Ok(status::follow(&mut io::stdout().lock())?),
    }
}

/// Opens the window, unless one is open: that one is asked to come forward.
fn window() -> Result<(), Error> {
    match instance::claim(&instance::path())? {
        Instance::First(window) => Ok(ui::run(&window)?),
        Instance::Running => Ok(()),
    }
}

/// Scans and waits for the results, or for 15 s; says so if the scan didn't finish by then.
fn rescan(iface: &str, ctrl: &mut Socket) -> Result<(), Error> {
    let mut events = Socket::open(iface)?.attach()?;
    wifi::scan(ctrl)?;
    if !events.wait_for("CTRL-EVENT-SCAN-RESULTS", SCAN_WAIT)? {
        eprintln!("hyprbsd-network: the scan didn't finish in 15 s; these are the last results");
    }
    Ok(())
}

fn list(scan: bool) -> Result<(), Error> {
    let iface = wpa::first_interface()?;
    let mut ctrl = Socket::open(&iface)?;
    if scan || !wifi::status(&mut ctrl)?.joined() {
        rescan(&iface, &mut ctrl)?;
    } else {
        eprintln!(
            "hyprbsd-network: these are the last scan's networks; scanning while joined can drop the connection for a moment (list --scan scans anyway)"
        );
    }
    let networks = wifi::networks(&mut ctrl)?;
    let saved = wifi::saved(&mut ctrl)?;
    let status = wifi::status(&mut ctrl)?;
    let current = if status.joined() { status.ssid } else { None };
    if networks.is_empty() {
        println!("No networks in range.");
    }
    for n in &networks {
        let note = if current.as_ref() == Some(&n.ssid) {
            " (connected)"
        } else if saved.iter().any(|s| s.ssid == n.ssid) {
            " (saved)"
        } else {
            ""
        };
        println!("{}", list_line(n, note));
    }
    Ok(())
}

fn list_line(network: &Network, note: &str) -> String {
    format!(
        "{:<6}  {:<19}  {}{note}",
        Strength::from_dbm(network.signal).describe(),
        network.security.describe(),
        network.ssid.display()
    )
}

fn join(ssid: &Ssid, hidden: bool) -> Result<(), Error> {
    let name = ssid.display();
    let iface = wpa::first_interface()?;
    let mut ctrl = Socket::open(&iface)?;
    let mut events = Socket::open(&iface)?.attach()?;
    let given =
        if io::stdin().is_terminal() { None } else { Some(line(&mut io::stdin().lock())?).filter(|p| !p.is_empty()) };
    let wait = Wait { stop: Some(&STOP), ..Wait::default() };
    let saved = wifi::saved(&mut ctrl)?.into_iter().rev().find(|s| s.ssid == *ssid);
    let joined = match (given, saved) {
        (None, Some(saved)) => {
            stop_joins_on_signals();
            wifi::connect(&mut ctrl, &mut events, saved.id, &wait)?
        }
        (given, _) => {
            let security = if hidden { None } else { Some(security(&iface, &mut ctrl, ssid)?) };
            if let Some(why) = security.and_then(Security::refusal) {
                return Err(Error::CantJoin { name, why });
            }
            let password = password(&name, security, given)?;
            let key = password.as_deref().map_or(Key::Open, Key::Passphrase);
            stop_joins_on_signals();
            wifi::join(&mut ctrl, &mut events, ssid, key, hidden, &wait)?
        }
    };
    match joined.address {
        Some(address) => println!("Joined {name}: {address}"),
        None => println!("Joined {name}; no address from DHCP yet"),
    }
    Ok(())
}

/// A network's security from the scan results, scanning again if it isn't there.
fn security(iface: &str, ctrl: &mut Socket, ssid: &Ssid) -> Result<Security, Error> {
    if let Some(security) = wifi::security_of(&wifi::networks(ctrl)?, ssid) {
        return Ok(security);
    }
    rescan(iface, ctrl)?;
    wifi::security_of(&wifi::networks(ctrl)?, ssid).ok_or_else(|| Error::NotInRange(ssid.display()))
}

/// The password to join with: none for an open network, where one given is refused, else the one
/// given. A hidden network's security isn't known (None), and none given means it's open.
fn password(name: &str, security: Option<Security>, given: Option<Vec<u8>>) -> Result<Option<Vec<u8>>, Error> {
    match (security, given) {
        (Some(Security::Open), Some(_)) => Err(Error::OpenHere(name.to_owned())),
        (Some(Security::Open), None) => Ok(None),
        (_, Some(password)) => Ok(Some(password)),
        (None, None) => Ok(None),
        (Some(_), None) => Err(Error::NeedsPassword(name.to_owned())),
    }
}

/// The first line of `input`, without its newline.
fn line(input: &mut impl BufRead) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    input.read_until(b'\n', &mut line)?;
    if line.ends_with(b"\n") {
        line.pop();
        if line.ends_with(b"\r") {
            line.pop();
        }
    }
    Ok(line)
}

/// Set by a signal during a join, which then stops and undoes itself.
static STOP: AtomicBool = AtomicBool::new(false);

/// A signal handler: the join in progress stops.
extern "C" fn stop_join(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

/// Makes the signals that end the program stop a join.
fn stop_joins_on_signals() {
    for signal in ENDING {
        // SAFETY: stop_join only stores to an atomic, which is async-signal-safe.
        unsafe { libc::signal(signal, stop_join as extern "C" fn(libc::c_int) as libc::sighandler_t) };
    }
}

fn forget(ssid: &Ssid) -> Result<(), Error> {
    let mut ctrl = Socket::open(&wpa::first_interface()?)?;
    wifi::forget(&mut ctrl, ssid)?;
    println!("Forgot {}", ssid.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    mod parse {
        use super::*;

        #[test]
        fn status_follow() {
            assert_eq!(parse(&args(&["status", "--follow"])).unwrap(), Command::Status { follow: true });
        }

        #[test]
        fn join_hidden_without_a_name_is_usage() {
            assert!(matches!(parse(&args(&["join", "--hidden"])), Err(Error::Usage)));
        }

        #[test]
        fn forget_without_a_name_is_usage() {
            assert!(matches!(parse(&args(&["forget"])), Err(Error::Usage)));
        }
    }

    mod line {
        use super::*;

        #[test]
        fn drops_the_newline() {
            assert_eq!(line(&mut &b"correct horse\n"[..]).unwrap(), b"correct horse");
        }

        #[test]
        fn keeps_trailing_spaces() {
            assert_eq!(line(&mut &b"correct horse  \r\n"[..]).unwrap(), b"correct horse  ");
        }

        #[test]
        fn reads_only_the_first_line() {
            assert_eq!(line(&mut &b"correct horse\n\n"[..]).unwrap(), b"correct horse");
        }

        #[test]
        fn keeps_bytes_that_arent_utf8() {
            assert_eq!(line(&mut &b"caf\xe9 horse\n"[..]).unwrap(), b"caf\xe9 horse");
        }
    }

    mod password {
        use super::*;

        #[test]
        fn none_given_for_a_hidden_network_is_open() {
            assert_eq!(password("Secret", None, None).unwrap(), None);
        }
    }
}
