# hyprbsd-network

![CI](https://github.com/HyprBSD/hyprbsd-network/actions/workflows/check.yml/badge.svg?branch=main)
![FreeBSD](https://img.shields.io/badge/FreeBSD-15-ab2b28?logo=freebsd)
![License](https://img.shields.io/github/license/HyprBSD/hyprbsd-network)
[![Buy Me a Coffee](https://img.shields.io/badge/Buy%20Me%20a%20Coffee-support-ffdd00?logo=buymeacoffee)](https://buymeacoffee.com/sheridans)

Wi-Fi and network settings for [HyprBSD](https://hyprbsd.org). The window lists the Wi-Fi
networks in range, joins and forgets them, and turns Wi-Fi on and off. Its Interfaces tab sets
each network card to DHCP or a fixed address, gateway and DNS, and makes link aggregations
(lagg). You can also list, join and forget Wi-Fi networks from a terminal, and
`hyprbsd-network status --follow` prints connection updates as JSON for Waybar.

![The Wi-Fi list, with a wrong password message under one network and notes on why two others can't be joined](docs/wifi.png)

![A wired card's settings in the Interfaces tab, with Manual selected and boxes for the address, prefix, gateway and DNS server](docs/wired.png)

![The lagg page, with failover selected, two cards ticked in order, and a note that failover needs every card on the same network](docs/lagg.png)

It runs as you and talks to FreeBSD's own wpa_supplicant. Setting up Wi-Fi, turning it or a
card on and off, and changing a card's settings go through its root helper,
`hyprbsd-network-helper`, using pkexec. hyprbsd-network(8) has the details.

## Install

It comes with HyprBSD, which builds it from this repository's releases.

## Usage

- **From the menu:** HyprBSD Network, under Settings.
- **`hyprbsd-network`** opens the window, or brings forward the one already open.
- **`hyprbsd-network list`** lists the networks in range, **`join NAME`** joins one,
  **`forget NAME`** forgets it, and **`status`** shows the connection.
- **The manual page:** `man hyprbsd-network`.

## Building

On HyprBSD you need Rust (FreeBSD's `rust` package) and `git`; `make deny` also needs
`cargo-deny`:

```
make            # target/release/hyprbsd-network
make check      # rustfmt's check, the tests, clippy, the manual page and installation checks
make deny       # cargo-deny: the crates' licences, advisories and sources (deny.toml)
make install    # as root, under /usr/local; then service devd restart
```

Run `make check` as an ordinary user: as root, the helper's tests skip some of their cases.
[CONTRIBUTING.md](CONTRIBUTING.md) has the tests, including the ones in bhyve guests, and how a
release is made. Report security problems privately: see [SECURITY.md](SECURITY.md).

## Licence

BSD-2-Clause; see `LICENSE`.
