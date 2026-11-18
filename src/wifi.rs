//! Wi-Fi networks: names, security, scan results, saved networks, joining and forgetting.

use crate::wpa::{self, Control, EventSource, WpaError, expect_ok};
use pbkdf2::pbkdf2_hmac;
use sha1::Sha1;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// BSS's fields (wpa_ctrl.h's WPA_BSS_MASK_*).
const MASK_ID: u32 = 1 << 0;
const MASK_LEVEL: u32 = 1 << 7;
const MASK_AGE: u32 = 1 << 9;
const MASK_FLAGS: u32 = 1 << 11;
const MASK_SSID: u32 = 1 << 12;
/// The BSS fields `scan_results` reads.
const BSS_MASK: u32 = MASK_ID | MASK_LEVEL | MASK_AGE | MASK_FLAGS | MASK_SSID;
/// More networks than any scan finds.
const MAX_BSS: usize = 1000;
const LIST_HEADING: &str = "network id / ssid / bssid / flags";

#[derive(Debug, thiserror::Error)]
pub enum WifiError {
    #[error("a Wi-Fi network's name has 1 to 32 bytes")]
    Name,
    #[error("a Wi-Fi password has 8 to 63 bytes")]
    Passphrase,
    #[error("couldn't join \"{0}\": the password is wrong")]
    WrongPassword(String),
    #[error("stopped; \"{0}\" isn't joined")]
    Stopped(String),
    #[error("couldn't join \"{0}\": the password is wrong, or the network is out of reach")]
    NotJoined(String),
    #[error("joined \"{0}\", but wpa_supplicant couldn't save it, so it won't rejoin it after a restart")]
    Unsaved(String),
    #[error("removed \"{0}\", but wpa_supplicant couldn't save that, so it's back after a restart")]
    NotForgotten(String),
    #[error("\"{0}\" isn't a saved network")]
    NotSaved(String),
    #[error("changed \"{0}\", but wpa_supplicant couldn't save that, so it's as it was after a restart")]
    NotKept(String),
    #[error("no saved network has id {0}")]
    NoId(u32),
    #[error(transparent)]
    Wpa(#[from] WpaError),
}

fn bad(command: &str, reply: &str) -> WifiError {
    WifiError::Wpa(wpa::unexpected(command, reply))
}

/// A network's name, as bytes: it needn't be UTF-8.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ssid(Vec<u8>);

impl Ssid {
    /// A name someone typed or picked: 1 to 32 bytes.
    pub fn new(bytes: &[u8]) -> Result<Self, WifiError> {
        if (1..=32).contains(&bytes.len()) { Ok(Ssid(bytes.to_vec())) } else { Err(WifiError::Name) }
    }

    /// A name as wpa_supplicant prints it, with its escapes.
    pub fn from_wpa(text: &str) -> Self {
        let bytes = text.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            let escaped = match (bytes[i], bytes.get(i + 1)) {
                (b'\\', Some(b'\\')) => Some((b'\\', 2)),
                (b'\\', Some(b'"')) => Some((b'"', 2)),
                (b'\\', Some(b'e')) => Some((0x1b, 2)),
                (b'\\', Some(b'n')) => Some((b'\n', 2)),
                (b'\\', Some(b'r')) => Some((b'\r', 2)),
                (b'\\', Some(b't')) => Some((b'\t', 2)),
                (b'\\', Some(b'x')) => bytes
                    .get(i + 2..i + 4)
                    .filter(|h| h.iter().all(u8::is_ascii_hexdigit))
                    .and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok())
                    .map(|b| (b, 4)),
                _ => None,
            };
            let (byte, len) = escaped.unwrap_or((bytes[i], 1));
            out.push(byte);
            i += len;
        }
        Ssid(out)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The name in hex, as wpa_supplicant takes an unquoted name.
    pub fn hex(&self) -> String {
        hex(&self.0)
    }

