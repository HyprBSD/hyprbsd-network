//! The window's link to wpa_supplicant: a thread that reads its state and carries out requests.

use crate::trace::trace;
use crate::view::{self, Row};
use crate::wifi::{self, Key, Ssid, Status, Wait, WifiError};
use crate::wpa::{self, Control, EventSource, Events, Socket, WpaError};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

/// How often the state is read again without an event.
const TICK: Duration = Duration::from_secs(5);
/// How long events may keep the state from being read again.
const BUSY: Duration = Duration::from_secs(1);
/// How long the worker waits for an event before it looks for a request.
const POLL: Duration = Duration::from_millis(100);
/// How long a scan may take before it counts as finished without results.
const SCAN_WAIT: Duration = Duration::from_secs(15);
/// How often it tries to reach wpa_supplicant again.
const RETRY: Duration = Duration::from_secs(1);
/// How long after wpa_supplicant says it is ending the worker waits to open it again.
const ENDED: Duration = Duration::from_millis(500);

/// What the window asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    Rescan,
    /// A new network, or a saved one with a new password.
    Join {
        ssid: Ssid,
        password: Option<Vec<u8>>,
        hidden: bool,
    },
    /// A saved network, as saved.
    Connect {
        id: u32,
    },
    Forget {
        ssid: Ssid,
    },
    Autoconnect {
        ssid: Ssid,
        on: bool,
    },
    /// Leaves the network joined, then scans.
    Disconnect,
}

/// Wi-Fi as wpa_supplicant last said.
#[derive(Debug, Clone)]
pub struct Wifi {
    pub status: Status,
    pub rows: Vec<Row>,
    /// A scan asked for hasn't finished.
    pub scanning: bool,
    /// Seconds since a scan last saw any of the networks.
    pub age: Option<u32>,
}

/// What the worker tells the window.
#[derive(Debug)]
pub enum Update {
    /// Wi-Fi's state, or why it can't be read.
    State(Result<Wifi, WifiError>),
    /// How the last request ended.
    Done(Result<(), WifiError>),
}

/// The control and event sockets for wpa_supplicant's first interface.
pub fn open() -> Result<(Socket, Events), WpaError> {
    let iface = wpa::first_interface()?;
    Ok((Socket::open(&iface)?, Socket::open(&iface)?.attach()?))
}

