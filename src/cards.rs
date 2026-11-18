//! The network cards the window's Interfaces page lists, their settings in rc.conf, and the
//! checks on a card's Manual settings.

use crate::iface::{self, IfaceError, Interface, Route};
use std::fs;
use std::net::Ipv4Addr;
use std::process::Command;

/// What kind of card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Wired,
    Wifi,
    /// A USB Ethernet adapter or a phone sharing its connection (FreeBSD names them ue<N>).
    Usb,
    /// A lagg(4): cards joined as one, for failover or more bandwidth.
    Lagg,
}

/// Where a card stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Up, with a link.
    Connected,
    /// Up, without a link: no cable, or Wi-Fi not joined.
    NoLink,
    /// Down.
    Off,
    /// A Wi-Fi adapter with no wlan.
    NotSetUp,
    /// A PCI network card FreeBSD has no driver for; its vendor and device.
    NoDriver(String),
}

/// How rc.conf gives a card its address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Addressing {
    Automatic,
    /// A fixed address, and its prefix length if rc.conf gives one.
    Manual {
        address: Ipv4Addr,
        prefix: Option<u32>,
    },
    /// Neither DHCP nor an address.
    Unset,
    /// Settings the page doesn't change: words besides DHCP, WPA, NOAUTO, one address, `up`,
    /// `mtu` and `accept_rtadv`, or DHCP with an address.
    ByHand,
}

/// A card's `ifconfig_` setting in rc.conf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub addressing: Addressing,
    /// NOAUTO: rc leaves the card down.
    pub off: bool,
    /// The card's own value, or else `ifconfig_DEFAULT`'s.
    pub value: String,
}

/// A card the Interfaces page lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub name: String,
    pub kind: Kind,
    pub state: State,
    /// IPv4 addresses and their prefix lengths.
    pub inet: Vec<(Ipv4Addr, u32)>,
    pub settings: Settings,
    /// The card's DNS servers: from its `/etc/start_if` if hyprbsd-network wrote it, else what
    /// DHCP gave it (resolvconf's file for the card).
    pub dns: Vec<Ipv4Addr>,
    /// The bridge or lagg the card is part of.
    pub part_of: Option<String>,
    /// A lagg's ports.
    pub members: Vec<String>,
}

/// The cards, and the computer's gateway.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cards {
    pub cards: Vec<Card>,
    /// rc.conf's `defaultrouter`.
    pub defaultrouter: Option<Ipv4Addr>,
    /// The default route.
    pub route: Option<Route>,
}

/// The value of `name` in `sysrc -a`'s "name: value" lines.
pub(crate) fn rc_value<'a>(rc: &'a str, name: &str) -> Option<&'a str> {
    rc.lines().find_map(|line| line.strip_prefix(name)?.strip_prefix(": "))
}

/// The rc.conf setting for a card's addressing, as rc's `get_if_var` names it.
pub fn ifconfig_var(card: &str) -> String {
    format!("ifconfig_{}", card.replace(['.', '-', '/', '+'], "_"))
}

/// A card's settings from `sysrc -a`'s lines: its own `ifconfig_` value, or else `ifconfig_DEFAULT`'s.
pub fn settings(rc: &str, card: &str) -> Settings {
    let value = rc_value(rc, &ifconfig_var(card)).or_else(|| rc_value(rc, "ifconfig_DEFAULT")).unwrap_or("");
    parse_settings(value)
}

/// An `ifconfig_` value read as rc and the root helper read it. Its address follows `inet`, or is
/// a word that looks like one (ifconfig takes a word that isn't a command as the address). Any
/// word but DHCP, WPA, NOAUTO, the address with `netmask` or `broadcast`, `up`, `mtu <n>`,
/// `accept_rtadv`, `laggproto <proto>` and `laggport <port>` makes it one the page doesn't change.
pub fn parse_settings(value: &str) -> Settings {
    let mut dhcp = false;
    let mut off = false;
    let mut by_hand = false;
    let mut manual = None;
    let mut words = value.split_whitespace().peekable();
    while let Some(word) = words.next() {
        match word.to_ascii_lowercase().as_str() {
            "dhcp" | "syncdhcp" | "nosyncdhcp" => dhcp = true,
            "noauto" => off = true,
            "wpa" => {}
            _ => {
                let found = match word {
                    "up" | "accept_rtadv" | "-accept_rtadv" => continue,
                    "laggproto" | "laggport" if words.next().is_some() => continue,
                    "mtu" if words.next_if(|n| n.bytes().all(|b| b.is_ascii_digit())).is_some() => continue,
                    "inet" => words.next().and_then(|w| address(w, &mut words)),
                    _ if word.starts_with(|c: char| c.is_ascii_digit()) && word.contains('.') => {
                        address(word, &mut words)
                    }
                    _ => None,
                };
                match found {
                    Some(found) if manual.is_none() => manual = Some(found),
                    _ => by_hand = true,
                }
            }
        }
    }
    let addressing = match (by_hand, manual, dhcp) {
        (true, ..) | (false, Some(_), true) => Addressing::ByHand,
        (false, Some((address, prefix)), false) => Addressing::Manual { address, prefix },
        (false, None, true) => Addressing::Automatic,
        (false, None, false) => Addressing::Unset,
    };
    Settings { addressing, off, value: value.to_owned() }
}

