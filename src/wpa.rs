//! wpa_supplicant's control socket: requests, replies and events.

use std::fs;
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// The directory of wpa_supplicant's control sockets (`ctrl_interface`'s DIR).
pub const CTRL_DIR: &str = "/var/run/wpa_supplicant";
/// How long a request waits for its reply.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(10);
/// How often a wait for wpa_supplicant checks that its socket is still there.
const CHECK: Duration = Duration::from_millis(250);
/// Room for the longest reply, 4096 bytes; a longer event is cut short.
const BUF_LEN: usize = 8192;

#[derive(Debug, thiserror::Error)]
pub enum WpaError {
    #[error("Wi-Fi isn't set up: wpa_supplicant has no control socket at {} (hyprbsd-network-helper wifi-setup sets it up)", .0.display())]
    NoSocket(PathBuf),
    #[error("only members of the wheel group can see or change Wi-Fi settings ({})", .0.display())]
    Denied(PathBuf),
    #[error("wpa_supplicant isn't running ({} doesn't answer)", .0.display())]
    NotRunning(PathBuf),
    #[error("wpa_supplicant didn't answer {0} in time")]
    Timeout(String),
    #[error("wpa_supplicant refused {command}: {reply}")]
    Refused { command: String, reply: String },
    #[error("wpa_supplicant's reply to {command} made no sense: {reply:?}")]
    BadReply { command: String, reply: String },
    #[error("couldn't use wpa_supplicant's control socket: {0}")]
    Io(#[from] io::Error),
}

/// A connection to wpa_supplicant that takes requests.
pub trait Control {
    /// Sends `command` and returns wpa_supplicant's reply.
    fn request(&mut self, command: &str) -> Result<String, WpaError>;

    /// Sends `command` and expects OK.
    fn ok(&mut self, command: &str) -> Result<(), WpaError> {
        let reply = self.request(command)?;
        expect_ok(command, &reply)
    }
}

/// A command's first word, the only part of it errors show.
pub fn verb(command: &str) -> &str {
    command.split_once(' ').map_or(command, |(verb, _)| verb)
}

/// OK as a result; anything else as an error naming the command's verb.
pub fn expect_ok(command: &str, reply: &str) -> Result<(), WpaError> {
    if reply.trim_end() == "OK" { Ok(()) } else { Err(unexpected(command, reply)) }
}

/// A reply that isn't the one wanted: FAIL as a refusal, anything else as a bad reply.
pub fn unexpected(command: &str, reply: &str) -> WpaError {
    let (command, reply) = (verb(command).to_owned(), reply.trim_end().to_owned());
    if reply.starts_with("FAIL") { WpaError::Refused { command, reply } } else { WpaError::BadReply { command, reply } }
}

/// A control socket connection: our own socket, bound in a directory, connected to wpa_supplicant's.
#[derive(Debug)]
pub struct Socket {
    sock: UnixDatagram,
    dir: PathBuf,
    local: PathBuf,
    server: PathBuf,
    /// `server`'s file when connected, as (device, inode).
    server_id: (u64, u64),
    /// How long a request waits for its reply.
    timeout: Duration,
    /// A request timed out on `sock`: the next one renews it first.
    stale: bool,
}

impl Socket {
    /// Connects to wpa_supplicant's socket for `iface`, from a socket in `$XDG_RUNTIME_DIR` (or /tmp).
    pub fn open(iface: &str) -> Result<Self, WpaError> {
        Self::connect(&ctrl_dir().join(iface), &runtime_dir())
    }

    /// Connects to the control socket `server` from a new socket in `dir`.
    pub fn connect(server: &Path, dir: &Path) -> Result<Self, WpaError> {
        Self::connect_then(server, dir, || {})
    }

    /// `connect`, running `between` once connected, before it reads `server`'s file again.
    fn connect_then(server: &Path, dir: &Path, between: impl FnOnce()) -> Result<Self, WpaError> {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let local = dir.join(format!("hyprbsd-network-{}-{}", process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let sock = match UnixDatagram::bind(&local) {
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
                fs::remove_file(&local)?;
                UnixDatagram::bind(&local)?
            }
            bound => bound?,
        };
        let mut socket = Socket {
            sock,
            dir: dir.to_owned(),
            local,
            server: server.to_owned(),
            server_id: (0, 0),
            timeout: REPLY_TIMEOUT,
            stale: false,
        };
        socket.server_id = file_id(server).map_err(|e| socket.lost(e))?;
        socket.sock.connect(server).map_err(|e| socket.lost(e))?;
        between();
        if file_id(server).map_err(|e| socket.lost(e))? != socket.server_id {
            return Err(WpaError::NotRunning(socket.server.clone()));
        }
        Ok(socket)
    }

    /// Sets how long a request waits for its reply.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Turns this connection into one that receives wpa_supplicant's events.
    pub fn attach(mut self) -> Result<Events, WpaError> {
        self.ok("ATTACH")?;
        Ok(Events { socket: self })
    }

    /// Replaces our socket with a new one, connected the same way; replies sent to the old one
    /// are lost.
    fn renew(&mut self) -> Result<(), WpaError> {
        let mut fresh = Socket::connect(&self.server, &self.dir)?;
        fresh.timeout = self.timeout;
        *self = fresh;
        Ok(())
    }

    fn send(&self, command: &str) -> Result<(), WpaError> {
        self.sock.send(command.as_bytes()).map(drop).map_err(|e| self.lost(e))
    }

    /// The next message within `timeout`, or None; NotRunning once wpa_supplicant's socket file
    /// has gone or been replaced.
    fn recv(&self, timeout: Duration) -> Result<Option<String>, WpaError> {
        let deadline = Instant::now() + timeout;
        let mut buf = vec![0; BUF_LEN];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            self.sock.set_read_timeout(Some(left.min(CHECK)))?;
            match self.sock.recv(&mut buf) {
                Ok(n) => return Ok(Some(String::from_utf8_lossy(&buf[..n]).into_owned())),
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
                Err(e) => return Err(self.lost(e)),
            }
            if !matches!(file_id(&self.server), Ok(id) if id == self.server_id) {
                return Err(WpaError::NotRunning(self.server.clone()));
            }
        }
    }