    /// The name to show: UTF-8 as it is; other bytes, control characters, line separators and
    /// direction overrides as `\xNN`.
    pub fn display(&self) -> String {
        let mut out = String::with_capacity(self.0.len());
        for chunk in self.0.utf8_chunks() {
            for c in chunk.valid().chars() {
                if c.is_control() || matches!(c, '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
                    let mut buf = [0; 4];
                    escape_bytes(&mut out, c.encode_utf8(&mut buf).as_bytes());
                } else {
                    out.push(c);
                }
            }
            escape_bytes(&mut out, chunk.invalid());
        }
        out
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn escape_bytes(out: &mut String, bytes: &[u8]) {
    for b in bytes {
        let _ = write!(out, "\\x{b:02x}");
    }
}

/// What a network asks of those who join it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Security {
    Open,
    /// WPA or WPA2 with a password (WPA-PSK), mixed WPA2/WPA3 included.
    Wpa2,
    /// WPA3 alone (SAE), which FreeBSD's wpa_supplicant can't join.
    Wpa3,
    /// 802.1X: a user name and password, or certificates.
    Enterprise,
    Wep,
    /// Security wpa_supplicant couldn't read, or that offers only PSK-SHA256 or FT-PSK.
    Unknown,
}

impl Security {
    /// The security in a scan result's flags, such as `[WPA2-PSK-CCMP][ESS]`.
    pub fn from_flags(flags: &str) -> Self {
        let (mut psk, mut sae, mut eap, mut rsn, mut wep) = (false, false, false, false, false);
        for entry in flags.split(['[', ']']) {
            if entry == "WEP" {
                wep = true;
            }
            let Some(suites) = ["WPA-", "WPA2-", "RSN-", "OSEN-"].iter().find_map(|p| entry.strip_prefix(p)) else {
                continue;
            };
            rsn = true;
            // Key managements joined by +, the last with the ciphers after it: PSK+SAE-CCMP.
            for suite in suites.split('+') {
                psk |= suite == "PSK" || (suite.starts_with("PSK-") && !suite.starts_with("PSK-SHA256"));
                sae |= suite.starts_with("SAE") || suite.starts_with("FT/SAE");
                eap |= suite.contains("EAP") || suite.contains("FILS");
            }
        }
        match () {
            () if psk => Security::Wpa2,
            () if sae => Security::Wpa3,
            () if eap => Security::Enterprise,
            () if rsn => Security::Unknown,
            () if wep => Security::Wep,
            () => Security::Open,
        }
    }

    /// Why hyprbsd-network can't join it, if it can't.
    pub fn refusal(self) -> Option<&'static str> {
        match self {
            Security::Open | Security::Wpa2 => None,
            Security::Wpa3 => Some("FreeBSD's Wi-Fi can't join WPA3-only networks yet"),
            Security::Enterprise => Some("802.1X (enterprise) networks can't be joined here yet"),
            Security::Wep => Some("WEP networks aren't joined: WEP isn't safe"),
            Security::Unknown => Some("this kind of network can't be joined here"),
        }
    }

    /// Whether hyprbsd-network can join it.
    pub fn joinable(self) -> bool {
        self.refusal().is_none()
    }

    pub fn describe(self) -> &'static str {
        match self {
            Security::Open => "open",
            Security::Wpa2 => "WPA2",
            Security::Wpa3 => "WPA3 only",
            Security::Enterprise => "enterprise (802.1X)",
            Security::Wep => "WEP",
            Security::Unknown => "other security",
        }
    }
}

/// Signal strength in four steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Strength {
    Weak,
    Fair,
    Good,
    Strong,
}

impl Strength {
    /// The step for a signal level in dBm.
    pub fn from_dbm(dbm: i32) -> Self {
        // As a percentage: -100 dBm is 0 and -50 dBm is 100.
        match dbm.saturating_add(100).saturating_mul(2) {
            q if q > 80 => Strength::Strong,
            q if q > 55 => Strength::Good,
            q if q > 30 => Strength::Fair,
            _ => Strength::Weak,
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Strength::Weak => "weak",
            Strength::Fair => "fair",
            Strength::Good => "good",
            Strength::Strong => "strong",
        }
    }
}
/// A network in range: one entry per name and security, at its strongest signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    pub ssid: Ssid,
    pub security: Security,
    /// dBm
    pub signal: i32,
}

/// A network in wpa_supplicant's configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    pub id: u32,
    pub ssid: Ssid,
    pub current: bool,
    /// Not joined automatically.
    pub disabled: bool,
}