/// A lagg's `laggproto` and its `laggport`s, in order, from its `ifconfig_` value.
pub fn lagg_words(value: &str) -> (Option<&str>, Vec<&str>) {
    let (mut proto, mut ports) = (None, Vec::new());
    let mut words = value.split_whitespace();
    while let Some(word) = words.next() {
        match word {
            "laggproto" => proto = words.next(),
            "laggport" => ports.extend(words.next()),
            _ => {}
        }
    }
    (proto, ports)
}

/// An address, "a.b.c.d/len", or "a.b.c.d" with `netmask` and `broadcast` in the words after it.
fn address<'a>(
    word: &str,
    words: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
) -> Option<(Ipv4Addr, Option<u32>)> {
    let (address, mut prefix) = match word.split_once('/') {
        Some((address, len)) => (address, Some(len.parse().ok().filter(|l| (1..=32).contains(l))?)),
        None => (word, None),
    };
    let address = address.parse().ok()?;
    while let Some(&key) = words.peek() {
        match key {
            "netmask" => {
                words.next();
                prefix = Some(mask_len(words.next()?)?);
            }
            "broadcast" => {
                words.next();
                words.next()?;
            }
            _ => break,
        }
    }
    Some((address, prefix))
}

/// The prefix length of a netmask, dotted or 0x hex, if its bits are contiguous.
fn mask_len(mask: &str) -> Option<u32> {
    let bits = match mask.strip_prefix("0x") {
        Some(hex) if hex.len() == 8 => u32::from_str_radix(hex, 16).ok()?,
        Some(_) => return None,
        None => u32::from(mask.parse::<Ipv4Addr>().ok()?),
    };
    let len = bits.leading_ones();
    (len > 0 && bits == prefix_mask(len)).then_some(len)
}

fn prefix_mask(len: u32) -> u32 {
    u32::MAX.checked_shl(32 - len).unwrap_or(0)
}

/// Whether `address` is on the network of `on` with prefix length `len`.
fn on_network(address: Ipv4Addr, on: Ipv4Addr, len: u32) -> bool {
    let mask = prefix_mask(len);
    u32::from(address) & mask == u32::from(on) & mask
}

/// The DNS servers in a card's `/etc/start_if`, if hyprbsd-network wrote it.
pub fn parse_hook(text: &str, card: &str) -> Vec<Ipv4Addr> {
    let mut lines = text.lines();
    if lines.next() != Some(hook_mark(card).as_str()) {
        return Vec::new();
    }
    lines
        .next()
        .and_then(|line| line.strip_prefix(r"printf 'nameserver %s\n' "))
        .and_then(|line| line.split_once(" |"))
        .map(|(servers, _)| servers.split(' ').filter_map(|s| s.parse().ok()).collect())
        .unwrap_or_default()
}

/// The IPv4 servers in resolvconf's file for a card (`nameserver` lines).
pub fn parse_resolv(text: &str) -> Vec<Ipv4Addr> {
    text.lines().filter_map(|line| line.strip_prefix("nameserver ")?.trim().parse().ok()).collect()
}

/// The first line of the hooks the root helper writes for a card.
fn hook_mark(card: &str) -> String {
    format!("# hyprbsd-network: {card}'s DNS servers, through resolvconf(8).")
}

/// The Wi-Fi adapters in `sysctl net.wlan`'s lines that no wlan is made from.
pub fn adapters_without_wlan(sysctl: &str) -> Vec<String> {
    let parents: Vec<&str> = sysctl
        .lines()
        .filter_map(|line| line.strip_prefix("net.wlan.")?.split_once(".%parent: ").map(|(_, parent)| parent))
        .collect();
    sysctl
        .lines()
        .find_map(|line| line.strip_prefix("net.wlan.devices:"))
        .map(|devices| devices.split_whitespace().filter(|d| !parents.contains(d)).map(str::to_owned).collect())
        .unwrap_or_default()
}

/// The PCI network cards in `pciconf -lv`'s output that no driver has attached: name, kind,
/// vendor and device.
pub fn without_driver(pciconf: &str) -> Vec<(String, Kind, String)> {
    let mut found = Vec::new();
    let mut lines = pciconf.lines().peekable();
    while let Some(line) = lines.next() {
        let Some((name, rest)) = line.split_once('@') else { continue };
        let Some(class) = rest.split_whitespace().find_map(|w| w.strip_prefix("class=0x02")) else { continue };
        if !name.starts_with("none") {
            continue;
        }
        let mut vendor = None;
        let mut device = None;
        while let Some(detail) = lines.next_if(|l| l.starts_with(char::is_whitespace)) {
            let Some((key, value)) = detail.split_once('=') else { continue };
            let value = value.trim().trim_matches('\'');
            match key.trim() {
                "vendor" => vendor = Some(value),
                "device" => device = Some(value),
                _ => {}
            }
        }
        let kind = if class.starts_with("80") { Kind::Wifi } else { Kind::Wired };
        let model = [vendor, device].into_iter().flatten().collect::<Vec<_>>().join(" ");
        found.push((name.to_owned(), kind, if model.is_empty() { "Network card".to_owned() } else { model }));
    }
    found
}