/// Serves the window's requests until it closes, sending the state when it changes and every
/// tick. It connects with `open`, again whenever wpa_supplicant goes away, and half a second
/// later when wpa_supplicant says it is ending.
pub fn run<C, E>(
    mut open: impl FnMut() -> Result<(C, E), WpaError>,
    requests: Receiver<Request>,
    send: impl Fn(Update),
    wait: Wait,
) where
    C: Control,
    E: EventSource,
{
    // The error last sent, so a retry that fails the same way sends nothing.
    let mut shown: Option<String> = None;
    // A request that came while wpa_supplicant couldn't be reached.
    let mut pending = None;
    loop {
        match open() {
            Ok((ctrl, events)) => {
                trace(|| "opened wpa_supplicant's sockets".into());
                shown = None;
                let mut session =
                    Session { ctrl, events, wait, scanning: None, scanned: None, dirty: true, quiet: true, read: None };
                match session.serve(&requests, &send, pending.take()) {
                    Ok(Closed) => return,
                    Err(Lost::Gone) => trace(|| "wpa_supplicant has gone".into()),
                    Err(Lost::Ending) => {
                        trace(|| "wpa_supplicant is ending".into());
                        thread::sleep(ENDED);
                    }
                }
                continue;
            }
            Err(e) => {
                let e = WifiError::from(e);
                trace(|| format!("couldn't open wpa_supplicant's sockets: {e}"));
                if pending.take().is_some() {
                    send(Update::Done(Err(e)));
                    continue;
                }
                if shown.as_deref() != Some(e.to_string().as_str()) {
                    shown = Some(e.to_string());
                    send(Update::State(Err(e)));
                }
            }
        }
        match requests.recv_timeout(RETRY) {
            Ok(request) => pending = Some(request),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// The window closed.
struct Closed;

/// How a session ended without the window closing.
enum Lost {
    /// wpa_supplicant went away, so its sockets must be opened again.
    Gone,
    /// wpa_supplicant said it is ending: its socket takes requests for a moment, but answers none.
    Ending,
}

/// Whether wpa_supplicant can't be reached: no control socket, or nothing answers on it.
pub fn gone(e: &WifiError) -> bool {
    matches!(e, WifiError::Wpa(WpaError::NoSocket(_) | WpaError::NotRunning(_)))
}

/// One connection to wpa_supplicant.
struct Session<C, E> {
    ctrl: C,
    events: E,
    wait: Wait,
    /// When the scan asked for started.
    scanning: Option<Instant>,
    /// When a scan was last asked for.
    scanned: Option<Instant>,
    /// Something changed since the state was last read.
    dirty: bool,
    /// No event came in the last poll; also true at the start and after a request.
    quiet: bool,
    /// When the state was last read.
    read: Option<Instant>,
}

impl<C: Control, E: EventSource> Session<C, E> {
    /// Answers `first`, then each request, and sends the state when it changes and every tick,
    /// until the window closes or wpa_supplicant goes.
    fn serve(
        &mut self,
        requests: &Receiver<Request>,
        send: &impl Fn(Update),
        first: Option<Request>,
    ) -> Result<Closed, Lost> {
        if let Some(request) = first {
            self.answer(request, send)?;
        }
        loop {
            if self.due() {
                match self.state() {
                    Ok(wifi) => send(Update::State(Ok(wifi))),
                    Err(e) if gone(&e) => return Err(Lost::Gone),
                    Err(e) => send(Update::State(Err(e))),
                }
            }
            let next = self.events.next_event(POLL);
            if let Ok(Some(event)) = &next {
                trace(|| format!("event {event}"));
            }
            match next {
                Ok(Some(event)) if event.starts_with("CTRL-EVENT-TERMINATING") => return Err(Lost::Ending),
                Ok(Some(event)) => self.event(&event),
                Ok(None) => self.quiet = true,
                Err(_) => return Err(Lost::Gone),
            }
            if self.scanning.is_some_and(|since| since.elapsed() >= SCAN_WAIT) {
                self.scanning = None;
                self.dirty = true;
            }
            match requests.try_recv() {
                Ok(request) => self.answer(request, send)?,
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => return Ok(Closed),
            }
        }
    }

    fn scan(&mut self) -> Result<(), WifiError> {
        self.scanned = Some(Instant::now());
        wifi::scan(&mut self.ctrl)?;
        self.scanning = Some(Instant::now());
        Ok(())
    }

    /// Whether to read the state now: something changed and events have stopped coming, or
    /// have kept coming a while, or nothing has been read for a tick, or a rescan has come due
    /// since the last read.
    fn due(&self) -> bool {
        let since = self.read.map_or(Duration::MAX, |read| read.elapsed());
        let rescan = self.rescan_at().is_some_and(|at| at <= Instant::now() && self.read.is_none_or(|read| read < at));
        (self.dirty && (self.quiet || since >= BUSY)) || since >= TICK || rescan
    }

    /// When a rescan comes due: `rescan` after the last scan asked for; none before the first.
    fn rescan_at(&self) -> Option<Instant> {
        self.scanned.map(|since| since + self.wait.rescan)
    }

    /// Scans unless a scan is running or a rescan isn't due yet. Only a lost wpa_supplicant is
    /// an error.
    fn rescan(&mut self) -> Result<(), WifiError> {
        if self.scanning.is_some() || self.rescan_at().is_some_and(|at| Instant::now() < at) {
            return Ok(());
        }
        match self.scan() {
            Err(e) if gone(&e) => Err(e),
            Err(e) => {
                trace(|| format!("rescan failed: {e}"));
                Ok(())
            }
            Ok(()) => Ok(()),
        }
    }

    /// Reads the state, and scans first if nothing is joined and a rescan is due: a scan while
    /// joined can drop the connection.
    fn state(&mut self) -> Result<Wifi, WifiError> {
        self.dirty = false;
        self.read = Some(Instant::now());
        let status = wifi::status(&mut self.ctrl)?;
        if !status.joined() {
            self.rescan()?;
        }
        let scan = wifi::scan_results(&mut self.ctrl)?;
        let saved = wifi::saved(&mut self.ctrl)?;
        Ok(Wifi {
            rows: view::rows(&scan.networks, &saved, &status),
            status,
            scanning: self.scanning.is_some(),
            age: scan.age,
        })
    }

    fn event(&mut self, event: &str) {
        if event.starts_with("CTRL-EVENT-SCAN-RESULTS") {
            self.scanning = None;
        }
        self.dirty = true;
        self.quiet = false;
    }

    /// Carries out a request and sends how it ended.
    fn answer(&mut self, request: Request, send: &impl Fn(Update)) -> Result<(), Lost> {
        let result = self.carry_out(request);
        self.dirty = true;
        self.quiet = true;
        let went = result.as_ref().is_err_and(gone);
        send(Update::Done(result));
        if went { Err(Lost::Gone) } else { Ok(()) }
    }

    fn carry_out(&mut self, request: Request) -> Result<(), WifiError> {
        match request {
            Request::Rescan => self.scan(),
            Request::Join { ssid, password, hidden } => {
                let key = password.as_deref().map_or(Key::Open, Key::Passphrase);
                wifi::join(&mut self.ctrl, &mut self.events, &ssid, key, hidden, &self.wait).map(drop)
            }
            Request::Connect { id } => wifi::connect(&mut self.ctrl, &mut self.events, id, &self.wait).map(drop),
            Request::Forget { ssid } => wifi::forget(&mut self.ctrl, &ssid).map(drop),
            Request::Autoconnect { ssid, on } => wifi::set_autoconnect(&mut self.ctrl, &ssid, on),
            Request::Disconnect => wifi::disconnect(&mut self.ctrl).and_then(|()| self.scan()),
        }
    }
}
