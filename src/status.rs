//! How the machine is connected, for Waybar and `hyprbsd-network status`.

use crate::iface::{self, IfaceError, Interface};
use crate::wifi::{self, Ssid, Strength};
use crate::wpa::Socket;
use std::io::{self, Write};
use std::net::Ipv4Addr;
use std::thread;
use std::time::Duration;

/// What wpa_supplicant says about a wlan.
#[derive(Debug, Clone)]
pub struct WifiView {
    pub status: wifi::Status,
    /// dBm
    pub signal: Option<i32>,
}

/// wpa_supplicant's states between choosing a network and finishing its handshake.
const JOINING: [&str; 5] = ["AUTHENTICATING", "ASSOCIATING", "ASSOCIATED", "4WAY_HANDSHAKE", "GROUP_HANDSHAKE"];
/// How often `follow` looks again.
const TICK: Duration = Duration::from_secs(5);

/// What a connection goes through: the default route's interface or, with no
/// default route, a joined wlan or an active wired interface with an address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    /// Through Wi-Fi; its name and strength if wpa_supplicant says.
    Wifi {
        iface: String,
        ssid: Option<Ssid>,
        strength: Option<Strength>,
        address: Option<Ipv4Addr>,
    },
    /// Through another interface.
    Wired {
        iface: String,
        address: Option<Ipv4Addr>,
    },
    /// Neither, and Wi-Fi is joining a network or waiting for an address.
    Connecting {
        ssid: Option<Ssid>,
    },
    Offline,
}

impl Connection {
    /// From the default route's interface, the interfaces, and what
    /// wpa_supplicant says about a wlan (`wifi`, by name).
    pub fn decide(
        route: Option<&str>,
        interfaces: &[Interface],
        mut wifi: impl FnMut(&str) -> Option<WifiView>,
    ) -> Self {
        if let Some(name) = route {
            let found = interfaces.iter().find(|i| i.name == name);
            let address = found.and_then(address);
            if !found.is_some_and(|i| i.wifi) {
                return Connection::Wired { iface: name.to_owned(), address };
            }
            return Connection::on_wifi(name, address, wifi(name));
        }
        let mut connecting = None;
        for iface in interfaces.iter().filter(|i| i.wifi) {
            let Some(view) = wifi(&iface.name) else { continue };
            if view.status.joined() && address(iface).is_some() {
                return Connection::on_wifi(&iface.name, address(iface), Some(view));
            }
            if connecting.is_none() && (view.status.joined() || JOINING.contains(&view.status.state.as_str())) {
                connecting = Some(Connection::Connecting { ssid: view.status.ssid });
            }
        }
        let wired =
            interfaces.iter().find(|i| !i.wifi && i.status.as_deref() == Some("active") && address(i).is_some());
        if let Some(iface) = wired {
            return Connection::Wired { iface: iface.name.clone(), address: address(iface) };
        }
        connecting.unwrap_or(Connection::Offline)
    }

    /// Through the wlan `name`, with what wpa_supplicant says if it has joined a network.
    fn on_wifi(name: &str, address: Option<Ipv4Addr>, view: Option<WifiView>) -> Self {
        let view = view.filter(|v| v.status.joined());
        Connection::Wifi {
            iface: name.to_owned(),
            ssid: view.as_ref().and_then(|v| v.status.ssid.clone()),
            strength: view.and_then(|v| v.signal).map(Strength::from_dbm),
            address,
        }
    }

    /// Lines for people: what it's connected through, then the address.
    pub fn lines(&self) -> Vec<String> {
        let address = |iface: &str, address: &Option<Ipv4Addr>| match address {
            Some(a) => format!("{a} on {iface}"),
            None => "No address yet".to_owned(),
        };
        match self {
            Connection::Wifi { iface, ssid, strength, address: a } => {
                let mut first = String::from("Wi-Fi");
                if let Some(ssid) = ssid {
                    first = format!("{first}: {}", ssid.display());
                }
                if let Some(strength) = strength {
                    first = format!("{first}, {} signal", strength.describe());
                }
                vec![first, address(iface, a)]
            }
            Connection::Wired { iface, address: a } => vec![format!("Wired: {iface}"), address(iface, a)],
            Connection::Connecting { ssid: Some(ssid) } => vec![format!("Connecting to {}", ssid.display())],
            Connection::Connecting { ssid: None } => vec!["Connecting to Wi-Fi".to_owned()],
            Connection::Offline => vec!["Not connected".to_owned()],
        }
    }