    /// The error for a failed connect, send or receive on wpa_supplicant's socket.
    fn lost(&self, e: io::Error) -> WpaError {
        let server = self.server.clone();
        match e.kind() {
            io::ErrorKind::NotFound => WpaError::NoSocket(server),
            io::ErrorKind::PermissionDenied => WpaError::Denied(server),
            io::ErrorKind::ConnectionRefused | io::ErrorKind::ConnectionReset | io::ErrorKind::NotConnected => {
                WpaError::NotRunning(server)
            }
            _ => WpaError::Io(e),
        }
    }
}

impl Control for Socket {
    fn request(&mut self, command: &str) -> Result<String, WpaError> {
        if self.stale {
            self.renew()?;
        }
        self.send(command)?;
        loop {
            match self.recv(self.timeout)? {
                None => {
                    self.stale = true;
                    return Err(WpaError::Timeout(verb(command).into()));
                }
                Some(msg) if msg.starts_with('<') => {}
                Some(reply) => return Ok(reply),
            }
        }
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.local);
    }
}

/// Where wpa_supplicant's events come from.
pub trait EventSource {
    /// The next event's text without its `<level>` prefix, or None if none comes within `timeout`.
    fn next_event(&mut self, timeout: Duration) -> Result<Option<String>, WpaError>;
}

/// A connection attached to wpa_supplicant's events.
#[derive(Debug)]
pub struct Events {
    socket: Socket,
}

impl Events {
    /// The next event's text without its `<level>` prefix, or None if none comes within `timeout`.
    pub fn wait(&mut self, timeout: Duration) -> Result<Option<String>, WpaError> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            match self.socket.recv(left)? {
                None => return Ok(None),
                Some(msg) => {
                    if let Some(event) = msg.strip_prefix('<').and_then(|m| m.split_once('>')).map(|(_, e)| e) {
                        return Ok(Some(event.to_owned()));
                    }
                }
            }
        }
    }

    /// Waits for an event whose text starts with `name`; false if none comes within `timeout`.
    pub fn wait_for(&mut self, name: &str, timeout: Duration) -> Result<bool, WpaError> {
        let deadline = Instant::now() + timeout;
        while let Some(event) = self.wait(deadline.saturating_duration_since(Instant::now()))? {
            if event.starts_with(name) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl EventSource for Events {
    fn next_event(&mut self, timeout: Duration) -> Result<Option<String>, WpaError> {
        self.wait(timeout)
    }
}

impl Drop for Events {
    fn drop(&mut self) {
        let _ = self.socket.send("DETACH");
    }
}

/// A file's (device, inode).
fn file_id(path: &Path) -> io::Result<(u64, u64)> {
    let meta = fs::metadata(path)?;
    Ok((meta.dev(), meta.ino()))
}

/// The interfaces wpa_supplicant has control sockets for in `dir`, sorted.
pub fn interfaces(dir: &Path) -> Result<Vec<String>, WpaError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => return Err(WpaError::Denied(dir.to_owned())),
        Err(e) => return Err(e.into()),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_socket() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// The directory of wpa_supplicant's control sockets: `HYPRBSD_NETWORK_CTRL_DIR`, else CTRL_DIR.
pub fn ctrl_dir() -> PathBuf {
    std::env::var_os("HYPRBSD_NETWORK_CTRL_DIR").map_or_else(|| PathBuf::from(CTRL_DIR), PathBuf::from)
}

/// The first interface wpa_supplicant has a control socket for.
pub fn first_interface() -> Result<String, WpaError> {
    let dir = ctrl_dir();
    interfaces(&dir)?.into_iter().next().ok_or(WpaError::NoSocket(dir))
}

/// `$XDG_RUNTIME_DIR` if it's set to an absolute path, else /tmp.
pub(crate) fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new directory for a test's sockets.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hyprbsd-network-wpa-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        dir
    }

    #[test]
    fn a_socket_replaced_while_connecting_isnt_waited_on() {
        let dir = scratch("replaced");
        let server = dir.join("wlan0");
        let _old = UnixDatagram::bind(&server).unwrap();
        let mut _new = None;

        let connected = Socket::connect_then(&server, &dir, || {
            fs::remove_file(&server).unwrap();
            _new = Some(UnixDatagram::bind(&server).unwrap());
        });

        let started = Instant::now();
        let result = connected.and_then(|mut socket| socket.request("STATUS"));
        let waited = started.elapsed();
        let _ = fs::remove_dir_all(&dir);
        assert!(
            matches!(result, Err(WpaError::NotRunning(_))) && waited < Duration::from_secs(2),
            "{result:?} after {waited:?}"
        );
    }
}
