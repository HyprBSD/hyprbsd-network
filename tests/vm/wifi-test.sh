#!/bin/sh
#
# hyprbsd-network's Wi-Fi in a bhyve guest (tests/vm/prepare.sh) with this
# host's Wi-Fi card lent to it: a card on bhyve's passthrough driver (ppt),
# which no other bhyve has. Boot 1 also installs WIFI_FIRMWARE, the card's
# firmware package built for the guest's release, and records that nothing
# has set the card up; boots 2 to 5 each run a stage of
# tests/vm/guest/wifi-checks: the card attached and devd's rule set it up;
# devd's rule with Wi-Fi set up by hand and wpa_supplicant stopped with
# SIGSTOP (tests/vm/guest/hnetteststuck); tester joins the network, the
# window and the commands; the network joined again after a restart. The
# guest joins a network from this host's
# /etc/wpa_supplicant.conf (WIFI_SSID picks one), so it reaches that network.
# The window's screenshots go to <dir>/shots.
#
#   doas env WIFI_FIRMWARE=<package file> tests/vm/wifi-test.sh
#
# Build it first, as yourself: make.
#
# SPDX-License-Identifier: BSD-2-Clause

set -eu

here=$(cd "$(dirname "$0")" && pwd)
. "$here/lib.sh"
. "$here/prepare.sh"

need_root
[ -n "${WIFI_FIRMWARE:-}" ] || fail "WIFI_FIRMWARE=<package file>: the Wi-Fi card's firmware, built for $RELEASE"
[ -f "$WIFI_FIRMWARE" ] || fail "$WIFI_FIRMWARE isn't a file"
firmware=$(pkg query -F "$WIFI_FIRMWARE" '%n-%v') || fail "$WIFI_FIRMWARE isn't a package"
abi=$(pkg query -F "$WIFI_FIRMWARE" '%q') || fail "$WIFI_FIRMWARE isn't a package"
[ "$abi" = "FreeBSD:${RELEASE%%.*}:amd64" ] ||
	fail "$WIFI_FIRMWARE is built for $abi, not FreeBSD:${RELEASE%%.*}:amd64 ($RELEASE)"
# Built on the release: FreeBSD_version 1501xxx for 15.1.
minor=${RELEASE#*.}
built=$(pkg query -F "$WIFI_FIRMWARE" '%At %Av' | awk '$1 == "FreeBSD_version" { print $2 }')
[ "${built%???}" = "$((${RELEASE%%.*} * 100 + ${minor%%-*}))" ] ||
	fail "$WIFI_FIRMWARE was built on FreeBSD_version '$built', not $RELEASE's"

card=$(pciconf -l | awk -F '[@:]' '/^ppt[0-9]+@pci/ && /class=0x0280/ { print $3 "/" $4 "/" $5; exit }')
[ -n "$card" ] || fail "no Wi-Fi card on bhyve's passthrough driver: devctl set driver -f <device> ppt"

# No other bhyve has the card in its command line.
card_free() {
	_st=0
	_procs=$(pgrep -l -f -- "passthru,$card") || _st=$?
	case $_st in
	1) return 0 ;;
	0) ;;
	*) fail "pgrep couldn't look for bhyve's with the card (status $_st)" ;;
	esac
	_holders=$(printf '%s\n' "$_procs" | awk -v card="$card" '
		$2 ~ /(^|\/)bhyve$/ { for (i = 3; i <= NF; i++) if ($i ~ "^[0-9]+(:[0-9]+)*,passthru," card "(,|$)") print $1 }')
	[ -z "$_holders" ] || fail "another bhyve ($(echo $_holders)) has the Wi-Fi card $card"
}
card_free

# The network the guest joins.
network=$(awk -v want="${WIFI_SSID:-}" '
	function value(s) { sub(/^[^"]*"/, "", s); sub(/"[^"]*$/, "", s); return s }
	/^[[:space:]]*network=/ { ssid = psk = ""; next }
	/^[[:space:]]*ssid="/ { ssid = value($0); next }
	/^[[:space:]]*psk="/ { psk = value($0); next }
	/^[[:space:]]*}/ && psk != "" && (want == "" || want == ssid) { print ssid; print psk; exit }
' /etc/wpa_supplicant.conf 2>/dev/null)
[ -n "$(printf '%s\n' "$network" | sed -n 2p)" ] ||
	fail "no network ${WIFI_SSID:+\"$WIFI_SSID\" }with a passphrase in /etc/wpa_supplicant.conf"

