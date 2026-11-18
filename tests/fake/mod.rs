//! A model of wpa_supplicant's control interface, as far as hyprbsd-network uses it.

// Each test crate uses part of it.
#![allow(dead_code)]

use hyprbsd_network::wpa::{Control, EventSource, WpaError};
use std::collections::VecDeque;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// A scratch directory under /tmp, removed when dropped.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(what: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir =
            PathBuf::from(format!("/tmp/hn-{what}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&dir).unwrap();
        Scratch(dir)
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The longest reply wpa_supplicant sends.
const REPLY_MAX: usize = 4096;
/// STATUS with nothing joined.
const DISCONNECTED: &str = "wpa_state=DISCONNECTED\naddress=02:00:00:00:00:99\n";

/// A network record in the configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub id: u32,
    pub ssid: Vec<u8>,
    pub key_mgmt: String,
    pub psk: String,
    pub scan_ssid: bool,
    pub disabled: bool,
}

/// A network on the air.
#[derive(Debug, Clone)]
pub struct Air {
    pub ssid: Vec<u8>,
    pub flags: String,
    pub level: i32,
    /// The pre-shared key in hex; empty for an open network.
    pub psk: String,
    /// Sends no name in its beacons.
    pub hidden: bool,
    /// Seconds since a scan last saw it.
    pub age: u32,
}

impl Air {
    pub fn open(ssid: &[u8], level: i32) -> Self {
        Air { ssid: ssid.to_vec(), flags: "[ESS]".into(), level, psk: String::new(), hidden: false, age: 0 }
    }

    pub fn wpa2(ssid: &[u8], level: i32, psk: &str) -> Self {
        Air { ssid: ssid.to_vec(), flags: "[WPA2-PSK-CCMP][ESS]".into(), level, psk: psk.into(), hidden: false, age: 0 }
    }
}

#[derive(Debug, Default)]
pub struct Fake {
    pub records: Vec<Record>,
    pub next_id: u32,
    pub current: Option<u32>,
    /// The record being joined, and the STATUS requests left until it is.
    pub joining: Option<(u32, u32)>,
    /// STATUS requests after a join until DHCP gives an address; None: never.
    pub dhcp_after: Option<u32>,
    pub polls_since_join: u32,
    pub air: Vec<Air>,
    /// The configuration file as last saved.
    pub file: Option<Vec<Record>>,
    pub save_fails: bool,
    /// SCAN is refused, as with the interface down.
    pub scan_fails: bool,
    /// BSS is refused.
    pub bss_fails: bool,
    /// SCAN requests that came while a network was joined.
    pub scans_while_joined: u32,
    /// A network joined drops at the STATUS request after the one that shows it joined, for good.
    pub drops: bool,
    /// STATUS requests a join takes.
    pub join_polls: u32,
    pub log: Vec<String>,
    /// Events not yet read, as wpa_supplicant sends them to attached sockets.
    pub events: Arc<Mutex<VecDeque<String>>>,
    /// A wrong key ends in CTRL-EVENT-SSID-TEMP-DISABLED with reason=WRONG_KEY, as some
    /// drivers report it; others never do.
    pub reports_wrong_key: bool,
    /// The record joined with a wrong key, and the STATUS requests left until that's reported.
    wrong_key: Option<(u32, u32)>,
}

impl Fake {
    pub fn new() -> Self {
        Fake { dhcp_after: Some(1), join_polls: 2, reports_wrong_key: true, ..Fake::default() }
    }

    /// A saved record, as if read from the configuration file at start.
    pub fn with_record(mut self, ssid: &[u8], psk: &str, disabled: bool) -> Self {
        let id = self.next_id;
        self.next_id += 1;
        self.records.push(Record {
            id,
            ssid: ssid.to_vec(),
            key_mgmt: if psk.is_empty() { "NONE" } else { "WPA-PSK" }.into(),
            psk: psk.into(),
            scan_ssid: false,
            disabled,
        });
        self.file = Some(self.records.clone());
        self
    }

    pub fn with_air(mut self, air: Air) -> Self {
        self.air.push(air);
        self
    }

    pub fn record(&self, id: u32) -> Option<&Record> {
        self.records.iter().find(|r| r.id == id)
    }

    fn record_mut(&mut self, id: u32) -> Option<&mut Record> {
        self.records.iter_mut().find(|r| r.id == id)
    }

    /// Whether a record would join something on the air.
    fn reachable(&self, id: u32) -> bool {
        let Some(r) = self.record(id) else { return false };
        self.air.iter().any(|a| a.ssid == r.ssid && (!a.hidden || r.scan_ssid) && a.psk == r.psk)
    }

    fn select(&mut self, id: u32) {
        for r in &mut self.records {
            r.disabled = r.id != id;
        }
        self.current = None;
        self.polls_since_join = 0;
        let polls = if self.reachable(id) { self.join_polls } else { u32::MAX };
        self.joining = Some((id, polls));
        self.wrong_key = (self.reports_wrong_key && self.wrong_key_for(id)).then_some((id, 1));
    }

    /// Whether a record names a secured network on the air with another key, and nothing it joins.
    fn wrong_key_for(&self, id: u32) -> bool {
        let Some(r) = self.record(id) else { return false };
        !self.reachable(id)
            && self
                .air
                .iter()
                .any(|a| a.ssid == r.ssid && (!a.hidden || r.scan_ssid) && !a.psk.is_empty() && a.psk != r.psk)
    }

    fn push_event(&self, event: String) {
        if let Ok(mut events) = self.events.lock() {
            events.push_back(event);
        }
    }

    /// An event source reading this model's events.
    pub fn events(&self) -> FakeEvents {
        FakeEvents(Arc::clone(&self.events))
    }

    fn status(&mut self) -> String {
        if let Some((id, left)) = self.wrong_key {
            if left == 0 {
                let ssid = self.record(id).map(|r| escape(&r.ssid)).unwrap_or_default();
                self.push_event(format!(
                    "CTRL-EVENT-SSID-TEMP-DISABLED id={id} ssid=\"{ssid}\" auth_failures=1 duration=10 reason=WRONG_KEY"
                ));
                self.wrong_key = None;
            } else {
                self.wrong_key = Some((id, left - 1));
            }
        }
        if let Some((id, left)) = self.joining {
            if left == u32::MAX {
                return "wpa_state=SCANNING\n".into();
            }
            if left > 1 {
                self.joining = Some((id, left - 1));
                return "wpa_state=4WAY_HANDSHAKE\n".into();
            }
            self.joining = None;
            self.current = Some(id);
        }
        let Some(id) = self.current else { return DISCONNECTED.into() };
        self.polls_since_join += 1;
        if self.drops && self.polls_since_join > 1 {
            self.current = None;
            return DISCONNECTED.into();
        }
        let ssid = self.record(id).map(|r| escape(&r.ssid)).unwrap_or_default();
        let mut out =
            format!("bssid=02:00:00:00:00:01\nfreq=2437\nssid={ssid}\nid={id}\nmode=station\nwpa_state=COMPLETED\n");
        if self.dhcp_after.is_some_and(|n| self.polls_since_join > n) {
            out.push_str("ip_address=192.168.1.20\n");
        }
        out
    }

    /// LIST_NETWORKS: the records after id `after`, if given.
    fn list(&self, after: Option<u32>) -> String {
        let mut out = String::from("network id / ssid / bssid / flags\n");
        for r in self.records.iter().filter(|r| after.is_none_or(|a| r.id > a)) {
            let current = if self.current == Some(r.id) { "[CURRENT]" } else { "" };
            let disabled = if r.disabled { "[DISABLED]" } else { "" };
            out.push_str(&format!("{}\t{}\tany\t{current}{disabled}\n", r.id, escape(&r.ssid)));
        }
        out
    }

    /// The fields of the BSS on the air at `index` that `mask` asks for.
    fn bss(&self, index: usize, mask: u32) -> String {
        let Some(a) = self.air.get(index) else { return String::new() };
        let ssid = if a.hidden { String::new() } else { escape(&a.ssid) };
        [
            (0x1, format!("id={index}")),
            (0x2, format!("bssid=02:00:00:00:01:{index:02x}")),
            (0x4, "freq=2437".to_owned()),
            (0x80, format!("level={}", a.level)),
            (0x200, format!("age={}", a.age)),
            (0x800, format!("flags={}", a.flags)),
            (0x1000, format!("ssid={ssid}")),
        ]
        .iter()
        .filter(|(bit, _)| mask & bit != 0)
        .map(|(_, field)| format!("{field}\n"))
        .collect()
    }

    fn set(&mut self, id: u32, field: &str, value: &str) -> bool {
        let Some(r) = self.record_mut(id) else { return false };
        match field {
            "ssid" => match unhex(value) {
                Some(bytes) => r.ssid = bytes,
                None => return false,
            },
            "key_mgmt" => r.key_mgmt = value.into(),
            "psk" if value.len() == 64 && unhex(value).is_some() => r.psk = value.into(),
            "scan_ssid" => r.scan_ssid = value == "1",
            // Unlike DISABLE_NETWORK, it leaves the network joined.
            "disabled" if value == "0" || value == "1" => r.disabled = value == "1",
            _ => return false,
        }
        true
    }
}

impl Control for Fake {
    fn request(&mut self, command: &str) -> Result<String, WpaError> {
        self.log.push(command.to_owned());
        if command == "SCAN" && self.current.is_some() {
            self.scans_while_joined += 1;
        }
        let words: Vec<&str> = command.split(' ').collect();
        let id = |i: usize| words.get(i).and_then(|w| w.parse::<u32>().ok());
        let reply = match words.as_slice() {
            ["PING"] => "PONG\n".into(),
            ["DISCONNECT"] => {
                self.current = None;
                self.joining = None;
                "OK\n".into()
            }
            ["SCAN"] if self.scan_fails => "FAIL\n".into(),
            ["SCAN"] => "OK\n".into(),
            ["BSS", ..] if self.bss_fails => "FAIL\n".into(),
            ["STATUS"] => self.status(),
            ["LIST_NETWORKS"] => self.list(None),
            ["LIST_NETWORKS", last] => self.list(last.strip_prefix("LAST_ID=").and_then(|n| n.parse().ok())),
            ["BSS", "FIRST", m] => self.bss(0, mask(m)),
            ["BSS", next, m] if next.starts_with("NEXT-") => {
                next[5..].parse::<usize>().map(|i| self.bss(i + 1, mask(m))).unwrap_or_default()
            }
            ["BSS", "CURRENT", m] => {
                let ssid = self.current.and_then(|c| self.record(c)).map(|r| r.ssid.clone());
                match ssid.and_then(|s| self.air.iter().position(|a| a.ssid == s)) {
                    Some(index) => self.bss(index, mask(m)),
                    None => String::new(),
                }
            }
            ["ADD_NETWORK"] => {
                let id = self.next_id;
                self.next_id += 1;
                self.records.push(Record {
                    id,
                    ssid: Vec::new(),
                    key_mgmt: "WPA-PSK".into(),
                    psk: String::new(),
                    scan_ssid: false,
                    disabled: true,
                });
                format!("{id}\n")
            }
            ["SET_NETWORK", _, field, value] => match id(1) {
                Some(n) if self.set(n, field, value) => "OK\n".into(),
                _ => "FAIL\n".into(),
            },
            ["SELECT_NETWORK", _] => match id(1) {
                Some(n) if self.record(n).is_some() => {
                    self.select(n);
                    "OK\n".into()
                }
                _ => "FAIL\n".into(),
            },
            ["ENABLE_NETWORK", _] | ["ENABLE_NETWORK", _, "no-connect"] => match id(1).and_then(|n| self.record_mut(n))
            {
                Some(r) => {
                    r.disabled = false;
                    "OK\n".into()
                }
                None => "FAIL\n".into(),
            },
            ["DISABLE_NETWORK", _] => match id(1) {
                Some(n) if self.record(n).is_some() => {
                    if let Some(r) = self.record_mut(n) {
                        r.disabled = true;
                    }
                    if self.current == Some(n) {
                        self.current = None;
                    }
                    "OK\n".into()
                }
                _ => "FAIL\n".into(),
            },
            ["REMOVE_NETWORK", _] => match id(1) {
                Some(n) if self.record(n).is_some() => {
                    self.records.retain(|r| r.id != n);
                    if self.current == Some(n) {
                        self.current = None;
                    }
                    if self.joining.is_some_and(|(j, _)| j == n) {
                        self.joining = None;
                    }
                    "OK\n".into()
                }
                _ => "FAIL\n".into(),
            },
            ["SAVE_CONFIG"] if self.save_fails => "FAIL\n".into(),
            ["SAVE_CONFIG"] => {
                self.file = Some(self.records.clone());
                "OK\n".into()
            }
            _ => "UNKNOWN COMMAND\n".into(),
        };
        Ok(cap(reply))
    }
}

/// BSS's MASK=: hex, all fields for 0 or anything unreadable.
fn mask(word: &str) -> u32 {
    word.strip_prefix("MASK=").and_then(|m| u32::from_str_radix(m, 16).ok()).filter(|&m| m != 0).unwrap_or(u32::MAX)
}

/// A reply cut to whole lines within wpa_supplicant's 4096 bytes.
fn cap(mut reply: String) -> String {
    if reply.len() > REPLY_MAX {
        let end = reply[..REPLY_MAX].rfind('\n').map_or(0, |i| i + 1);
        reply.truncate(end);
    }
    reply
}

/// wpa_supplicant's printf_encode.
pub fn escape(bytes: &[u8]) -> String {
    let mut out = String::new();
    for &b in bytes {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            0x1b => out.push_str("\\e"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            32..=126 => out.push(char::from(b)),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
}

/// The model behind a control socket at `path`: replies to requests, ATTACH,
/// and a scan-results event to attached sockets after each SCAN it takes.
pub fn serve(path: &Path, fake: Fake) -> Arc<Mutex<Fake>> {
    let sock = UnixDatagram::bind(path).unwrap();
    let shared = Arc::new(Mutex::new(fake));
    let model = Arc::clone(&shared);
    thread::spawn(move || {
        let mut attached: Vec<PathBuf> = Vec::new();
        let mut buf = [0; 4096];
        while let Ok((n, from)) = sock.recv_from(&mut buf) {
            let Some(from) = from.as_pathname().map(Path::to_path_buf) else { continue };
            let command = String::from_utf8_lossy(&buf[..n]).into_owned();
            let reply = match command.as_str() {
                "ATTACH" => {
                    attached.push(from.clone());
                    "OK\n".to_owned()
                }
                "DETACH" => {
                    attached.retain(|a| *a != from);
                    "OK\n".to_owned()
                }
                _ => model.lock().unwrap().request(&command).unwrap(),
            };
            let _ = sock.send_to(reply.as_bytes(), &from);
            let mut events: Vec<String> = model.lock().unwrap().events.lock().unwrap().drain(..).collect();
            if command == "SCAN" && reply == "OK\n" {
                events.push("CTRL-EVENT-SCAN-RESULTS ".to_owned());
            }
            for event in events {
                for a in &attached {
                    let _ = sock.send_to(format!("<3>{event}").as_bytes(), a);
                }
            }
        }
    });
    shared
}

/// The model's events, read in the same process.
pub struct FakeEvents(Arc<Mutex<VecDeque<String>>>);

impl EventSource for FakeEvents {
    fn next_event(&mut self, timeout: Duration) -> Result<Option<String>, WpaError> {
        if let Some(event) = self.0.lock().ok().and_then(|mut events| events.pop_front()) {
            return Ok(Some(event));
        }
        thread::sleep(timeout);
        Ok(None)
    }
}

/// An event source with nothing to say.
pub struct NoEvents;

impl EventSource for NoEvents {
    fn next_event(&mut self, timeout: Duration) -> Result<Option<String>, WpaError> {
        thread::sleep(timeout);
        Ok(None)
    }
}