/// What the Interfaces page lists: from ifconfig, every wlan, every lagg and every interface with
/// an Ethernet address that isn't cloned (in a group named after one of `cloners`); then the
/// Wi-Fi adapters without a wlan (`sysctl net.wlan`), as the wlan rc.conf gives them, off; then
/// the PCI network cards without a driver (`pciconf -lv`). Settings come from `sysrc -a`'s lines,
/// DNS servers from `hook`.
pub fn list(
    interfaces: &[Interface],
    cloners: &str,
    sysctl: &str,
    pciconf: &str,
    rc: &str,
    hook: impl Fn(&str) -> Vec<Ipv4Addr>,
) -> Vec<Card> {
    let cloners: Vec<&str> = cloners.split_whitespace().filter(|c| *c != "wlan").collect();
    let mut cards: Vec<Card> = interfaces
        .iter()
        .filter(|i| i.wifi || lagg(i) || (i.ether && !i.groups.iter().any(|g| cloners.contains(&g.as_str()))))
        .map(|i| Card {
            name: i.name.clone(),
            kind: kind(i),
            state: state(i),
            inet: i.inet.clone(),
            settings: settings(rc, &i.name),
            dns: hook(&i.name),
            part_of: interfaces.iter().find(|p| p.members.contains(&i.name)).map(|p| p.name.clone()),
            members: if lagg(i) { i.members.clone() } else { Vec::new() },
        })
        .collect();
    let unset = Settings { addressing: Addressing::Unset, off: false, value: String::new() };
    // An adapter whose wlan rc.conf names isn't there: Wi-Fi turned off (NOAUTO, the wlan
    // destroyed); with none named, Wi-Fi isn't set up.
    cards.extend(adapters_without_wlan(sysctl).into_iter().map(|adapter| {
        let wlan = rc_value(rc, &format!("wlans_{adapter}")).and_then(|w| w.split_whitespace().next());
        let (name, state, settings) = match wlan {
            Some(wlan) => (wlan.to_owned(), State::Off, settings(rc, wlan)),
            None => (adapter, State::NotSetUp, unset.clone()),
        };
        Card {
            name,
            kind: Kind::Wifi,
            state,
            inet: Vec::new(),
            settings,
            dns: Vec::new(),
            part_of: None,
            members: Vec::new(),
        }
    }));
    cards.extend(without_driver(pciconf).into_iter().map(|(name, kind, model)| Card {
        name,
        kind,
        state: State::NoDriver(model),
        inet: Vec::new(),
        settings: unset.clone(),
        dns: Vec::new(),
        part_of: None,
        members: Vec::new(),
    }));
    cards
}

fn kind(interface: &Interface) -> Kind {
    let usb = interface.name.strip_prefix("ue").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    if interface.wifi {
        Kind::Wifi
    } else if lagg(interface) {
        Kind::Lagg
    } else if usb {
        Kind::Usb
    } else {
        Kind::Wired
    }
}

/// Whether an interface is a lagg: if_clone puts each in the group lagg, even renamed.
fn lagg(interface: &Interface) -> bool {
    interface.groups.iter().any(|g| g == "lagg")
}

fn state(interface: &Interface) -> State {
    match interface.status.as_deref() {
        _ if !interface.up => State::Off,
        None | Some("active" | "associated") => State::Connected,
        Some(_) => State::NoLink,
    }
}

impl Card {
    /// Whether the card has an interface: a driver, and for Wi-Fi a wlan, which turning Wi-Fi off
    /// destroys.
    pub fn has_interface(&self) -> bool {
        matches!(self.state, State::Connected | State::NoLink | State::Off)
            && !(self.kind == Kind::Wifi && self.state == State::Off)
    }
}

impl Cards {
    fn card(&self, name: &str) -> Option<&Card> {
        self.cards.iter().find(|c| c.name == name)
    }

    /// The cards lagg `lagg` can have as ports: its own, and the wired, USB and Wi-Fi cards with
    /// an interface that are in no bridge or lagg and not set up by hand.
    pub fn ports_for<'a>(&'a self, lagg: &'a str) -> impl Iterator<Item = &'a Card> {
        self.cards.iter().filter(move |c| {
            let free = c.part_of.is_none() && c.settings.addressing != Addressing::ByHand;
            matches!(c.kind, Kind::Wired | Kind::Usb | Kind::Wifi)
                && c.has_interface()
                && (free || c.part_of.as_deref() == Some(lagg))
        })
    }

    /// The first lagg name, lagg0, lagg1 ..., that no card has.
    pub fn free_lagg(&self) -> String {
        (0..=self.cards.len())
            .map(|n| format!("lagg{n}"))
            .find(|name| self.card(name).is_none())
            .expect("more lagg names than cards")
    }

    /// Whether `card` is Manual on a network that holds rc.conf's `defaultrouter`.
    fn holds_gateway(&self, card: &Card) -> bool {
        match (self.defaultrouter, card.settings.addressing) {
            (Some(router), Addressing::Manual { address, prefix: Some(len) }) => on_network(router, address, len),
            _ => false,
        }
    }

    /// The gateway a card's page shows: on a Manual card, `defaultrouter` if it's on the
    /// card's network; on an Automatic card, the default route's gateway if it goes through it.
    pub fn gateway(&self, name: &str) -> Option<Ipv4Addr> {
        let card = self.card(name)?;
        match card.settings.addressing {
            Addressing::Manual { .. } => self.defaultrouter.filter(|_| self.holds_gateway(card)),
            Addressing::Automatic => self.route.as_ref().filter(|r| r.iface == name).and_then(|r| r.gateway),
            _ => None,
        }
    }