vm_begin "$WORK/vm/wifi-test"
chmod 0700 "$dir"
shots="joined open autoconnect-off autoconnect-on disconnected wifi-off-20s wifi-on"
say "The guest gets the Wi-Fi card $card, the firmware $firmware, and joins \"$(printf '%s\n' "$network" | sed -n 1p)\""
prepare_guest wifi
cp "$WIFI_FIRMWARE" "$dir/seed/firmware.pkg"
install -m 0755 "$here/guest/hnetteststuck" "$here/guest/stuck.sh" "$dir/seed/"
printf '%s\n' "$network" | sed -n 1p >"$dir/seed/wifi-ssid"
(umask 077 && printf '%s\n' "$network" | sed -n 2p >"$dir/seed/wifi-psk")
cat >"$dir/seed/setup-more" <<'EOF'
if ! pkg add /media/seed/firmware.pkg >/var/log/hnet-firmware.log 2>&1; then
	echo "@@setup firmware failed: $(tail -n 3 /var/log/hnet-firmware.log | tr '\n' ' ')"
	exit 1
fi
echo "@@setup firmware installed"
install -m 0444 /media/seed/wifi-ssid /root/hnet-wifi-ssid
install -m 0600 /media/seed/wifi-psk /root/hnet-wifi-psk
install -m 0444 /media/seed/stuck.sh /root/stuck.sh
install -m 0555 /media/seed/hnetteststuck /root/hnetteststuck
install -m 0444 /media/seed/root/usr/local/etc/devd/hyprbsd-network.conf /root/hnet-devd-rule
sysrc -aN >/var/db/hnet-rc-names
{
	grep -E '^(wlans_|wpa_supplicant_)' /var/db/hnet-rc-names
	grep -s -x 'update_config=1' /etc/wpa_supplicant.conf
} >/var/db/hnet-wifi-before || true
EOF
seal_guest

# The checks each boot's stage must pass, one name a line.
fixture() {
	cat <<'EOF'
the jail hnetdhcp has vtnet2 and vtnet3
dnsmasq listens for DHCP in the jail
the guest's own cards are vtnet0 vtnet1 wlan0
vtnet0 has a lease on 10.78.1.0/24
EOF
}
expected() {
	case $1 in
	2)
		echo "the lent card attached, the guest's one Wi-Fi adapter"
		echo "hyprbsd-network's devd rule is the one make install rendered"
		echo "before this start, nothing had set the card up"
		echo "devd's rule set the card up: wlans_, wlan0, wpa_supplicant.conf"
		fixture
		echo "hyprbsd-network's settings taken out, the wlan kept, as Wi-Fi set up by hand"
		;;
	3)
		echo "wlan0's wpa_supplicant was stopped before devd started"
		echo "with wpa_supplicant stopped, the start finished, devd's rule put the settings back and restarted nothing"
		;;
	4)
		echo "after the next start, wpa_supplicant uses the settings devd's rule put back"
		fixture
		cat <<'EOF'
tester joins the network with hyprbsd-network join, with an address and a working route
sway starts
the window opens
the window's Connect automatically off reaches /etc/wpa_supplicant.conf
and keeps the network joined
the window's Connect automatically on reaches /etc/wpa_supplicant.conf
the window's Disconnect leaves the network
Wi-Fi off: wlan0 NOAUTO and gone
Wi-Fi on in the same session: wlan0 made again and joined
Wi-Fi off again
Wi-Fi on with wlan0 made but not started, as at boot: joined again
a second start leaves one window
tester's hyprbsd-network list shows the network joined
status says Wired, then Wi-Fi, as does status --follow
a wrong password fails in words and changes nothing
the screenshots are on the transfer disk
EOF
		for s in $shots; do
			echo "screenshot $s"
		done
		;;
	5)
		cat <<'EOF'
the adapter's wlans_ line is wlan0
wlan0 with WPA and DHCP
wpa_supplicant's umask 077
wpa_supplicant.conf gives wheel the socket and lets wpa_supplicant save
wpa_supplicant.conf is root's, mode 0600
wpa_supplicant keeps the networks a scan found for an hour
wlan0 joins the network by itself and has an address
a working route through wlan0
tester's hyprbsd-network list shows the network joined
tester reads wpa_supplicant's state through the socket
tester's save_config keeps the file root's, mode 0600
EOF
		;;
	esac
}

card_free
run_boots wifi 5 expected -s 15,passthru,"$card"
shots_back 4 $shots
vm_end
say "wifi-test: $passed checks passed"
