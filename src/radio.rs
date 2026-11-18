//! Wi-Fi as rc.conf and the kernel have it, for when wpa_supplicant can't be reached.

use crate::cards::rc_value;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Radio {
    /// FreeBSD has attached no Wi-Fi adapter.
    NoAdapter,
    /// No wlan in rc.conf uses wpa_supplicant.
    NotSetUp,
    /// Every wlan that uses wpa_supplicant has NOAUTO, so rc leaves it down.
    Off,
    On,
}

/// Wi-Fi from `sysctl -n net.wlan.devices` and `sysrc -a`'s "name: value" lines.
pub fn parse(adapters: &str, rc: &str) -> Radio {
    if adapters.trim().is_empty() {
        return Radio::NoAdapter;
    }
    let wpa: Vec<Vec<String>> =
        wlans(rc).map(|wlan| words(rc, &format!("ifconfig_{wlan}"))).filter(|w| has(w, "wpa")).collect();
    if wpa.is_empty() {
        Radio::NotSetUp
    } else if wpa.iter().all(|w| has(w, "noauto")) {
        Radio::Off
    } else {
        Radio::On
    }
}

/// The wlans that rc.conf's wlans_ lines name.
fn wlans(rc: &str) -> impl Iterator<Item = &str> {
    rc.lines()
        .filter_map(|line| line.strip_prefix("wlans_")?.split_once(": "))
        .flat_map(|(_, wlans)| wlans.split_whitespace())
}

/// A setting's words, in lower case; none if it isn't set.
fn words(rc: &str, name: &str) -> Vec<String> {
    rc_value(rc, name).map(|value| value.split_whitespace().map(str::to_ascii_lowercase).collect()).unwrap_or_default()
}

fn has(words: &[String], word: &str) -> bool {
    words.iter().any(|w| w == word)
}

/// Wi-Fi as the running system has it.
pub fn read() -> Radio {
    let output = |program: &str, args: &[&str]| {
        Command::new(program)
            .args(args)
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
            .unwrap_or_default()
    };
    parse(&output("/sbin/sysctl", &["-n", "net.wlan.devices"]), &output("/usr/sbin/sysrc", &["-a"]))
}
