# The bhyve suites' guest, sourced after lib.sh; its functions run after
# vm_begin. FreeBSD's cloud image with a seed disc (CIDATA) and a disc of
# FreeBSD's packages (PACKAGES, fetched by the host; the guest has no other
# way to them). Boot 1 runs the seed's user-data: the packages,
# hyprbsd-network from make install, the jail that serves the test networks
# (guest/hnetdhcp), a user tester in wheel, a polkit rule letting tester run
# the helper's iface without a password, and the system settings
# hyprbsd-network expects (net.route.multipath=0, a dhclient exit hook
# running card-gone); then the seed's setup-more, if a suite put one there.
# A step that fails powers the guest off without "@@setup done". Each later
# boot runs the suite's checks from rc.local and powers off.
#
# SPDX-License-Identifier: BSD-2-Clause

GUEST_PACKAGES="sway wtype grim mesa-dri mesa-libs dbus polkit noto-sans dnsmasq"
# Where make built the program.
: "${CARGO_TARGET_DIR:=$top/target}"

# A release build of the program for /usr/local, no older than its sources.
network_built() {
	_bin=$CARGO_TARGET_DIR/release/hyprbsd-network
	[ -x "$_bin" ] || fail "$_bin isn't built: make"
	[ -z "$(find "$top/src" "$top/build.rs" "$top/Cargo.toml" "$top/Cargo.lock" -newer "$_bin" 2>/dev/null)" ] ||
		fail "$_bin is older than its sources: make"
	"$top/tests/binary-paths-test.sh" "$_bin" /usr/local /usr/local "$dir" >/dev/null ||
		fail "$_bin wasn't built for /usr/local: make"
}

# prepare_guest <name> [package...]: the guest's disk, the transfer disk for
# its screenshots, and its seed in $dir/seed, with tests/vm/guest/<name>-checks
# for rc.local to run. The guest gets the FreeBSD packages named, or
# GUEST_PACKAGES if none are.
prepare_guest() {
	[ "${RELEASE%%.*}" = "$(uname -r | cut -d . -f 1)" ] ||
		fail "RELEASE $RELEASE isn't this host's FreeBSD $(uname -r | cut -d . -f 1): the guest takes the host's packages and build"
	_name=$1
	shift
	guest_packages=${*:-$GUEST_PACKAGES}
	network_built
	_image=$(cloud_image)
	say "Preparing the guest in $dir"
	mkdir -p "$dir/seed"
	cp "$_image" "$dir/disk.img"
	truncate -s 8G "$dir/disk.img"
	truncate -s 64M "$dir/shots.img"
	env -u MAKEFLAGS -u MFLAGS make -C "$top" install DESTDIR="$dir/seed/root" PREFIX=/usr/local \
		LOCALBASE=/usr/local CARGO_TARGET_DIR="$CARGO_TARGET_DIR" >"$dir/install.log" 2>&1 ||
		fail "make install failed; see $dir/install.log"
	install -m 0755 "$here/guest/hnetdhcp" "$here/guest/checks.sh" "$dir/seed/"
	install -m 0755 "$here/guest/$_name-checks" "$dir/seed/checks"
	echo "$guest_packages" >"$dir/seed/packages"

	cat >"$dir/seed/10-hnet-test.rules" <<'EOF'
polkit.addRule(function(action, subject) {
	if (action.id == "org.hyprbsd.network.iface" && subject.user == "tester")
		return polkit.Result.YES;
	return polkit.Result.NOT_HANDLED;
});
EOF
	cat >"$dir/seed/dhclient-exit-hooks" <<'EOF'
case $reason in
EXPIRE | FAIL) /usr/local/libexec/hyprbsd-network-helper card-gone "$interface" 2>&1 | logger -t hyprbsd-network & ;;
esac
EOF
	printf 'instance-id: hyprbsd-network-%s\nlocal-hostname: hnet-%s\n' "$_name" "$_name" >"$dir/seed/meta-data"
	cat >"$dir/seed/user-data" <<'USERDATA'
#!/bin/sh
exec >/dev/console 2>&1
set -e
trap '[ -f /var/db/hnet-test-boot ] || { echo "@@setup failed"; shutdown -p now; }' EXIT
echo "@@setup start"
mkdir -p /media/seed /media/packages /root/repos
mount -t cd9660 /dev/iso9660/CIDATA /media/seed
mount -t cd9660 /dev/iso9660/PACKAGES /media/packages
echo 'packages: { url: "file:///media/packages" }' >/root/repos/packages.conf
if ! env ASSUME_ALWAYS_YES=yes pkg -R /root/repos install -y $(cat /media/seed/packages) >/var/log/hnet-pkg.log 2>&1; then
	echo "@@setup packages failed: $(tail -n 3 /var/log/hnet-pkg.log | tr '\n' ' ')"
	exit 1
