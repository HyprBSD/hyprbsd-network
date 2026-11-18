# Contributing to hyprbsd-network

Bug reports, fixes and new features are welcome. Open an issue or a pull request on GitHub,
against `main`. For a security problem, don't open an issue: see [SECURITY.md](SECURITY.md).

A bug report helps most with the FreeBSD version (`freebsd-version -ku`), hyprbsd-network's
version (on HyprBSD, `pkg info hyprbsd-network`; built from source, the tag or commit), what you
did and what happened, and the network as the machine sees it: `hyprbsd-network status`,
`ifconfig -a`, and rc.conf's `ifconfig_*`, `wlans_*` and `defaultrouter` lines
(`sysrc -a | grep -E '^(ifconfig_|wlans_|defaultrouter)'`). Leave out network passwords:
wpa_supplicant's file has them.

## Before a pull request

- `make check` and `make deny` pass. GitHub runs rustfmt's check, cargo-deny and `make check`
  in FreeBSD 15.1, as an ordinary user, on every push and pull request
  (`.github/workflows/check.yml`).
- A change in behaviour comes with a test that fails without it.
- A change to the helper (`libexec/`), its devd rule or polkit's actions passes the bhyve tests
  too: `make test`, and `make test-wifi` when it touches Wi-Fi.
- Code is formatted by `cargo fmt` (`rustfmt.toml`) and clean under clippy with warnings denied.
  Comments say what the code does, briefly.
- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/):
  `feat:`, `fix:`, `docs:`, `test:`, `refactor:`, `chore:` and the rest. The changelog is made
  from them with git-cliff (`cliff.toml`), which leaves out any other commit.

Contributions are under the BSD-2-Clause licence, as the rest of hyprbsd-network (`LICENSE`).

## Tests

`make check` runs as an ordinary user and needs no network card of its own. It runs the Rust
tests, which talk to a model of wpa_supplicant's control socket (`tests/fake/`);
`tests/helper-test.sh`, the helper on scratch roots (`-r`) and against stand-ins for the
commands it runs (as root those stand-ins are skipped); the paths test and the binary's paths
under other prefixes; `make install` and `make dist` (`tests/install-test.sh`), with each
installed devd rule checked by `tests/devd-test.sh`; the manual page; and the bhyve tests'
library with stand-ins (`tests/vm/lib-test.sh`).

The bhyve tests run hyprbsd-network in guests on FreeBSD's cloud image:

```
make test          # the cards: DHCP, Manual, off and on across restarts, the window
make test-wifi     # Wi-Fi, with this host's Wi-Fi card lent to the guest
```

The Wi-Fi test uses the stock devd at every boot. It records before the first test boot that
nothing has set the card up, so the settings found after it are devd's rule's. It also stops
wpa_supplicant with SIGSTOP before devd starts (`tests/vm/guest/hnetteststuck`), standing in
for one stuck in the driver, and checks the start still finishes with the settings put back
and nothing restarted.

They run as root through doas and need bhyve (`vmm`), netmap (in FreeBSD's GENERIC kernel) and
the `bhyve-firmware` package. A guest's four network cards are on two VALE switches made for the
run, which the host has no interface on, so the host's own network, addresses and routes are
left alone; the test networks' DHCP and DNS come from a jail inside the guest. The host's pkg
fetches the FreeBSD packages a guest needs onto a disc of its own. A run's guests are named
`hnet-<suite>-<run>`; a run refuses to start while another holds the tests' lock, while a VM of
its name exists, or while a VALE port is on its switches, and afterwards checks that none of its
ports is left and that the host's interfaces and routes are as they were (a route's Expire
countdown aside). The tests keep downloads and
guests in `WORK`, `/var/hyprbsd-network` unless set: `make test WORK=/somewhere/else`.
`RELEASE` sets the guests' FreeBSD release, which must be of the host's major version (the guests
take the host's build and its packages); `VM_MEM` and `VM_CPUS` set their memory and CPUs.

bhyve's virtio cards always have a link, so the tests don't lose a card's carrier: what the
helper does when a link goes down or comes back is tested by `tests/helper-test.sh`, and on
hardware.

`make test-wifi` lends the guest a Wi-Fi card on bhyve's passthrough driver, which no other bhyve
may have: `devctl set driver -f <device> ppt` (and `devctl clear driver -f <device>` to give it
back). `WIFI_FIRMWARE` is the card's firmware package file, built for the guest's release:
`make test-wifi WIFI_FIRMWARE=/path/to/wifi-firmware-<driver>-kmod-<family>-<version>.pkg`. The
guest joins a network from the host's `/etc/wpa_supplicant.conf` (`WIFI_SSID` picks one), so it
reaches that network.

## Releases

A release is a tag, `v` and the version in `Cargo.toml`'s `[package]`, for example `v0.2.0`,
with its source tarball attached. It needs `git-cliff` (FreeBSD's package):

1. Set the version in `Cargo.toml`, and run `make check`, `make deny` and the bhyve tests.
2. Add the release to the changelog, commit it with the new version, then tag it:

   ```
   git cliff --unreleased --tag v<version> --prepend CHANGELOG.md
   git commit -m "chore(release): prepare for v<version>" Cargo.toml Cargo.lock CHANGELOG.md
   git tag -a v<version> -m "hyprbsd-network <version>"
   ```

   The first release makes `CHANGELOG.md`: `-o CHANGELOG.md` in place of
   `--prepend CHANGELOG.md`, and `git add CHANGELOG.md` before the commit. Then `git status`
   shows nothing to commit, and `git show v<version>:Cargo.toml` has the new version.
3. Push `main` and the tag, and wait for GitHub's checks to pass.
4. `make dist` makes `target/hyprbsd-network-<version>.tar.gz` from the tag. Make the tag's
   GitHub release with that tarball attached and `git cliff --latest --strip header` as its
   notes.

The port fetches that tarball, so it must not change once the release is out: a fix is a new
version.