/// wpa_supplicant's state for its interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// `wpa_state`: COMPLETED once joined.
    pub state: String,
    pub ssid: Option<Ssid>,
    /// The saved network in use.
    pub id: Option<u32>,
    /// The interface's IPv4 address.
    pub address: Option<String>,
}

impl Status {
    pub fn joined(&self) -> bool {
        self.state == "COMPLETED"
    }
}

/// The key a network is joined with.
#[derive(Debug, Clone, Copy)]
pub enum Key<'a> {
    Open,
    Passphrase(&'a [u8]),
}

/// How long a join waits for the key handshake, then for an address, polling at `poll`, what
/// stops it early, and how often the window scans while nothing is joined.
#[derive(Debug, Clone, Copy)]
pub struct Wait {
    pub handshake: Duration,
    pub address: Duration,
    pub poll: Duration,
    /// How often the window's worker scans again while nothing is joined.
    pub rescan: Duration,
    /// Set (by a signal handler) to stop waiting and undo the join.
    pub stop: Option<&'static AtomicBool>,
}

impl Wait {
    fn stopped(&self) -> bool {
        self.stop.is_some_and(|stop| stop.load(Ordering::Relaxed))
    }
}

impl Default for Wait {
    fn default() -> Self {
        Wait {
            handshake: Duration::from_secs(25),
            address: Duration::from_secs(20),
            poll: Duration::from_millis(500),
            rescan: Duration::from_secs(30),
            stop: None,
        }
    }
}

/// A network joined, and the address it gave, if any yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Joined {
    pub id: u32,
    pub address: Option<String>,
}

/// The pre-shared key for a passphrase: PBKDF2-HMAC-SHA1, 4096 rounds, the name as salt.
pub fn psk(passphrase: &[u8], ssid: &Ssid) -> [u8; 32] {
    let mut key = [0; 32];
    pbkdf2_hmac::<Sha1>(passphrase, ssid.as_bytes(), 4096, &mut key);
    key
}

pub fn check_passphrase(passphrase: &[u8]) -> Result<(), WifiError> {
    if (8..=63).contains(&passphrase.len()) { Ok(()) } else { Err(WifiError::Passphrase) }
}

/// A `key=value` line's value in a reply.
fn field<'a>(reply: &'a str, key: &str) -> Option<&'a str> {
    reply.lines().find_map(|line| line.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| v))
}

/// Asks wpa_supplicant to scan; one already running is as good.
pub fn scan<C: Control>(ctrl: &mut C) -> Result<(), WifiError> {
    let reply = ctrl.request("SCAN")?;
    if reply.trim_end() == "FAIL-BUSY" {
        return Ok(());
    }
    Ok(expect_ok("SCAN", &reply)?)
}

/// One entry of wpa_supplicant's scan results.
struct Bss {
    id: u32,
    level: i32,
    /// Seconds since a scan last saw it.
    age: Option<u32>,
    flags: String,
    ssid: Ssid,
}

/// The scan results: the networks in range, and how many seconds ago a scan last saw any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    pub networks: Vec<Network>,
    pub age: Option<u32>,
}

/// A BSS reply; None for an empty reply, after the last entry or for an entry that has expired.
fn parse_bss(command: &str, reply: &str) -> Result<Option<Bss>, WifiError> {
    if reply.is_empty() {
        return Ok(None);
    }
    let id = field(reply, "id").and_then(|v| v.parse().ok());
    let level = field(reply, "level").and_then(|v| v.parse().ok());
    let (Some(id), Some(level)) = (id, level) else { return Err(bad(command, reply)) };
    Ok(Some(Bss {
        id,
        level,
        age: field(reply, "age").and_then(|v| v.parse().ok()),
        flags: field(reply, "flags").unwrap_or_default().to_owned(),
        ssid: Ssid::from_wpa(field(reply, "ssid").unwrap_or_default()),
    }))
}

/// The networks in range, strongest first, without hidden ones (names empty or all zero bytes).
pub fn networks<C: Control>(ctrl: &mut C) -> Result<Vec<Network>, WifiError> {
    Ok(scan_results(ctrl)?.networks)
}