    /// What applying a gateway on card `name` takes from another card, or from rc.conf, in words.
    pub fn takes_over(&self, name: &str) -> Option<String> {
        if let Some(router) = self.defaultrouter
            && !self.card(name).is_some_and(|c| self.holds_gateway(c))
        {
            return Some(match self.cards.iter().find(|c| self.holds_gateway(c)) {
                Some(holder) => format!("{} has the gateway {router}. Applying moves it to {name}.", holder.name),
                None => format!("The computer's gateway is {router}. Applying replaces it with {name}'s."),
            });
        }
        let route = self.route.as_ref().filter(|r| r.iface != name)?;
        let by_dhcp = self.card(&route.iface).is_some_and(|c| c.settings.addressing == Addressing::Automatic);
        by_dhcp.then(|| format!("{} gets a gateway by DHCP; {name}'s takes over.", route.iface))
    }
}

/// A Manual card's settings, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manual {
    pub address: Ipv4Addr,
    pub prefix: u32,
    pub gateway: Option<Ipv4Addr>,
    pub dns: Vec<Ipv4Addr>,
}

impl Manual {
    /// Checks what's typed on a card's page, as the root helper checks it again: the address,
    /// the prefix length (or a netmask), an optional gateway, and up to three DNS servers
    /// separated by spaces or commas.
    pub fn check(address: &str, prefix: &str, gateway: &str, dns: &str) -> Result<Manual, String> {
        let address = ip(address.trim(), "address")?;
        let prefix = prefix.trim();
        let prefix = prefix
            .strip_prefix('/')
            .unwrap_or(prefix)
            .parse::<u32>()
            .ok()
            .filter(|l| (1..=32).contains(l))
            .or_else(|| mask_len(prefix))
            .ok_or("The prefix length must be 1 to 32, or a netmask like 255.255.255.0.")?;
        if !host(address) {
            return Err(format!("{address} can't be a computer's address."));
        }
        if let Some(edge) = edge(address, prefix) {
            return Err(format!("{address} is the {edge} address of its /{prefix} network."));
        }
        let gateway = match gateway.trim() {
            "" => None,
            typed => Some(check_gateway(ip(typed, "gateway")?, address, prefix)?),
        };
        let dns = dns
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .map(|s| ip(s, "DNS server"))
            .collect::<Result<Vec<_>, _>>()?;
        if dns.len() > 3 {
            return Err("At most three DNS servers.".to_owned());
        }
        if let Some(bad) = dns.iter().find(|d| d.octets()[0] == 0 || d.octets()[0] >= 224) {
            return Err(format!("{bad} can't be a DNS server."));
        }
        Ok(Manual { address, prefix, gateway, dns })
    }

    /// The root helper's arguments for these settings, after the card's name.
    pub fn args(&self) -> Vec<String> {
        let mut args = vec!["manual".to_owned()];
        args.push(format!("{}/{}", self.address, self.prefix));
        args.push(self.gateway.map_or_else(|| "-".to_owned(), |g| g.to_string()));
        args.extend(self.dns.iter().map(Ipv4Addr::to_string));
        args
    }
}

fn ip(text: &str, what: &str) -> Result<Ipv4Addr, String> {
    text.parse().map_err(|_| format!("\"{text}\" isn't an IPv4 address for the {what} (like 192.168.1.5)."))
}

/// Whether an address can be a host's: not 0.x, 127.x, or multicast and above.
fn host(address: Ipv4Addr) -> bool {
    let first = address.octets()[0];
    first != 0 && first != 127 && first < 224
}

/// "network" or "broadcast" if the address is that address of its network.
fn edge(address: Ipv4Addr, len: u32) -> Option<&'static str> {
    if len > 30 {
        return None;
    }
    let (bits, mask) = (u32::from(address), prefix_mask(len));
    if bits == bits & mask {
        Some("network")
    } else if bits == bits | !mask {
        Some("broadcast")
    } else {
        None
    }
}

fn check_gateway(gateway: Ipv4Addr, address: Ipv4Addr, len: u32) -> Result<Ipv4Addr, String> {
    if !host(gateway) {
        return Err(format!("{gateway} can't be a gateway."));
    }
    if len == 32 {
        return Err("A /32 address has no gateway on its network.".to_owned());
    }
    if !on_network(gateway, address, len) {
        return Err(format!("The gateway {gateway} isn't on {address}/{len}'s network."));
    }
    if gateway == address {
        return Err("The gateway can't be the card's own address.".to_owned());
    }
    if let Some(edge) = edge(gateway, len) {
        return Err(format!("{gateway} is the {edge} address of {address}/{len}'s network."));
    }
    Ok(gateway)
}