    /// Waybar's JSON line: an icon, the lines as a tooltip, and a class.
    pub fn bar(&self) -> String {
        let (icon, class) = match self {
            Connection::Wifi { strength: Some(Strength::Weak), .. } => ('\u{f091f}', "wifi"),
            Connection::Wifi { strength: Some(Strength::Fair), .. } => ('\u{f0922}', "wifi"),
            Connection::Wifi { strength: Some(Strength::Good), .. } => ('\u{f0925}', "wifi"),
            Connection::Wifi { strength: Some(Strength::Strong), .. } => ('\u{f0928}', "wifi"),
            Connection::Wifi { strength: None, .. } => ('\u{f05a9}', "wifi"),
            Connection::Wired { .. } => ('\u{f0200}', "wired"),
            Connection::Connecting { .. } => ('\u{f092f}', "connecting"),
            Connection::Offline => ('\u{f0c9b}', "offline"),
        };
        serde_json::json!({ "text": icon.to_string(), "tooltip": markup(&self.lines().join("\n")), "class": class })
            .to_string()
    }
}

/// How the machine is connected now.
pub fn current() -> Result<Connection, IfaceError> {
    let route = iface::default_route()?;
    let interfaces = iface::interfaces()?;
    Ok(Connection::decide(route.as_deref(), &interfaces, wifi_view))
}

/// What wpa_supplicant says about `iface`, if it answers.
fn wifi_view(iface: &str) -> Option<WifiView> {
    let mut ctrl = Socket::open(iface).ok()?;
    let status = wifi::status(&mut ctrl).ok()?;
    let signal = wifi::signal(&mut ctrl).ok().flatten();
    Some(WifiView { status, signal })
}

/// Writes Waybar's line, then a new one whenever it changes, looking every 5 s.
pub fn follow(out: &mut impl Write) -> io::Result<()> {
    let mut last = String::new();
    loop {
        let line = current().unwrap_or(Connection::Offline).bar();
        if line != last {
            writeln!(out, "{line}")?;
            out.flush()?;
            last = line;
        }
        thread::sleep(TICK);
    }
}

/// An interface's first IPv4 address.
fn address(iface: &Interface) -> Option<Ipv4Addr> {
    iface.inet.first().map(|(a, _)| *a)
}