/// The networks in range as `networks` gives them, and the age of the newest sighting.
pub fn scan_results<C: Control>(ctrl: &mut C) -> Result<Scan, WifiError> {
    let mut found: Vec<Network> = Vec::new();
    let mut age: Option<u32> = None;
    // One entry a request, first to last.
    let mut command = format!("BSS FIRST MASK={BSS_MASK:x}");
    for _ in 0..MAX_BSS {
        let reply = ctrl.request(&command)?;
        let Some(bss) = parse_bss(&command, &reply)? else { break };
        command = format!("BSS NEXT-{} MASK={BSS_MASK:x}", bss.id);
        age = age.into_iter().chain(bss.age).min();
        if bss.ssid.as_bytes().iter().all(|&b| b == 0) {
            continue;
        }
        let security = Security::from_flags(&bss.flags);
        match found.iter_mut().find(|n| n.ssid == bss.ssid && n.security == security) {
            Some(n) => n.signal = n.signal.max(bss.level),
            None => found.push(Network { ssid: bss.ssid, security, signal: bss.level }),
        }
    }
    found.sort_by(|a, b| b.signal.cmp(&a.signal).then_with(|| a.ssid.cmp(&b.ssid)));
    Ok(Scan { networks: found, age })
}

/// Leaves the network joined; wpa_supplicant joins none by itself until asked to.
pub fn disconnect<C: Control>(ctrl: &mut C) -> Result<(), WifiError> {
    Ok(ctrl.ok("DISCONNECT")?)
}

/// One LIST_NETWORKS reply.
fn parse_list(command: &str, reply: &str) -> Result<Vec<Saved>, WifiError> {
    let mut lines = reply.lines();
    if lines.next() != Some(LIST_HEADING) {
        return Err(bad(command, reply));
    }
    lines
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut cols = line.split('\t');
            let (Some(id), Some(ssid), Some(_bssid), Some(flags)) =
                (cols.next(), cols.next(), cols.next(), cols.next())
            else {
                return Err(bad(command, reply));
            };
            let id = id.parse().map_err(|_| bad(command, reply))?;
            Ok(Saved {
                id,
                ssid: Ssid::from_wpa(ssid),
                current: flags.contains("[CURRENT]"),
                disabled: flags.contains("[DISABLED]"),
            })
        })
        .collect()
}

/// A network's security from the scan: secured if any access point by that name is, so an open
/// one never takes the place of the password.
pub fn security_of(networks: &[Network], ssid: &Ssid) -> Option<Security> {
    let rank = |security: Security| match security {
        Security::Wpa2 => 0,
        Security::Open => 2,
        _ => 1,
    };
    networks.iter().filter(|n| n.ssid == *ssid).map(|n| n.security).min_by_key(|&s| rank(s))
}

/// The saved networks.
pub fn saved<C: Control>(ctrl: &mut C) -> Result<Vec<Saved>, WifiError> {
    let mut all: Vec<Saved> = Vec::new();
    // Page by page: LAST_ID asks for the networks after it.
    let mut command = String::from("LIST_NETWORKS");
    loop {
        let reply = ctrl.request(&command)?;
        let seen = all.last().map(|s| s.id);
        let page: Vec<Saved> =
            parse_list(&command, &reply)?.into_iter().filter(|s| seen.is_none_or(|last| s.id > last)).collect();
        let Some(last) = page.last().map(|s| s.id) else { break };
        all.extend(page);
        command = format!("LIST_NETWORKS LAST_ID={last}");
    }
    Ok(all)
}

pub fn status<C: Control>(ctrl: &mut C) -> Result<Status, WifiError> {
    let reply = ctrl.request("STATUS")?;
    let state = field(&reply, "wpa_state").ok_or_else(|| bad("STATUS", &reply))?;
    Ok(Status {
        state: state.to_owned(),
        ssid: field(&reply, "ssid").map(Ssid::from_wpa),
        id: field(&reply, "id").and_then(|v| v.parse().ok()),
        address: field(&reply, "ip_address").map(str::to_owned),
    })
}

/// The signal of the network in use, in dBm, from the last scan.
pub fn signal<C: Control>(ctrl: &mut C) -> Result<Option<i32>, WifiError> {
    let command = format!("BSS CURRENT MASK={MASK_LEVEL:x}");
    let reply = ctrl.request(&command)?;
    if reply.is_empty() {
        return Ok(None);
    }
    field(&reply, "level").and_then(|v| v.parse().ok()).map(Some).ok_or_else(|| bad(&command, &reply))
}

