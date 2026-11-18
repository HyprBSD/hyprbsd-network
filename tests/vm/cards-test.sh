#!/bin/sh
#
# hyprbsd-network's cards in a bhyve guest (tests/vm/prepare.sh); boots 2 to
# 6 each run a stage of tests/vm/guest/cards-checks: Manual with gateway and
# DNS, DHCP, Manual again, kept after a restart; off, still off; on, still
# on; the window's Automatic and Manual from the keyboard, kept; polkit's
# actions; hyprbsd-network without Wi-Fi; none of the host's IPv4
# addresses reachable from the test networks. The guest's four cards are on
# the run's two VALE switches (tests/vm/lib.sh). The window's screenshots go
# to <dir>/shots.
#
#   doas tests/vm/cards-test.sh
#
# Build it first, as yourself: make.
#
# SPDX-License-Identifier: BSD-2-Clause

set -eu

here=$(cd "$(dirname "$0")" && pwd)
. "$here/lib.sh"
. "$here/prepare.sh"

need_root
vm_begin "$WORK/vm/cards-test"
shots="iface-list iface-settings iface-automatic iface-manual-typed iface-manual"
prepare_guest cards

# The host's IPv4 addresses, its default gateway and an address on the
# internet, none of which the guest may reach; and the host's IPv6 addresses
# other than loopback and link-local.
{
	awk '$1 == "inet" && $2 !~ /^127\./ { print $2 }' "$VM_RUN/host-ifconfig.before"
	awk '/^Internet:/ { v4 = 1 } /^Internet6:/ { v4 = 0 } v4 && $1 == "default" { print $2 }' \
		"$VM_RUN/host-routes.before"
	echo 1.1.1.1
} | sort -u >"$dir/seed/outside"
awk '$1 == "inet6" && $2 !~ /^(fe80:|::1$)/ { sub(/%.*/, "", $2); print $2 }' "$VM_RUN/host-ifconfig.before" |
	sort -u >"$dir/seed/outside6"
printf '%s\n' 'install -m 0444 /media/seed/outside /root/hnet-outside' \
	'install -m 0444 /media/seed/outside6 /root/hnet-outside6' >"$dir/seed/setup-more"
seal_guest

# The checks each boot's stage must pass, one name a line.
networks() {
	cat <<'EOF'
the jail hnetdhcp has vtnet2 and vtnet3
dnsmasq listens for DHCP in the jail
the guest's own cards are vtnet0 vtnet1
vtnet0 has a lease on 10.78.1.0/24
the jail answers at 10.78.1.1
the jail's DNS server answers at 10.78.1.1
the jail answers at 10.78.2.1
the jail's DNS server answers at 10.78.2.1
EOF
	sed "s/.*/& doesn't answer/" "$dir/seed/outside"
	echo "the guest has no IPv6 default route"
}
expected() {
	case $1 in
	2)
		networks
		cat <<'EOF'
net.route.multipath is 0, as a distribution sets it
polkit's rule for the helper is installed
polkit's org.hyprbsd.network.wifi: the helper's wifi, implicitly auth_admin_keep
polkit's org.hyprbsd.network.setup: the helper's wifi-setup, implicitly auth_admin_keep
polkit's org.hyprbsd.network.iface-switch: the helper's iface-switch, implicitly auth_admin_keep
polkit's org.hyprbsd.network.iface: the helper's iface, implicitly auth_admin_keep
polkit's org.hyprbsd.network.lagg: the helper's lagg, implicitly auth_admin_keep
the guest has no Wi-Fi adapter
wifi-setup without an adapter says so
and sets nothing up
tester's hyprbsd-network status names the wired card
tester's hyprbsd-network list says Wi-Fi isn't set up
Manual with gateway and DNS: the address, the default route, defaultrouter, resolv.conf
the helper's iface dhcp gives vtnet1 an address by DHCP again
and Manual again
EOF
		;;
	3)
		echo "Manual kept after a restart"
		echo "iface-switch off: down, NOAUTO, its DNS server and default route gone"
		;;
	4)
		echo "still off after a restart"
		echo "iface-switch on: up with its address"
		;;
	5)
		cat <<'EOF'
still on after a restart
sway starts
the window opens
the window's Automatic gives vtnet1 an address by DHCP through pkexec
the window's Manual gives vtnet1 the typed address through pkexec
the screenshots are on the transfer disk
EOF
		for s in $shots; do
			echo "screenshot $s"
		done
		;;
	6)
		echo "the window's Manual address kept after a restart"
		networks
		;;
	esac
}

run_boots cards 6 expected
shots_back 5 $shots
vm_end
say "cards-test: $passed checks passed"
