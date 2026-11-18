# Shell functions the guests' checks share, sourced by cards-checks and
# wifi-checks. Each check prints "@@check ok|FAIL <what>", with "@@got" lines
# saying why it failed, and counts its failures in $failures; "@@seen" lines
# record without checking. The window's screenshots go to the disk with
# serial HNETSHOTS as a tar archive, each with a "@@shot <name> <bytes>
# <sha256>" line.
#
# SPDX-License-Identifier: BSD-2-Clause

PATH=/sbin:/bin:/usr/sbin:/usr/bin:/usr/local/sbin:/usr/local/bin
export PATH

user=tester
helper=/usr/local/libexec/hyprbsd-network-helper
RUN=/tmp/hnet-window
SHOTS=/var/log/hnet-window
TOOLS=/usr/local/bin:/usr/bin:/bin
failures=0

# check <what> <command...>: "@@check ok|FAIL <what>" for the command's
# result, which it returns.
check() {
	_what=$1
	shift
	if "$@"; then
		echo "@@check ok $_what"
		return 0
	fi
	echo "@@check FAIL $_what"
	failures=$((failures + 1))
	return 1
}

got() {
	echo "@@got $*"
	return 1
}

# said <text> <output>: the output has the text.
said() {
	case $2 in
	*"$1"*) ;;
	*) got "it said: $2" ;;
	esac
}

# The disk with this serial number.
disk() {
	geom disk list | awk -v s="$1" '/^Geom name:/ { n = $3 } $1 == "ident:" && $2 == s { print n }'
}

addr_of() {
	ifconfig "$1" 2>/dev/null | awk '$1 == "inet" { print $2; exit }'
}

# wait_addr <card> <pattern> <seconds>: the card's IPv4 address matches the
# pattern within the time.
wait_addr() {
	_i=0
	while [ $_i -lt "$3" ]; do
		case $(addr_of "$1") in
		$2) return 0 ;;
		esac
		sleep 1
		_i=$((_i + 1))
	done
	got "$1 has '$(addr_of "$1")' after $3 s, not $2"
}

gateway() {
	route -n get default 2>/dev/null | awk '$1 == "gateway:" { print $2 }'
}

# helper_runs <args...>: the helper as root, as polkit runs it for the window.
helper_runs() {
	_out=$("$helper" "$@" 2>&1) || got "the helper's $* said: $_out"
}

# stack_is <cards> [jail]: the network stack's cards, the jail's with a jail,
# sorted.
stack_is() {
	_l=$(${2:+jexec "$2"} ifconfig -l 2>&1 | tr ' ' '\n' | sort | paste -s -d ' ' -)
	[ "$_l" = "$1" ] || got "${2:-the guest}'s stack has '$_l', not '$1'"
}

dnsmasq_listens() {
	jexec hnetdhcp sockstat -4 -l -p 67 | grep -q dnsmasq || got "nothing listens on DHCP's port in the jail"
}

# The test networks' jail serves them, and the guest's own cards are the
# ones named, vtnet0 with a lease on 10.78.1.0/24.
fixture() {
	check "the jail hnetdhcp has vtnet2 and vtnet3" stack_is "lo0 vtnet2 vtnet3" hnetdhcp
	check "dnsmasq listens for DHCP in the jail" dnsmasq_listens
	check "the guest's own cards are $1" stack_is "lo0 $1"
	check "vtnet0 has a lease on 10.78.1.0/24" wait_addr vtnet0 '10.78.1.1[0-9][0-9]' 60
}

# hyprbsd-network as $user, without root's XDG_RUNTIME_DIR; its arguments in
# $1, run by su's shell.
as_user_network() {
	su -m "$user" -c "env -u XDG_RUNTIME_DIR /usr/local/bin/hyprbsd-network $1"
}

# A command as $user in the headless sway's session, run by su's shell.
as_user() {
	su -m "$user" -c "env PATH=$TOOLS HOME=/home/$user XDG_RUNTIME_DIR=$RUN WAYLAND_DISPLAY=$display $1"
}

# sway's Wayland socket in $RUN within 10 s, as $display.
sway_up() {
	_i=0
	while [ $_i -lt 50 ]; do
		for _s in "$RUN"/wayland-*; do
			[ -S "$_s" ] && display=${_s##*/} && return 0
		done
		sleep 0.2
		_i=$((_i + 1))
	done
	got "sway didn't start: $(tail -n 1 "$RUN/sway.log")"
}

windows() {
	swaymsg -s "$(ls "$RUN"/sway-ipc.*.sock 2>/dev/null | head -n 1)" -t get_tree 2>/dev/null |
		grep -c '"name": "HyprBSD Network"'
}

window_opens() {
	_i=0
	while [ "$(windows)" -lt 1 ] && [ $_i -lt 30 ]; do
		sleep 1
		_i=$((_i + 1))
	done
	[ "$(windows)" -eq 1 ] || got "sway has $(windows) HyprBSD Network windows after 30 s: $(tail -n 3 "$RUN/window.log")"
}

# window_start: as $user, a headless sway, a keyboard wtype keeps there, and
# the window, its trace in $RUN/window.log; false if either doesn't start.
window_start() {
	rm -rf "$RUN" "$SHOTS"
	install -d -o "$user" -m 0700 "$RUN"
	install -d -o "$user" -m 0755 "$SHOTS"
	cat >"$RUN/sway.conf" <<'EOF'
output HEADLESS-1 resolution 520x640
default_border none
for_window [title="^HyprBSD Network$"] floating enable, resize set 520 640, move position center
EOF
	service dbus status >/dev/null 2>&1 || service dbus start >/dev/null 2>&1
	su -m "$user" -c "env -u WAYLAND_DISPLAY PATH=$TOOLS HOME=/home/$user XDG_RUNTIME_DIR=$RUN WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 sway -c $RUN/sway.conf >$RUN/sway.log 2>&1 &"
	check "sway starts" sway_up || return
	as_user "wtype -k F24 -s 3600000 -k F24 >/dev/null 2>&1 &"
	as_user "HYPRBSD_NETWORK_TRACE=1 hyprbsd-network >$RUN/window.log 2>&1 &"
	check "the window opens" window_opens || return
	sleep 3
}

window_stop() {
	pkill -U "$user" -x hyprbsd-network || true
	pkill -U "$user" -x wtype || true
	pkill -U "$user" -x sway || true
	grep '^hyprbsd-network: ' "$RUN/window.log" | sed 's/^/@@seen trace /'
}

# The keys named, 150 ms apart, then a second for the window to draw.
keys() {
	_keys=
	for _k; do
		_keys="$_keys -k $_k -s 150"
	done
	as_user "wtype$_keys"
	sleep 1
}

shot() {
	check "screenshot $1" as_user "grim $SHOTS/$1.png"
}

shots() {
	for _png in "$SHOTS"/*.png; do
		[ -f "$_png" ] && echo "@@shot ${_png##*/} $(stat -f %z "$_png") $(sha256 -q "$_png")"
	done
	check "the screenshots are on the transfer disk" tar -cf "/dev/$(disk HNETSHOTS)" -C "$SHOTS" .
}