/// Joins a network as a new saved network, and saves it once joined.
///
/// The other saved networks keep whether they're joined automatically. If
/// the network isn't joined, the new record is removed and nothing is saved.
/// A join with a key replaces older records of the name, and if none was joined
/// automatically, neither is the new one; a join without a key keeps them.
/// A wrong key reported on `events`, attached before the call, ends the wait.
pub fn join<C: Control, E: EventSource>(
    ctrl: &mut C,
    events: &mut E,
    ssid: &Ssid,
    key: Key,
    hidden: bool,
    wait: &Wait,
) -> Result<Joined, WifiError> {
    if let Key::Passphrase(passphrase) = key {
        check_passphrase(passphrase)?;
    }
    let before = saved(ctrl)?;
    let reply = ctrl.request("ADD_NETWORK")?;
    let id = reply.trim_end().parse().map_err(|_| bad("ADD_NETWORK", &reply))?;
    match configure(ctrl, id, ssid, key, hidden).and_then(|()| select_and_wait(ctrl, events, id, ssid, wait)) {
        Ok(address) => {
            let replaces = matches!(key, Key::Passphrase(_));
            let (old, others): (Vec<&Saved>, Vec<&Saved>) = before.iter().partition(|s| replaces && s.ssid == *ssid);
            let off = !old.is_empty() && old.iter().all(|s| s.disabled);
            for s in old {
                ctrl.ok(&format!("REMOVE_NETWORK {}", s.id))?;
            }
            if off {
                keep_off(ctrl, id)?;
            }
            enable_again(ctrl, others)?;
            save(ctrl, WifiError::Unsaved(ssid.display()))?;
            Ok(Joined { id, address })
        }
        Err(e) => {
            let _ = ctrl.ok(&format!("REMOVE_NETWORK {id}"));
            restore(ctrl, &before);
            Err(e)
        }
    }
}

fn configure<C: Control>(ctrl: &mut C, id: u32, ssid: &Ssid, key: Key, hidden: bool) -> Result<(), WifiError> {
    ctrl.ok(&format!("SET_NETWORK {id} ssid {}", ssid.hex()))?;
    match key {
        Key::Open => ctrl.ok(&format!("SET_NETWORK {id} key_mgmt NONE"))?,
        Key::Passphrase(passphrase) => {
            ctrl.ok(&format!("SET_NETWORK {id} key_mgmt WPA-PSK"))?;
            ctrl.ok(&format!("SET_NETWORK {id} psk {}", hex(&psk(passphrase, ssid))))?;
        }
    }
    if hidden {
        ctrl.ok(&format!("SET_NETWORK {id} scan_ssid 1"))?;
    }
    Ok(())
}

/// Selects a saved network, which disables the others, and waits until it's
/// joined, then for an address while it's joined. Not joined at the end is an
/// error.
fn select_and_wait<C: Control, E: EventSource>(
    ctrl: &mut C,
    events: &mut E,
    id: u32,
    ssid: &Ssid,
    wait: &Wait,
) -> Result<Option<String>, WifiError> {
    ctrl.ok(&format!("SELECT_NETWORK {id}"))?;
    let ours = |s: &Status| s.joined() && s.id == Some(id);
    let deadline = Instant::now() + wait.handshake;
    let mut now = status(ctrl)?;
    while !ours(&now) {
        if wait.stopped() {
            return Err(WifiError::Stopped(ssid.display()));
        }
        if Instant::now() >= deadline {
            return Err(WifiError::NotJoined(ssid.display()));
        }
        pause(events, id, ssid, wait.poll)?;
        now = status(ctrl)?;
    }
    let deadline = Instant::now() + wait.address;
    while !(ours(&now) && now.address.is_some()) && Instant::now() < deadline {
        if wait.stopped() {
            return Err(WifiError::Stopped(ssid.display()));
        }
        pause(events, id, ssid, wait.poll)?;
        now = status(ctrl)?;
    }
    if wait.stopped() {
        return Err(WifiError::Stopped(ssid.display()));
    }
    if ours(&now) { Ok(now.address) } else { Err(WifiError::NotJoined(ssid.display())) }
}