/// Text as Pango markup, which Waybar's tooltips are.
fn markup(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(name: &str, wifi: bool, address: Option<[u8; 4]>) -> Interface {
        Interface {
            name: name.into(),
            up: true,
            status: Some("active".into()),
            wifi,
            inet: address.map(|a| (Ipv4Addr::from(a), 24)).into_iter().collect(),
            ether: true,
            groups: if wifi { vec!["wlan".into()] } else { Vec::new() },
            members: Vec::new(),
        }
    }

    fn view(state: &str, ssid: &[u8], signal: Option<i32>) -> WifiView {
        WifiView {
            status: wifi::Status {
                state: state.into(),
                ssid: Some(Ssid::new(ssid).unwrap()),
                id: Some(0),
                address: None,
            },
            signal,
        }
    }

    fn no_wifi(_: &str) -> Option<WifiView> {
        None
    }

    mod decide {
        use super::*;

        #[test]
        fn a_route_through_a_joined_wlan_is_wifi() {
            let ifaces = [iface("em0", false, None), iface("wlan0", true, Some([192, 168, 1, 20]))];

            let c = Connection::decide(Some("wlan0"), &ifaces, |_| Some(view("COMPLETED", b"Home", Some(-55))));

            assert_eq!(
                c,
                Connection::Wifi {
                    iface: "wlan0".into(),
                    ssid: Some(Ssid::new(b"Home").unwrap()),
                    strength: Some(Strength::Strong),
                    address: Some(Ipv4Addr::new(192, 168, 1, 20)),
                }
            );
        }

        #[test]
        fn a_route_through_a_wlan_wpa_supplicant_wont_describe_is_wifi_without_a_name() {
            let ifaces = [iface("wlan0", true, Some([192, 168, 1, 20]))];

            let c = Connection::decide(Some("wlan0"), &ifaces, no_wifi);

            assert!(matches!(c, Connection::Wifi { ssid: None, strength: None, .. }), "{c:?}");
        }

        #[test]
        fn a_route_through_another_interface_is_wired_even_with_wifi_joined() {
            let ifaces = [iface("em0", false, Some([10, 1, 9, 16])), iface("wlan0", true, None)];

            let c = Connection::decide(Some("em0"), &ifaces, |_| Some(view("COMPLETED", b"Home", Some(-55))));

            assert_eq!(c, Connection::Wired { iface: "em0".into(), address: Some(Ipv4Addr::new(10, 1, 9, 16)) });
        }

        #[test]
        fn no_route_and_wifi_mid_handshake_is_connecting() {
            let ifaces = [iface("wlan0", true, None)];

            let c = Connection::decide(None, &ifaces, |_| Some(view("4WAY_HANDSHAKE", b"Home", None)));

            assert_eq!(c, Connection::Connecting { ssid: Some(Ssid::new(b"Home").unwrap()) });
        }

        #[test]
        fn no_route_and_wifi_joined_but_without_an_address_is_connecting() {
            let ifaces = [iface("wlan0", true, None)];

            let c = Connection::decide(None, &ifaces, |_| Some(view("COMPLETED", b"Home", None)));

            assert!(matches!(c, Connection::Connecting { .. }), "{c:?}");
        }

        #[test]
        fn no_route_and_wifi_joined_with_an_address_is_wifi() {
            let ifaces = [iface("wlan0", true, Some([192, 168, 1, 20]))];

            let c = Connection::decide(None, &ifaces, |_| Some(view("COMPLETED", b"Home", Some(-55))));

            assert_eq!(
                c,
                Connection::Wifi {
                    iface: "wlan0".into(),
                    ssid: Some(Ssid::new(b"Home").unwrap()),
                    strength: Some(Strength::Strong),
                    address: Some(Ipv4Addr::new(192, 168, 1, 20)),
                }
            );
        }

        #[test]
        fn no_route_and_an_active_wired_interface_with_an_address_is_wired() {
            let mut lo0 = iface("lo0", false, Some([127, 0, 0, 1]));
            lo0.status = None;
            let ifaces = [lo0, iface("em0", false, Some([10, 1, 9, 16]))];

            let c = Connection::decide(None, &ifaces, no_wifi);

            assert_eq!(c, Connection::Wired { iface: "em0".into(), address: Some(Ipv4Addr::new(10, 1, 9, 16)) });
        }

        #[test]
        fn no_route_and_a_wired_interface_without_a_carrier_is_offline() {
            let mut em0 = iface("em0", false, Some([10, 1, 9, 16]));
            em0.status = Some("no carrier".into());

            let c = Connection::decide(None, &[em0], no_wifi);

            assert_eq!(c, Connection::Offline);
        }

        #[test]
        fn no_route_and_wifi_scanning_is_offline() {
            let ifaces = [iface("wlan0", true, None)];

            let c = Connection::decide(None, &ifaces, |_| Some(view("SCANNING", b"Home", None)));

            assert_eq!(c, Connection::Offline);
        }

        #[test]
        fn asks_wpa_supplicant_only_about_wlans() {
            let ifaces = [iface("em0", false, None), iface("wlan0", true, None)];
            let mut asked = Vec::new();

            Connection::decide(None, &ifaces, |name| {
                asked.push(name.to_owned());
                None
            });

            assert_eq!(asked, ["wlan0"]);
        }
    }

    mod lines {
        use super::*;

        #[test]
        fn wifi_names_the_network_strength_and_address() {
            let c = Connection::Wifi {
                iface: "wlan0".into(),
                ssid: Some(Ssid::new(b"Home").unwrap()),
                strength: Some(Strength::Good),
                address: Some(Ipv4Addr::new(192, 168, 1, 20)),
            };

            assert_eq!(c.lines(), ["Wi-Fi: Home, good signal", "192.168.1.20 on wlan0"]);
        }

        #[test]
        fn wired_names_the_interface() {
            let c = Connection::Wired { iface: "em0".into(), address: None };

            assert_eq!(c.lines(), ["Wired: em0", "No address yet"]);
        }

        #[test]
        fn connecting_names_the_network() {
            let c = Connection::Connecting { ssid: Some(Ssid::new(b"Home").unwrap()) };

            assert_eq!(c.lines(), ["Connecting to Home"]);
        }

        #[test]
        fn offline_says_so() {
            assert_eq!(Connection::Offline.lines(), ["Not connected"]);
        }
    }

    mod bar {
        use super::*;

        fn json(c: &Connection) -> serde_json::Value {
            serde_json::from_str(&c.bar()).unwrap()
        }

        #[test]
        fn is_one_line_with_a_newline_in_a_network_name_shown_as_an_escape() {
            let c = Connection::Connecting { ssid: Some(Ssid::new(b"two\nlines").unwrap()) };

            let bar = c.bar();

            let parsed: serde_json::Value = serde_json::from_str(&bar).unwrap();
            assert!(!bar.contains('\n') && parsed["tooltip"] == r"Connecting to two\x0alines", "{bar}");
        }

        #[test]
        fn has_a_class_for_each_state() {
            let classes: Vec<serde_json::Value> = [
                Connection::Wifi { iface: "wlan0".into(), ssid: None, strength: None, address: None },
                Connection::Wired { iface: "em0".into(), address: None },
                Connection::Connecting { ssid: None },
                Connection::Offline,
            ]
            .iter()
            .map(|c| json(c)["class"].clone())
            .collect();

            assert_eq!(classes, ["wifi", "wired", "connecting", "offline"]);
        }

        #[test]
        fn escapes_markup_in_a_network_name() {
            let c = Connection::Connecting { ssid: Some(Ssid::new(b"<b>Tom & Jerry</b>").unwrap()) };

            assert_eq!(json(&c)["tooltip"], "Connecting to &lt;b&gt;Tom &amp; Jerry&lt;/b&gt;");
        }
    }
}
