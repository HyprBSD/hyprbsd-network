//! What the window shows of Wi-Fi.

use crate::wifi::{self, Network, Saved, Security, Ssid, Status, WifiError};
use crate::worker::Request;
use std::cmp::Reverse;

/// A network in the window's list: in range, saved, or both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub ssid: Ssid,
    /// The strongest signal by this name, in dBm; None when it isn't in range.
    pub signal: Option<i32>,
    /// None when it isn't in range.
    pub security: Option<Security>,
    pub connected: bool,
    /// The ids of its saved records; empty when it isn't saved.
    pub saved: Vec<u32>,
    /// Whether wpa_supplicant joins it by itself.
    pub autoconnect: bool,
}

/// One row per name: the connected network, then those in range strongest first, then saved
/// networks out of range by name.
pub fn rows(networks: &[Network], saved: &[Saved], status: &Status) -> Vec<Row> {
    let mut names: Vec<&Ssid> = networks.iter().map(|n| &n.ssid).collect();
    names.extend(saved.iter().map(|s| &s.ssid));
    let mut rows: Vec<Row> = Vec::new();
    for ssid in names {
        if !rows.iter().any(|r| r.ssid == *ssid) {
            rows.push(row(ssid, networks, saved, status));
        }
    }
    rows.sort_by(|a, b| {
        // Reverse puts None, out of range, after every signal.
        let rank = |r: &Row| (!r.connected, Reverse(r.signal));
        rank(a).cmp(&rank(b)).then_with(|| a.ssid.cmp(&b.ssid))
    });
    rows
}

fn row(ssid: &Ssid, networks: &[Network], saved: &[Saved], status: &Status) -> Row {
    let records: Vec<&Saved> = saved.iter().filter(|s| s.ssid == *ssid).collect();
    Row {
        ssid: ssid.clone(),
        signal: networks.iter().filter(|n| n.ssid == *ssid).map(|n| n.signal).max(),
        security: wifi::security_of(networks, ssid),
        connected: status.joined() && status.ssid.as_ref() == Some(ssid),
        saved: records.iter().map(|s| s.id).collect(),
        autoconnect: records.iter().any(|s| !s.disabled),
    }
}

/// Whether Connect asks for a password: a secured network not saved, or saved but with its
/// password to be typed again.
pub fn needs_password(row: &Row, retype: bool) -> bool {
    row.security == Some(Security::Wpa2) && (row.saved.is_empty() || retype)
}

/// Whether a card has Connect: a network not joined that can be joined here, or a saved one out of
/// range, which may hide its name.
pub fn connectable(row: &Row) -> bool {
    !row.connected && row.security.is_none_or(Security::joinable)
}

/// The hint in the card's password field, if it has one: the password Connect needs, or, once a
/// saved network out of range has failed to join, a new one it may take.
pub fn password_hint(row: &Row, retype: bool) -> Option<&'static str> {
    if needs_password(row, retype) {
        Some("Password")
    } else if takes_new_password(row, retype) {
        Some("New password, if it changed")
    } else {
        None
    }
}

/// Whether a saved network out of range has failed to join, so its card takes a new password.
fn takes_new_password(row: &Row, retype: bool) -> bool {
    row.security.is_none() && retype
}

/// What Connect asks for: the network joined with the password typed, else as saved, else open.
/// A saved network out of range given a new password is joined as hidden, asked for by name.
pub fn connect(row: &Row, password: &str, retype: bool) -> Result<Request, WifiError> {
    let ssid = row.ssid.clone();
    let hidden = takes_new_password(row, retype) && !password.is_empty();
    if needs_password(row, retype) || hidden {
        wifi::check_passphrase(password.as_bytes())?;
        return Ok(Request::Join { ssid, password: Some(password.as_bytes().to_vec()), hidden });
    }
    Ok(match row.saved.last() {
        Some(&id) => Request::Connect { id },
        None => Request::Join { ssid, password: None, hidden: false },
    })
}

/// What Connect asks for on the Other network card: without a password the network is open.
pub fn connect_hidden(name: &str, password: &str) -> Result<Request, WifiError> {
    let ssid = Ssid::new(name.as_bytes())?;
    let password = (!password.is_empty()).then(|| password.as_bytes().to_vec());
    if let Some(password) = &password {
        wifi::check_passphrase(password)?;
    }
    Ok(Request::Join { ssid, password, hidden: true })
}

/// An error as a sentence.
pub fn sentence(e: &WifiError) -> String {
    as_sentence(&e.to_string())
}

/// Text as a sentence: a capital first and a full stop last.
pub fn as_sentence(text: &str) -> String {
    let mut out = capitalised(text);
    if !out.ends_with('.') {
        out.push('.');
    }
    out
}

/// When the networks listed were last scanned, `age` seconds ago.
pub fn scanned(age: u32) -> String {
    match age {
        0..60 => "Last scanned just now.".to_owned(),
        60..120 => "Last scanned a minute ago.".to_owned(),
        120..3600 => format!("Last scanned {} minutes ago.", age / 60),
        3600..7200 => "Last scanned an hour ago.".to_owned(),
        7200..172_800 => format!("Last scanned {} hours ago.", age / 3600),
        _ => format!("Last scanned {} days ago.", age / 86_400),
    }
}

/// The line under a network's name.
pub fn caption(row: &Row, address: Option<&str>) -> String {
    if row.connected {
        return address.map_or_else(|| "Connected".to_owned(), |a| format!("Connected · {a}"));
    }
    let Some(security) = row.security else { return "Saved · not in range".to_owned() };
    if let Some(why) = security.refusal() {
        return capitalised(why);
    }
    let kind = capitalised(security.describe());
    if row.saved.is_empty() { kind } else { format!("{kind} · saved") }
}

/// The text with its first letter a capital.
fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}