/// Waits up to `poll` for an event; one saying network `id` has the wrong key is an error.
fn pause<E: EventSource>(events: &mut E, id: u32, ssid: &Ssid, poll: Duration) -> Result<(), WifiError> {
    match events.next_event(poll)? {
        Some(event) if wrong_key(&event) == Some(id) => Err(WifiError::WrongPassword(ssid.display())),
        _ => Ok(()),
    }
}

/// The network a `CTRL-EVENT-SSID-TEMP-DISABLED ... reason=WRONG_KEY` event names.
fn wrong_key(event: &str) -> Option<u32> {
    let rest = event.strip_prefix("CTRL-EVENT-SSID-TEMP-DISABLED id=")?;
    let (_, reason) = rest.rsplit_once(" reason=")?;
    if reason.trim_end() != "WRONG_KEY" {
        return None;
    }
    rest.split_once(' ')?.0.parse().ok()
}

/// Enables the networks that were enabled before, without connecting to one.
fn enable_again<'a, C: Control>(ctrl: &mut C, saved: impl IntoIterator<Item = &'a Saved>) -> Result<(), WifiError> {
    for s in saved.into_iter().filter(|s| !s.disabled) {
        ctrl.ok(&format!("ENABLE_NETWORK {} no-connect", s.id))?;
    }
    Ok(())
}

/// After a failed join: enables the networks that were enabled before, and
/// wpa_supplicant joins one of them.
fn restore<C: Control>(ctrl: &mut C, before: &[Saved]) {
    for s in before.iter().filter(|s| !s.disabled) {
        let _ = ctrl.ok(&format!("ENABLE_NETWORK {}", s.id));
    }
}

/// Joins a saved network.
pub fn connect<C: Control, E: EventSource>(
    ctrl: &mut C,
    events: &mut E,
    id: u32,
    wait: &Wait,
) -> Result<Joined, WifiError> {
    let before = saved(ctrl)?;
    let target = before.iter().find(|s| s.id == id).ok_or(WifiError::NoId(id))?;
    let (ssid, was_disabled) = (target.ssid.clone(), target.disabled);
    match select_and_wait(ctrl, events, id, &ssid, wait) {
        Ok(address) => {
            enable_again(ctrl, before.iter().filter(|s| s.id != id))?;
            if was_disabled {
                keep_off(ctrl, id)?;
            }
            Ok(Joined { id, address })
        }
        Err(e) => {
            if was_disabled {
                let _ = ctrl.ok(&format!("DISABLE_NETWORK {id}"));
            }
            restore(ctrl, &before);
            Err(e)
        }
    }
}

/// Marks a saved network not joined automatically, without leaving it if it's joined
/// (DISABLE_NETWORK would).
fn keep_off<C: Control>(ctrl: &mut C, id: u32) -> Result<(), WifiError> {
    Ok(ctrl.ok(&format!("SET_NETWORK {id} disabled 1"))?)
}

/// The ids of the saved networks with this name; none is an error.
fn saved_ids<C: Control>(ctrl: &mut C, ssid: &Ssid) -> Result<Vec<u32>, WifiError> {
    let ids: Vec<u32> = saved(ctrl)?.into_iter().filter(|s| s.ssid == *ssid).map(|s| s.id).collect();
    if ids.is_empty() { Err(WifiError::NotSaved(ssid.display())) } else { Ok(ids) }
}

/// Removes every saved network with this name, and saves; how many there were.
pub fn forget<C: Control>(ctrl: &mut C, ssid: &Ssid) -> Result<usize, WifiError> {
    let ids = saved_ids(ctrl, ssid)?;
    for id in &ids {
        ctrl.ok(&format!("REMOVE_NETWORK {id}"))?;
    }
    save(ctrl, WifiError::NotForgotten(ssid.display()))?;
    Ok(ids.len())
}

/// Sets whether every saved network with this name is joined automatically, and saves. Off
/// leaves one joined now joined; on doesn't join one now.
pub fn set_autoconnect<C: Control>(ctrl: &mut C, ssid: &Ssid, on: bool) -> Result<(), WifiError> {
    for id in saved_ids(ctrl, ssid)? {
        if on {
            ctrl.ok(&format!("ENABLE_NETWORK {id} no-connect"))?;
        } else {
            keep_off(ctrl, id)?;
        }
    }
    save(ctrl, WifiError::NotKept(ssid.display()))
}

