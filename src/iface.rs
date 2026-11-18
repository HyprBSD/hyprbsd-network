//! Network interfaces as ifconfig and route show them.

use std::io;
use std::net::Ipv4Addr;
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum IfaceError {
    #[error("couldn't run {0}: {1}")]
    Run(&'static str, io::Error),
    #[error("{0} failed: {1}")]
    Failed(&'static str, String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub up: bool,
    /// ifconfig's status: active, no carrier, associated...
    pub status: Option<String>,
    pub wifi: bool,
    /// IPv4 addresses and their prefix lengths.
    pub inet: Vec<(Ipv4Addr, u32)>,
    /// It has an Ethernet address.
    pub ether: bool,
    pub groups: Vec<String>,
    /// A bridge's members, or a lagg's ports.
    pub members: Vec<String>,
}

/// A route as `route -n get` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub iface: String,
    /// None for a route straight to an interface.
    pub gateway: Option<Ipv4Addr>,
}

/// The interfaces in ifconfig's output.
pub fn parse_ifconfig(text: &str) -> Vec<Interface> {
    let mut found: Vec<Interface> = Vec::new();
    for line in text.lines() {
        if let Some((name, rest)) = line.split_once(": flags=").filter(|_| !line.starts_with(char::is_whitespace)) {
            let flags = rest.split_once('<').and_then(|(_, f)| f.split_once('>')).map_or("", |(f, _)| f);
            found.push(Interface {
                name: name.to_owned(),
                up: flags.split(',').any(|f| f == "UP"),
                status: None,
                wifi: false,
                inet: Vec::new(),
                ether: false,
                groups: Vec::new(),
                members: Vec::new(),
            });
            continue;
        }
        let Some(iface) = found.last_mut() else { continue };
        let line = line.trim_start();
        if let Some(status) = line.strip_prefix("status: ") {
            iface.status = Some(status.to_owned());
        } else if let Some(groups) = line.strip_prefix("groups: ") {
            iface.groups = groups.split_whitespace().map(str::to_owned).collect();
            iface.wifi = iface.groups.iter().any(|g| g == "wlan");
        } else if let Some(inet) = line.strip_prefix("inet ") {
            iface.inet.extend(parse_inet(inet));
        } else if line.starts_with("ether ") {
            iface.ether = true;
        } else if let Some(member) = line.strip_prefix("member: ").or_else(|| line.strip_prefix("laggport: ")) {
            iface.members.extend(member.split(' ').next().map(str::to_owned));
        }
    }
    found
}

/// "10.1.9.16 netmask 0xffffff00 ..." as an address and prefix length.
fn parse_inet(text: &str) -> Option<(Ipv4Addr, u32)> {
    let mut words = text.split(' ');
    let address = words.next()?.parse().ok()?;
    let mask = words.skip_while(|w| *w != "netmask").nth(1)?.strip_prefix("0x")?;
    Some((address, u32::from_str_radix(mask, 16).ok()?.count_ones()))
}

/// The interface and gateway in `route -n get`'s output.
pub fn parse_route(text: &str) -> Option<Route> {
    let field = |name: &str| text.lines().find_map(|line| line.trim_start().strip_prefix(name));
    let iface = field("interface: ")?.to_owned();
    Some(Route { iface, gateway: field("gateway: ").and_then(|g| g.parse().ok()) })
}

/// Every interface, from ifconfig.
pub fn interfaces() -> Result<Vec<Interface>, IfaceError> {
    let out = Command::new("/sbin/ifconfig")
        .env_remove("IFCONFIG_FORMAT")
        .output()
        .map_err(|e| IfaceError::Run("ifconfig", e))?;
    if !out.status.success() {
        return Err(IfaceError::Failed("ifconfig", String::from_utf8_lossy(&out.stderr).trim().to_owned()));
    }
    Ok(parse_ifconfig(&String::from_utf8_lossy(&out.stdout)))
}

/// The interface the default IPv4 route goes through, if there is one.
pub fn default_route() -> Result<Option<String>, IfaceError> {
    Ok(route()?.map(|route| route.iface))
}

/// The default IPv4 route, if there is one.
pub fn route() -> Result<Option<Route>, IfaceError> {
    let out =
        Command::new("/sbin/route").args(["-n", "get", "default"]).output().map_err(|e| IfaceError::Run("route", e))?;
    // Without a default route, route fails and prints no interface.
    Ok(parse_route(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const IFCONFIG: &str = "\
em0: flags=1008843<UP,BROADCAST,RUNNING,SIMPLEX,MULTICAST,LOWER_UP> metric 0 mtu 1500
\toptions=8209b<RXCSUM,TXCSUM,VLAN_MTU,VLAN_HWTAGGING,VLAN_HWCSUM,WOL_MAGIC,LINKSTATE>
\tether 02:00:00:00:00:10
\tinet 10.1.9.16 netmask 0xffffff00 broadcast 10.1.9.255
\tinet 10.1.9.17 netmask 0xffffffff broadcast 10.1.9.17
\tmedia: Ethernet autoselect (1000baseT <full-duplex>)
\tstatus: active
\tnd6 options=29<PERFORMNUD,IFDISABLED,AUTO_LINKLOCAL>
re0: flags=8802<BROADCAST,SIMPLEX,MULTICAST> metric 0 mtu 1500
\tether 02:00:00:00:00:11
\tmedia: Ethernet autoselect
\tstatus: no carrier
lo0: flags=1008049<UP,LOOPBACK,RUNNING,MULTICAST,LOWER_UP> metric 0 mtu 16384
\tinet 127.0.0.1 netmask 0xff000000
\tinet6 ::1 prefixlen 128
\tgroups: lo
wlan0: flags=8843<UP,BROADCAST,RUNNING,SIMPLEX,MULTICAST> metric 0 mtu 1500
\tether 02:00:00:00:00:12
\tinet 192.168.1.20 netmask 0xffffff00 broadcast 192.168.1.255
\tgroups: wlan
\tssid \"status: active\" channel 6 (2437 MHz 11g ht/20) bssid 02:00:00:00:00:01
\tregdomain ETSI country GB authmode WPA2/802.11i privacy ON deftxkey UNDEF
\tmedia: IEEE 802.11 Wireless Ethernet MCS mode 11ng
\tstatus: associated
";

    fn parsed(name: &str) -> Interface {
        parse_ifconfig(IFCONFIG).into_iter().find(|i| i.name == name).unwrap()
    }

    #[test]
    fn finds_every_interface_in_order() {
        let names: Vec<String> = parse_ifconfig(IFCONFIG).into_iter().map(|i| i.name).collect();

        assert_eq!(names, ["em0", "re0", "lo0", "wlan0"]);
    }

    #[test]
    fn reads_every_ipv4_address_and_prefix() {
        assert_eq!(parsed("em0").inet, [(Ipv4Addr::new(10, 1, 9, 16), 24), (Ipv4Addr::new(10, 1, 9, 17), 32)]);
    }

    #[test]
    fn reads_the_link_status() {
        assert_eq!(parsed("re0").status.as_deref(), Some("no carrier"));
    }

    #[test]
    fn an_interface_without_up_is_down() {
        assert!(!parsed("re0").up);
    }

    #[test]
    fn an_interface_in_the_wlan_group_is_wifi_and_a_wired_one_isnt() {
        assert_eq!((parsed("wlan0").wifi, parsed("em0").wifi), (true, false));
    }

    #[test]
    fn a_network_name_doesnt_pass_for_a_status_line() {
        assert_eq!(parsed("wlan0").status.as_deref(), Some("associated"));
    }

    const CLONES: &str = "\
vm-public: flags=1008843<UP,BROADCAST,RUNNING,SIMPLEX,MULTICAST,LOWER_UP> metric 0 mtu 1500
\tether 8a:8d:fa:9c:d1:87
\tid 00:00:00:00:00:00 priority 32768 hellotime 2 fwddelay 15
\tbridge flags=0<>
\tmember: tap0 flags=143<LEARNING,DISCOVER,AUTOEDGE,AUTOPTP>
\tmember: em1 flags=143<LEARNING,DISCOVER,AUTOEDGE,AUTOPTP>
\tgroups: bridge vm-switch viid-4c918@
lagg0: flags=1008843<UP,BROADCAST,RUNNING,SIMPLEX,MULTICAST,LOWER_UP> metric 0 mtu 1500
\tether 02:00:00:00:00:13
\tlaggproto failover lagghash l2,l3,l4
\tlaggport: re1 flags=5<MASTER,ACTIVE>
\tlaggport: wlan1 flags=0<>
\tgroups: lagg
usbus0: flags=0<> metric 0 mtu 65536
\tgroups: usbus
";

    fn clone(name: &str) -> Interface {
        parse_ifconfig(CLONES).into_iter().find(|i| i.name == name).unwrap()
    }

    #[test]
    fn an_interface_with_an_ether_line_has_an_ethernet_address_and_loopback_doesnt() {
        assert_eq!((parsed("em0").ether, parsed("lo0").ether), (true, false));
    }

    #[test]
    fn reads_every_group() {
        assert_eq!(clone("vm-public").groups, ["bridge", "vm-switch", "viid-4c918@"]);
    }

    #[test]
    fn reads_a_bridges_members() {
        assert_eq!(clone("vm-public").members, ["tap0", "em1"]);
    }

    #[test]
    fn reads_a_laggs_ports() {
        assert_eq!(clone("lagg0").members, ["re1", "wlan1"]);
    }

    #[test]
    fn route_reads_the_interface_and_gateway() {
        let text = "   route to: 0.0.0.0\ndestination: 0.0.0.0\n       mask: 0.0.0.0\n    gateway: 10.1.9.1\n        fib: 0\n  interface: re0\n      flags: <UP,GATEWAY,DONE,STATIC>\n";

        assert_eq!(
            parse_route(text),
            Some(Route { iface: "re0".to_owned(), gateway: Some(Ipv4Addr::new(10, 1, 9, 1)) })
        );
    }

    #[test]
    fn a_route_to_an_interface_has_no_gateway() {
        let text = "   route to: 0.0.0.0\ndestination: 0.0.0.0\n       mask: 0.0.0.0\n        fib: 0\n  interface: tun0\n      flags: <UP,DONE,STATIC>\n";

        assert_eq!(parse_route(text), Some(Route { iface: "tun0".to_owned(), gateway: None }));
    }

    #[test]
    fn no_route_without_an_interface_line() {
        assert_eq!(parse_route("route: route has not been found\n"), None);
    }
}