fi
echo "@@setup packages installed"
tar -cf - -C /media/seed/root . | tar -xpf - -C /
install -m 0555 /media/seed/hnetdhcp /usr/local/etc/rc.d/hnetdhcp
install -m 0444 /media/seed/checks.sh /root/checks.sh
install -m 0555 /media/seed/checks /root/checks
install -d -m 0755 /usr/local/etc/polkit-1/rules.d
install -m 0644 /media/seed/10-hnet-test.rules /usr/local/etc/polkit-1/rules.d/
install -m 0644 /media/seed/dhclient-exit-hooks /etc/dhclient-exit-hooks
echo net.route.multipath=0 >>/etc/sysctl.conf
pw useradd tester -m -G wheel -s /bin/sh -c "hyprbsd-network test user"
for v in $(sysrc -aN | grep -E '^ifconfig_'); do
	sysrc -x "$v"
done
sysrc ifconfig_vtnet0=DHCP ifconfig_vtnet1=DHCP hnetdhcp_enable=YES dbus_enable=YES >/dev/null
echo "@@setup rc.conf: $(sysrc -a | grep -E '^ifconfig_' | tr '\n' ' ')"
[ ! -f /media/seed/setup-more ] || sh -e /media/seed/setup-more
umount /media/seed /media/packages
printf '%s\n' 'exec >/dev/console 2>&1' '/root/checks' 'shutdown -p now' >/etc/rc.local
echo 2 >/var/db/hnet-test-boot
rm -f /firstboot
echo "@@setup done"
shutdown -p now
sleep 600
USERDATA
}

# seal_guest: the seed disc, and the packages fetched onto theirs.
seal_guest() {
	makefs -t cd9660 -o rockridge -o label=CIDATA "$dir/seed.iso" "$dir/seed"
	say "Fetching the guest's packages"
	fetch_packages "$dir/packages" pkg $guest_packages
	makefs -t cd9660 -o rockridge -o label=PACKAGES "$dir/packages.iso" "$dir/packages"
}

# run_boots <name> <last boot> <expected> [bhyve slot args...]: boot 1 and
# then each boot to the last, each to its power-off. Boot 1 must install the
# packages and finish; each later one must have the jail's DHCP ready, and
# its checks (tagged <name>-<boot>) must pass, among them every one the
# function <expected> prints for that boot, one name a line. The slot
# arguments are given to every boot after the guest's disks and cards. A boot
# has 30 minutes, and a console silent for VM_STALL seconds stops it sooner.
run_boots() {
	_suite=$1 _last=$2 _expected=$3
	shift 3
	passed=0
	_boot=1
	while [ "$_boot" -le "$_last" ]; do
		say "Boot $_boot (serial log: $dir/boot$_boot.log)"
		_ended=0
		run_vm "$_suite" "$dir/boot$_boot.log" 1800 \
			-s 2,virtio-blk,"$dir/disk.img" -s 3,ahci-cd,"$dir/seed.iso" -s 4,ahci-cd,"$dir/packages.iso" \
			$(net_slots) -s 9,virtio-blk,"$dir/shots.img",ser=HNETSHOTS "$@" || _ended=$?
		[ "$_ended" -eq 1 ] || fail "boot $_boot ended with status $_ended rather than powering off; see $dir/boot$_boot.log"
		serial "boot$_boot" >"$dir/boot$_boot.txt"
		grep -a -E '^@@' "$dir/boot$_boot.txt" || true
		if [ "$_boot" -eq 1 ]; then
			grep -a -q -x '@@setup packages installed' "$dir/boot1.txt" || fail "boot 1 didn't install the packages"
			grep -a -q -x '@@setup done' "$dir/boot1.txt" || fail "boot 1 didn't finish; see $dir/boot1.log"
		else
			grep -a -q '^@@dhcp ready ' "$dir/boot$_boot.txt" || fail "boot $_boot: the jail's DHCP wasn't ready"
			"$_expected" "$_boot" >"$dir/expected$_boot"
			_why=$(_guest_ran_listed "$dir/boot$_boot.txt" "$_suite-$_boot" "$dir/expected$_boot") ||
				fail "boot $_boot: $_why; see $dir/boot$_boot.log"
			passed=$((passed + $(grep -c . "$dir/expected$_boot")))
		fi
		_boot=$((_boot + 1))
	done
}

# _guest_ran_listed <log> <tag> <file>: guest_ran with the checks named in the
# file, one a line.
_guest_ran_listed() {
	set -f
	IFS='
'
	set -- "$1" "$2" $(cat "$3")
	unset IFS
	set +f
	guest_ran "$@"
}

# shots_back <boot> <name...>: the transfer disk's screenshots in $dir/shots,
# each as the guest gave it at that boot.
shots_back() {
	_boot=$1
	shift
	rm -rf "$dir/shots"
	mkdir "$dir/shots"
	tar -xf "$dir/shots.img" -C "$dir/shots" || fail "the transfer disk holds no archive of the screenshots"
	_why=$(guest_shots "$dir/boot$_boot.txt" "$dir/shots" "$@") || fail "$_why"
	say "The window's screenshots: $dir/shots/*.png"
}