/// SAVE_CONFIG, with `refused` for a refusal.
fn save<C: Control>(ctrl: &mut C, refused: WifiError) -> Result<(), WifiError> {
    match ctrl.ok("SAVE_CONFIG") {
        Err(WpaError::Refused { .. }) => Err(refused),
        saved => Ok(saved?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod psk {
        use super::*;

        #[test]
        fn matches_wpa_passphrase_for_ieee() {
            let key = psk(b"password", &Ssid::new(b"IEEE").unwrap());

            assert_eq!(hex(&key), "f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e");
        }

        #[test]
        fn matches_wpa_passphrase_for_a_longer_pair() {
            let key = psk(b"ThisIsAPassword", &Ssid::new(b"ThisIsASSID").unwrap());

            assert_eq!(hex(&key), "0dc0d6eb90555ed6419756b9a15ec3e3209b63df707dd508d14581f8982721af");
        }

        #[test]
        fn matches_wpa_passphrase_for_utf8() {
            let key = psk("pässwörd ünïcode".as_bytes(), &Ssid::new("Café 5G".as_bytes()).unwrap());

            assert_eq!(hex(&key), "7ab250bbf2d34131913644a55438c9cab28c2eb8f9fceeae0b8548e24d5756d3");
        }
    }

    mod checks {
        use super::*;

        #[test]
        fn a_name_of_32_bytes_is_allowed() {
            assert!(Ssid::new(&[b'n'; 32]).is_ok());
        }

        #[test]
        fn a_name_of_33_bytes_is_refused() {
            assert!(matches!(Ssid::new(&[b'n'; 33]), Err(WifiError::Name)));
        }

        #[test]
        fn a_name_counts_bytes_not_characters() {
            assert!(matches!(Ssid::new("é".repeat(17).as_bytes()), Err(WifiError::Name)));
        }

        #[test]
        fn an_empty_name_is_refused() {
            assert!(matches!(Ssid::new(b""), Err(WifiError::Name)));
        }

        #[test]
        fn a_passphrase_of_7_bytes_is_refused() {
            assert!(matches!(check_passphrase(b"1234567"), Err(WifiError::Passphrase)));
        }

        #[test]
        fn a_passphrase_of_63_bytes_is_allowed() {
            assert!(check_passphrase(&[b'p'; 63]).is_ok());
        }

        #[test]
        fn a_passphrase_counts_bytes_not_characters() {
            assert!(matches!(check_passphrase("é".repeat(32).as_bytes()), Err(WifiError::Passphrase)));
        }
    }

    mod ssid {
        use super::*;

        #[test]
        fn from_wpa_undoes_its_escapes() {
            let ssid = Ssid::from_wpa(r#"a\"b\\c\e\n\r\t\x00\xff"#);

            assert_eq!(ssid.as_bytes(), b"a\"b\\c\x1b\n\r\t\x00\xff");
        }

        #[test]
        fn from_wpa_keeps_a_stray_backslash() {
            let ssid = Ssid::from_wpa(r"a\qb\x4");

            assert_eq!(ssid.as_bytes(), br"a\qb\x4");
        }

        #[test]
        fn hex_is_two_digits_a_byte() {
            assert_eq!(Ssid::new(b"\x00A\xff").unwrap().hex(), "0041ff");
        }

        #[test]
        fn display_keeps_utf8() {
            assert_eq!(Ssid::new("Café ☕".as_bytes()).unwrap().display(), "Café ☕");
        }

        #[test]
        fn display_escapes_other_bytes_and_control_characters() {
            assert_eq!(Ssid::new(b"a\xffb\nc\x7f").unwrap().display(), r"a\xffb\x0ac\x7f");
        }

        #[test]
        fn display_escapes_line_separators_and_direction_overrides() {
            assert_eq!(Ssid::new("a\u{2028}b\u{202e}c".as_bytes()).unwrap().display(), r"a\xe2\x80\xa8b\xe2\x80\xaec");
        }
    }

    mod security {
        use super::*;

        #[test]
        fn no_flags_but_ess_is_open() {
            assert_eq!(Security::from_flags("[ESS]"), Security::Open);
        }

        #[test]
        fn wpa2_psk_is_wpa2() {
            assert_eq!(Security::from_flags("[WPA2-PSK-CCMP][ESS]"), Security::Wpa2);
        }

        #[test]
        fn old_wpa_psk_is_wpa2() {
            assert_eq!(Security::from_flags("[WPA-PSK-TKIP][ESS]"), Security::Wpa2);
        }

        #[test]
        fn mixed_wpa2_and_wpa3_is_wpa2() {
            assert_eq!(Security::from_flags("[WPA2-PSK+SAE-CCMP][SAE-H2E][ESS]"), Security::Wpa2);
        }

        #[test]
        fn sae_alone_is_wpa3() {
            assert_eq!(Security::from_flags("[WPA2-SAE-CCMP][SAE-H2E][ESS]"), Security::Wpa3);
        }

        #[test]
        fn ft_sae_alone_is_wpa3() {
            assert_eq!(Security::from_flags("[WPA2-SAE+FT/SAE-CCMP][ESS]"), Security::Wpa3);
        }

        #[test]
        fn eap_is_enterprise() {
            assert_eq!(Security::from_flags("[WPA2-EAP-CCMP][ESS]"), Security::Enterprise);
        }

        #[test]
        fn wep_is_wep() {
            assert_eq!(Security::from_flags("[WEP][ESS]"), Security::Wep);
        }

        #[test]
        fn an_rsn_entry_wpa_supplicant_couldnt_read_is_unknown() {
            assert_eq!(Security::from_flags("[WPA2-?][ESS]"), Security::Unknown);
        }

        #[test]
        fn psk_sha256_alone_is_unknown() {
            assert_eq!(Security::from_flags("[WPA2-PSK-SHA256-CCMP][ESS]"), Security::Unknown);
        }

        #[test]
        fn ft_psk_alone_is_unknown() {
            assert_eq!(Security::from_flags("[WPA2-FT/PSK-CCMP][ESS]"), Security::Unknown);
        }

        #[test]
        fn psk_beside_psk_sha256_is_wpa2() {
            assert_eq!(Security::from_flags("[WPA2-PSK+PSK-SHA256-CCMP][ESS]"), Security::Wpa2);
        }

        #[test]
        fn psk_with_two_ciphers_is_wpa2() {
            assert_eq!(Security::from_flags("[WPA2-PSK-CCMP+TKIP][ESS]"), Security::Wpa2);
        }

        #[test]
        fn only_open_and_wpa2_are_joinable() {
            let joinable: Vec<_> = [
                Security::Open,
                Security::Wpa2,
                Security::Wpa3,
                Security::Enterprise,
                Security::Wep,
                Security::Unknown,
            ]
            .into_iter()
            .filter(|s| s.joinable())
            .collect();

            assert_eq!(joinable, [Security::Open, Security::Wpa2]);
        }
    }

    mod wrong_key {
        use super::*;

        #[test]
        fn reads_the_network_of_a_wrong_key_event() {
            let event = "CTRL-EVENT-SSID-TEMP-DISABLED id=1 ssid=\"turn the bathroom light off\" auth_failures=1 duration=10 reason=WRONG_KEY";

            assert_eq!(wrong_key(event), Some(1));
        }

        #[test]
        fn another_reason_isnt_a_wrong_key() {
            let event =
                "CTRL-EVENT-SSID-TEMP-DISABLED id=1 ssid=\"Home\" auth_failures=4 duration=30 reason=CONN_FAILED";

            assert_eq!(wrong_key(event), None);
        }

        #[test]
        fn a_name_with_the_reason_in_it_doesnt_count() {
            let event = r#"CTRL-EVENT-SSID-TEMP-DISABLED id=2 ssid="x reason=WRONG_KEY \" y" auth_failures=1 duration=10 reason=CONN_FAILED"#;

            assert_eq!(wrong_key(event), None);
        }
    }

    mod strength {
        use super::*;

        #[test]
        fn steps_follow_the_dbm_level() {
            let steps: Vec<_> = [-45, -59, -61, -72, -73, -84, -86, -100].into_iter().map(Strength::from_dbm).collect();

            assert_eq!(
                steps,
                [
                    Strength::Strong,
                    Strength::Strong,
                    Strength::Good,
                    Strength::Good,
                    Strength::Fair,
                    Strength::Fair,
                    Strength::Weak,
                    Strength::Weak
                ]
            );
        }
    }
}