/// The cards as the running system has them.
pub fn read() -> Result<Cards, IfaceError> {
    let output = |program: &str, args: &[&str]| {
        Command::new(program)
            .args(args)
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
            .unwrap_or_default()
    };
    let interfaces = iface::interfaces()?;
    let rc = output("/usr/sbin/sysrc", &["-a"]);
    let cards = list(
        &interfaces,
        &output("/sbin/ifconfig", &["-C"]),
        &output("/sbin/sysctl", &["net.wlan"]),
        &output("/usr/sbin/pciconf", &["-lv"]),
        &rc,
        |name| {
            let hook = fs::read_to_string(format!("/etc/start_if.{name}")).map(|t| parse_hook(&t, name));
            match hook.unwrap_or_default() {
                dns if dns.is_empty() => fs::read_to_string(format!("/var/run/resolvconf/interfaces/{name}"))
                    .map(|t| parse_resolv(&t))
                    .unwrap_or_default(),
                dns => dns,
            }
        },
    );
    let defaultrouter = rc_value(&rc, "defaultrouter").and_then(|r| r.parse().ok());
    Ok(Cards { cards, defaultrouter, route: iface::route()? })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iface::parse_ifconfig;

    const IFCONFIG: &str = "\
em0: flags=1008843<UP,BROADCAST,RUNNING,SIMPLEX,MULTICAST,LOWER_UP> metric 0 mtu 1500
\tether 02:00:00:00:00:10
\tinet 192.168.1.5 netmask 0xffffff00 broadcast 192.168.1.255
\tmedia: Ethernet autoselect (1000baseT <full-duplex>)
\tstatus: active
re0: flags=8802<BROADCAST,SIMPLEX,MULTICAST> metric 0 mtu 1500
\tether 02:00:00:00:00:11
\tmedia: Ethernet autoselect
\tstatus: no carrier
ue0: flags=8843<UP,BROADCAST,RUNNING,SIMPLEX,MULTICAST> metric 0 mtu 1500
\tether 02:00:00:00:00:12
\tmedia: Ethernet autoselect
\tstatus: no carrier
lo0: flags=1008049<UP,LOOPBACK,RUNNING,MULTICAST,LOWER_UP> metric 0 mtu 16384
\tinet 127.0.0.1 netmask 0xff000000
\tgroups: lo
wlan0: flags=8843<UP,BROADCAST,RUNNING,SIMPLEX,MULTICAST> metric 0 mtu 1500
\tether 02:00:00:00:00:13
\tinet 192.168.1.20 netmask 0xffffff00 broadcast 192.168.1.255
\tgroups: wlan
\tstatus: associated
vm-public: flags=1008843<UP,BROADCAST,RUNNING,SIMPLEX,MULTICAST,LOWER_UP> metric 0 mtu 1500
\tether 8a:8d:fa:9c:d1:87
\tmember: tap0 flags=143<LEARNING,DISCOVER,AUTOEDGE,AUTOPTP>
\tmember: em1 flags=143<LEARNING,DISCOVER,AUTOEDGE,AUTOPTP>
\tgroups: bridge vm-switch viid-4c918@
em1: flags=8843<UP,BROADCAST,RUNNING,SIMPLEX,MULTICAST> metric 0 mtu 1500
\tether 02:00:00:00:00:14
\tstatus: active
tap0: flags=8943<UP,BROADCAST,RUNNING,PROMISC,SIMPLEX,MULTICAST> metric 0 mtu 1500
\tether 58:9c:fc:00:00:01
\tgroups: tap vm-port
\tmedia: Ethernet 1000baseT <full-duplex>
\tstatus: active
usbus0: flags=0<> metric 0 mtu 65536
\tgroups: usbus
lagg0: flags=1008843<UP,BROADCAST,RUNNING,SIMPLEX,MULTICAST,LOWER_UP> metric 0 mtu 1500
\tether 02:00:00:00:00:11
\tinet 192.168.1.30 netmask 0xffffff00 broadcast 192.168.1.255
\tlaggproto failover lagghash l2,l3,l4
\tlaggport: re0 flags=1<MASTER>
\tlaggport: ue0 flags=0<>
\tgroups: lagg
\tmedia: Ethernet autoselect
\tstatus: no carrier
";
    const CLONERS: &str = "bridge vlan vmnet tap tun lo gif usbus wlan lagg";
    const RC: &str = "\
ifconfig_DEFAULT: DHCP
ifconfig_em0: inet 192.168.1.5/24
ifconfig_em1: up
defaultrouter: 192.168.1.1
";
    const PCICONF: &str = "\
re0@pci0:1:0:0:\tclass=0x020000 rev=0x1b hdr=0x00 vendor=0x10ec device=0x8168 subvendor=0x10ec subdevice=0x0123
    vendor     = 'Realtek Semiconductor Co., Ltd.'
    device     = 'RTL8111/8168/8211/8411 PCI Express Gigabit Ethernet Controller'
    class      = network
    subclass   = ethernet
none2@pci0:0:22:0:\tclass=0x078000 rev=0x00 hdr=0x00 vendor=0x8086 device=0x54e0 subvendor=0x0000 subdevice=0x0000
    vendor     = 'Intel Corporation'
    class      = simple comms
none3@pci0:5:0:0:\tclass=0x020000 rev=0x05 hdr=0x00 vendor=0x10ec device=0x8125 subvendor=0x10ec subdevice=0x8125
    vendor     = 'Realtek Semiconductor Co., Ltd.'
    device     = 'RTL8125 2.5GbE Controller'
    class      = network
    subclass   = ethernet
none4@pci0:6:0:0:\tclass=0x028000 rev=0x00 hdr=0x00 vendor=0x14c3 device=0x0616 subvendor=0x14c3 subdevice=0x0616
    vendor     = 'MEDIATEK Corp.'
    class      = network
";
    const SYSCTL: &str = "\
net.wlan.devices: iwlwifi0 rtw880
net.wlan.mesh.maxholding: 2
net.wlan.0.%parent: iwlwifi0
";

    fn listed() -> Vec<Card> {
        let dns = |name: &str| if name == "em0" { vec![Ipv4Addr::new(192, 0, 2, 53)] } else { Vec::new() };
        list(&parse_ifconfig(IFCONFIG), CLONERS, SYSCTL, PCICONF, RC, dns)
    }

    fn card(name: &str) -> Card {
        listed().into_iter().find(|c| c.name == name).unwrap()
    }

    fn addressing(value: &str) -> Addressing {
        parse_settings(value).addressing
    }

    fn manual(a: [u8; 4], prefix: Option<u32>) -> Addressing {
        Addressing::Manual { address: Ipv4Addr::from(a), prefix }
    }

    #[test]
    fn lists_wired_wifi_and_usb_cards_and_laggs_then_adapters_and_cards_without_a_driver() {
        let names: Vec<String> = listed().into_iter().map(|c| c.name).collect();

        assert_eq!(names, ["em0", "re0", "ue0", "wlan0", "em1", "lagg0", "rtw880", "none3", "none4"]);
    }

    #[test]
    fn a_lagg_is_a_lagg() {
        assert_eq!(card("lagg0").kind, Kind::Lagg);
    }

    #[test]
    fn a_lagg_has_its_ports() {
        assert_eq!(card("lagg0").members, ["re0", "ue0"]);
    }

    #[test]
    fn a_lagg_has_its_address() {
        assert_eq!(card("lagg0").inet, [(Ipv4Addr::new(192, 168, 1, 30), 24)]);
    }

    #[test]
    fn a_lagg_port_says_which_lagg() {
        assert_eq!(card("re0").part_of.as_deref(), Some("lagg0"));
    }

    #[test]
    fn a_ue_interface_is_usb() {
        assert_eq!(card("ue0").kind, Kind::Usb);
    }

    #[test]
    fn a_wlan_is_wifi() {
        assert_eq!(card("wlan0").kind, Kind::Wifi);
    }

    #[test]
    fn a_card_that_isnt_up_is_off() {
        assert_eq!(card("re0").state, State::Off);
    }

    #[test]
    fn an_up_card_without_carrier_has_no_link() {
        assert_eq!(card("ue0").state, State::NoLink);
    }

    #[test]
    fn an_associated_wlan_is_connected() {
        assert_eq!(card("wlan0").state, State::Connected);
    }

    #[test]
    fn a_bridge_member_says_which_bridge() {
        assert_eq!(card("em1").part_of.as_deref(), Some("vm-public"));
    }

    #[test]
    fn a_card_reads_its_own_line() {
        assert_eq!(card("em0").settings.addressing, manual([192, 168, 1, 5], Some(24)));
    }

    #[test]
    fn a_card_without_a_line_reads_ifconfig_default() {
        assert_eq!(card("re0").settings.addressing, Addressing::Automatic);
    }

    #[test]
    fn a_card_has_the_dns_servers_of_its_hook() {
        assert_eq!(card("em0").dns, [Ipv4Addr::new(192, 0, 2, 53)]);
    }

    #[test]
    fn an_adapter_without_a_wlan_isnt_set_up() {
        assert_eq!(card("rtw880").state, State::NotSetUp);
    }

    #[test]
    fn wifi_turned_off_is_its_wlan_off_not_an_adapter_to_set_up() {
        let rc = format!("{RC}wlans_rtw880: wlan1\nifconfig_wlan1: WPA DHCP NOAUTO\n");
        let cards = list(&parse_ifconfig(IFCONFIG), CLONERS, SYSCTL, PCICONF, &rc, |_| Vec::new());

        let wifi = cards.iter().find(|c| c.name == "wlan1").map(|c| (c.kind, c.state.clone(), c.settings.off));
        assert_eq!(wifi, Some((Kind::Wifi, State::Off, true)), "{cards:?}");
    }

    #[test]
    fn a_pci_network_card_without_a_driver_has_its_model() {
        assert_eq!(
            card("none3").state,
            State::NoDriver("Realtek Semiconductor Co., Ltd. RTL8125 2.5GbE Controller".into())
        );
    }

    #[test]
    fn a_driverless_card_of_class_0x0280_is_wifi() {
        assert_eq!(card("none4").kind, Kind::Wifi);
    }

    #[test]
    fn dhcp_is_automatic() {
        assert_eq!(addressing("WPA SYNCDHCP"), Addressing::Automatic);
    }

    #[test]
    fn an_address_with_its_prefix_is_manual() {
        assert_eq!(addressing("inet 10.0.0.5/24 mtu 9000"), manual([10, 0, 0, 5], Some(24)));
    }

    #[test]
    fn a_netmask_gives_the_prefix() {
        assert_eq!(
            addressing("inet 10.0.0.5 netmask 255.255.0.0 broadcast 10.0.255.255"),
            manual([10, 0, 0, 5], Some(16))
        );
    }

    #[test]
    fn a_hex_netmask_gives_the_prefix() {
        assert_eq!(addressing("inet 10.0.0.5 netmask 0xffffff00"), manual([10, 0, 0, 5], Some(24)));
    }

    #[test]
    fn an_address_alone_has_no_prefix() {
        assert_eq!(addressing("inet 10.0.0.5"), manual([10, 0, 0, 5], None));
    }

    #[test]
    fn a_bare_first_address_is_inet() {
        assert_eq!(addressing("192.0.2.5/24 NOAUTO"), manual([192, 0, 2, 5], Some(24)));
    }

    #[test]
    fn an_address_after_other_settings_is_inet() {
        assert_eq!(addressing("up mtu 9000 192.0.2.5/24"), manual([192, 0, 2, 5], Some(24)));
    }

    #[test]
    fn up_and_mtu_are_settings_the_page_keeps() {
        assert_eq!(addressing("WPA SYNCDHCP up mtu 1400 NOAUTO"), Addressing::Automatic);
    }

    #[test]
    fn freebsd_cloud_images_default_is_automatic() {
        assert_eq!(addressing("SYNCDHCP accept_rtadv"), Addressing::Automatic);
    }

    #[test]
    fn noauto_is_off() {
        assert!(parse_settings("DHCP noauto").off);
    }

    #[test]
    fn up_alone_is_unset() {
        assert_eq!(addressing("up"), Addressing::Unset);
    }

    #[test]
    fn what_the_page_doesnt_change_is_by_hand() {
        let values = [
            "inet6 2001:db8::5 prefixlen 64",
            "inet 10.0.0.5/24 alias",
            "inet 10.0.0.5/24 inet 10.0.0.6/24",
            "10.0.0.5/24 10.0.0.6/24",
            "inet 10.0.0.5/24 up 10.0.0.6",
            "HOSTAP inet 10.0.0.1/24",
            "DHCP inet 10.0.0.5/24",
            "inet 010.0.0.5/24",
            "inet 10.0.0.5/33",
            "vnet jail1",
            "DHCP description 192.0.2.5",
            "DHCP media 1000baseT",
            "-tso DHCP",
            "DHCP inet6 accept_rtadv",
            "DHCP mtu big",
        ];

        assert!(values.iter().all(|v| addressing(v) == Addressing::ByHand), "{values:?}");
    }

    #[test]
    fn an_empty_line_of_its_own_doesnt_fall_back_to_the_default() {
        assert_eq!(settings("ifconfig_DEFAULT: DHCP\nifconfig_em1: \n", "em1").addressing, Addressing::Unset);
    }

    #[test]
    fn a_vlans_setting_has_rcs_name() {
        assert_eq!(ifconfig_var("em0.10"), "ifconfig_em0_10");
    }

    #[test]
    fn a_hook_of_ours_gives_its_servers() {
        let text = "# hyprbsd-network: em0's DNS servers, through resolvconf(8).\nprintf 'nameserver %s\\n' 192.0.2.53 192.0.2.54 | /sbin/resolvconf -a em0:static\n";

        assert_eq!(parse_hook(text, "em0"), [Ipv4Addr::new(192, 0, 2, 53), Ipv4Addr::new(192, 0, 2, 54)]);
    }

    #[test]
    fn another_cards_hook_gives_nothing() {
        let text = "# hyprbsd-network: em1's DNS servers, through resolvconf(8).\nprintf 'nameserver %s\\n' 192.0.2.53 | /sbin/resolvconf -a em1:static\n";

        assert!(parse_hook(text, "em0").is_empty());
    }

    #[test]
    fn resolvconfs_file_for_a_card_gives_its_ipv4_servers() {
        let text = "# Generated by resolvconf\nsearch home\nnameserver 10.1.9.1\nnameserver fe80::1%em0\nnameserver 192.0.2.53\n";

        assert_eq!(parse_resolv(text), [Ipv4Addr::new(10, 1, 9, 1), Ipv4Addr::new(192, 0, 2, 53)]);
    }

    #[test]
    fn the_owners_hook_gives_nothing() {
        assert!(parse_hook("ifconfig em0 media 100baseTX\n", "em0").is_empty());
    }

    fn cards(route: Option<(&str, [u8; 4])>) -> Cards {
        Cards {
            cards: listed(),
            defaultrouter: Some(Ipv4Addr::new(192, 168, 1, 1)),
            route: route.map(|(iface, g)| Route { iface: iface.into(), gateway: Some(Ipv4Addr::from(g)) }),
        }
    }

    #[test]
    fn a_manual_card_shows_defaultrouter_on_its_network() {
        assert_eq!(cards(None).gateway("em0"), Some(Ipv4Addr::new(192, 168, 1, 1)));
    }

    #[test]
    fn an_automatic_card_shows_the_default_routes_gateway_through_it() {
        assert_eq!(cards(Some(("re0", [10, 0, 0, 1]))).gateway("re0"), Some(Ipv4Addr::new(10, 0, 0, 1)));
    }

    #[test]
    fn an_automatic_card_without_the_default_route_shows_none() {
        assert_eq!(cards(Some(("em0", [192, 168, 1, 1]))).gateway("re0"), None);
    }

    /// em0 and em1 manual on 192.0.2.0/24, in `order`, with defaultrouter 192.0.2.1.
    fn same_network(order: [&str; 2]) -> Cards {
        let rc = "ifconfig_em0: inet 192.0.2.5/24\nifconfig_em1: inet 192.0.2.6/24\n";
        let cards = order
            .iter()
            .map(|name| Card {
                name: (*name).into(),
                kind: Kind::Wired,
                state: State::Connected,
                inet: Vec::new(),
                settings: settings(rc, name),
                dns: Vec::new(),
                part_of: None,
                members: Vec::new(),
            })
            .collect();
        Cards { cards, defaultrouter: Some(Ipv4Addr::new(192, 0, 2, 1)), route: None }
    }

    #[test]
    fn two_manual_cards_on_the_gateways_network_both_show_it() {
        let gateways = [["em0", "em1"], ["em1", "em0"]].map(|order| same_network(order).gateway("em1"));

        assert_eq!(gateways, [Some(Ipv4Addr::new(192, 0, 2, 1)); 2]);
    }

    #[test]
    fn a_card_on_the_gateways_network_takes_nothing_over_from_another_on_it() {
        assert_eq!(same_network(["em0", "em1"]).takes_over("em1"), None);
    }

    #[test]
    fn a_gateway_on_another_card_says_it_moves() {
        assert_eq!(
            cards(None).takes_over("ue0").as_deref(),
            Some("em0 has the gateway 192.168.1.1. Applying moves it to ue0.")
        );
    }

    #[test]
    fn the_card_with_the_gateway_takes_nothing_over() {
        assert_eq!(cards(Some(("em0", [192, 168, 1, 1]))).takes_over("em0"), None);
    }

    #[test]
    fn a_default_route_by_dhcp_on_another_card_says_it_takes_over() {
        let mut cards = cards(Some(("re0", [10, 0, 0, 1])));
        cards.defaultrouter = None;

        assert_eq!(cards.takes_over("em0").as_deref(), Some("re0 gets a gateway by DHCP; em0's takes over."));
    }

    #[test]
    fn a_gateway_on_no_cards_network_says_its_replaced() {
        let mut cards = cards(None);
        cards.defaultrouter = Some(Ipv4Addr::new(172, 16, 0, 1));

        assert_eq!(
            cards.takes_over("em0").as_deref(),
            Some("The computer's gateway is 172.16.0.1. Applying replaces it with em0's.")
        );
    }

    #[test]
    fn manual_settings_are_checked_into_the_helpers_arguments() {
        let manual = Manual::check("192.168.1.5", "24", "192.168.1.1", "1.1.1.1, 9.9.9.9").unwrap();

        assert_eq!(manual.args(), ["manual", "192.168.1.5/24", "192.168.1.1", "1.1.1.1", "9.9.9.9"]);
    }

    #[test]
    fn no_gateway_is_a_dash() {
        let manual = Manual::check("192.168.1.5", "/24", "", "192.0.2.53").unwrap();

        assert_eq!(manual.args(), ["manual", "192.168.1.5/24", "-", "192.0.2.53"]);
    }

    #[test]
    fn a_netmask_is_taken_for_the_prefix() {
        assert_eq!(Manual::check("10.0.0.5", "255.255.0.0", "", "").unwrap().prefix, 16);
    }

    #[test]
    fn bad_manual_settings_are_refused() {
        let bad = [
            ("192.168.1.256", "24", "", ""),
            ("010.0.0.5", "24", "", ""),
            ("10.0.0.5", "33", "", ""),
            ("10.0.0.5", "0", "", ""),
            ("10.0.0.5", "255.0.255.0", "", ""),
            ("10.0.0.0", "24", "", ""),
            ("10.0.0.255", "24", "", ""),
            ("127.0.0.5", "8", "", ""),
            ("224.0.0.5", "24", "", ""),
            ("0.0.0.5", "24", "", ""),
            ("10.0.0.5", "24", "10.0.1.1", ""),
            ("10.0.0.5", "24", "10.0.0.5", ""),
            ("10.0.0.5", "24", "10.0.0.255", ""),
            ("10.0.0.5", "32", "10.0.0.1", ""),
            ("10.0.0.5", "24", "", "1.2.3"),
            ("10.0.0.5", "24", "", "239.1.1.1"),
            ("10.0.0.5", "24", "", "1.1.1.1 1.0.0.1 9.9.9.9 8.8.8.8"),
        ];

        let passed: Vec<_> = bad.iter().filter(|(a, p, g, d)| Manual::check(a, p, g, d).is_ok()).collect();

        assert!(passed.is_empty(), "{passed:?}");
    }

    #[test]
    fn a_31_takes_the_other_address_as_its_gateway() {
        assert!(Manual::check("10.0.0.4", "31", "10.0.0.5", "").is_ok());
    }

    #[test]
    fn a_lagg_line_has_its_mode_and_its_ports_in_order() {
        let words = lagg_words("up laggproto failover laggport re0 laggport wlan0 DHCP");

        assert_eq!(words, (Some("failover"), vec!["re0", "wlan0"]));
    }

    #[test]
    fn a_lagg_line_of_laggproto_laggports_and_dhcp_is_automatic() {
        assert_eq!(addressing("DHCP up laggproto failover laggport re0 laggport wlan0"), Addressing::Automatic);
    }

    #[test]
    fn a_lagg_line_with_lagghash_is_set_up_by_hand() {
        assert_eq!(addressing("laggproto lacp lagghash l2 laggport re0 laggport ue0 DHCP"), Addressing::ByHand);
    }

    fn all() -> Cards {
        Cards { cards: listed(), defaultrouter: None, route: None }
    }

    #[test]
    fn a_new_laggs_ports_are_the_wired_usb_and_wifi_cards_in_no_bridge_or_lagg() {
        let cards = all();

        let names: Vec<&str> = cards.ports_for("lagg1").map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["em0", "wlan0"]);
    }

    #[test]
    fn a_laggs_own_ports_are_its_ports_too() {
        let cards = all();

        let names: Vec<&str> = cards.ports_for("lagg0").map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["em0", "re0", "ue0", "wlan0"]);
    }

    #[test]
    fn a_card_set_up_by_hand_cant_be_a_port() {
        let mut cards = all();
        cards.cards[0].settings = parse_settings("DHCP description office");

        assert!(!cards.ports_for("lagg1").any(|c| c.name == "em0"));
    }

    #[test]
    fn a_new_lagg_takes_the_first_name_no_card_has() {
        assert_eq!(all().free_lagg(), "lagg1");
    }
}
