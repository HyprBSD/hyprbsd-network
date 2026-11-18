# Settings and functions for the bhyve tests. Sourced, never run.
#
# A run holds the suite lock (VM_LOCK) from vm_begin to its exit, and its
# bhyve inherits it. Its guest's four cards are ports v0 to v3 on two VALE
# switches named for the run (vale<RUN_ID>a and vale<RUN_ID>b), which the
# host has no interface on; valectl only lists them, and a run reads each
# listing whole or fails. A guest's VM is hnet-<name>-<RUN_ID>, started only
# when VMM_DIR has none of that name, and destroyed by that name only while
# or after the bhyve this run started runs. On exit, a failure or a signal,
# the run stops its guest and checks that none of its ports is left, the
# other VALE ports are the same ones, and the host's interfaces (ifconfig
# -a) and routes (netstat -rn, a route's Expire countdown aside) are as they
# were.
#
# SPDX-License-Identifier: BSD-2-Clause

# The C locale: tr and grep take the serial logs byte by byte.
LC_ALL=C
export LC_ALL

# This repository.
top=$(cd "$(dirname "$0")/../.." && pwd)
# The FreeBSD release of the guests' cloud image, of the host's major version.
: "${RELEASE:=15.1-RELEASE}"
# Downloads and guests.
: "${WORK:=/var/hyprbsd-network}"
IMAGES=$WORK/images
FIRMWARE=/usr/local/share/uefi-firmware/BHYVE_UEFI.fd
# Guest memory (wired) and CPUs.
: "${VM_MEM:=2G}"
: "${VM_CPUS:=2}"
# The suite lock, bhyve's VMs, the start of a guest's console's nmdm(4) path,
# seconds between looks at a running guest, a console silent this long
# (seconds) taken as hung; and in whole seconds, how long a new guest's ports
# have to open, how long one may be missing while its bhyve runs on, and how
# long a stopped guest's bhyve has to exit and its ports to go.
: "${VM_LOCK:=/var/run/hyprbsd-network-vm.lock}"
: "${VMM_DIR:=/dev/vmm}"
: "${NMDM:=/dev/nmdm}"
: "${VM_POLL:=2}"
: "${VM_STALL:=900}"
: "${VM_OPEN_WAIT:=30}"
: "${VM_CLOSE_WAIT:=30}"
: "${VM_STOP_WAIT:=30}"
# A line of valectl's listing.
VALE_LINE='^vale[^:[:space:]]*:[^[:space:]]+ bridge_idx [0-9]+ port_idx [0-9]+$'

say() {
	printf '==> %s\n' "$*"
}

say_failed() {
	printf 'FAIL: %s\n' "$*" >&2
}

fail() {
	say_failed "$@"
	exit 1
}

need_root() {
	[ "$(id -u)" -eq 0 ] || fail "run this as root (doas $0)."
}

# The serial log of a boot in $dir, without carriage returns: serial <boot>
serial() {
	tr -d '\r' <"$dir/$1.log"
}

# vm_begin <dir>: the suite lock; the clean-up on exit and on SIGINT, SIGHUP
# and SIGTERM; this run's switch names; <dir> (under $WORK/vm) emptied for
# the run; refused if valectl lists a port on either switch; then the VALE
# ports and the host's interfaces and routes as they are.
vm_begin() {
	dir=$1
	case $dir in
	"$WORK"/vm/*/* | "$WORK"/vm/. | "$WORK"/vm/..) fail "the run's directory $dir isn't one in $WORK/vm" ;;
	"$WORK"/vm/?*) ;;
	*) fail "the run's directory $dir isn't one in $WORK/vm" ;;
	esac
	command exec 9>>"$VM_LOCK" || fail "can't open the suite lock $VM_LOCK"
	lockf -s -t 0 9 ||
		fail "another run of these tests, or a bhyve it left, holds $VM_LOCK (fstat $VM_LOCK names it); wait for it to end"
	trap vm_exit EXIT
	trap 'vm_signal INT 130' INT
	trap 'vm_signal HUP 129' HUP
	trap 'vm_signal TERM 143' TERM
	vale_names
	rm -rf "$dir"
	mkdir -p "$dir/run"
	VM_RUN=$dir/run
	vale_inventory "$VM_RUN/vale.before" >"$VM_RUN/why" ||
		fail "can't list the VALE ports: $(cat "$VM_RUN/why")"
	_taken=$(vale_on_ours "$VM_RUN/vale.before")
	[ -z "$_taken" ] || fail "VALE ports not this run's are on its switches, left alone: $(echo $_taken)"
	printf '%s\n' $VALE_PORTS | sort >"$VM_RUN/vale.ours"
	VM_CLAIMED=yes
	_others=$(cd "$VMM_DIR" 2>/dev/null && ls -d hnet-* 2>/dev/null || true)
	[ -z "$_others" ] || say "VMs of other runs, left alone: $(echo $_others)" >&2
	host_state before || fail "can't read the host's interfaces and routes: $(cat "$VM_RUN"/host-*.before.err)"
	VM_HOST=yes
}

# vm_end: after the run's last guest, vm_after's checks, failing on any.
vm_end() {
	VM_ENDED=yes
	vm_after >"$VM_RUN/after" || fail "after the run: $(cat "$VM_RUN/after")"
	say "No VALE port of this run is left, the others are as they were, and so are the host's interfaces and routes"
}

vm_signal() {
	say "Interrupted (SIG$1)" >&2
	exit "$2"
}

# On exit: this run's guest stopped and its VM destroyed, then vm_after's
# checks unless vm_end made them; a failure there fails the run.
vm_exit() {
	_exit_st=$?
	trap '' INT HUP TERM
	if [ -n "${VM_PID:-}" ]; then
		say "Stopping this run's guest, $VM_NAME" >&2
		vm_stop || [ "$_exit_st" -ne 0 ] || _exit_st=1
	fi
	if ! vm_forget; then
		say_failed "couldn't destroy $VM_NAME, this run's VM"
		[ "$_exit_st" -ne 0 ] || _exit_st=1
	fi
	if [ "${VM_ENDED:-}" != yes ] && [ -n "${VM_RUN:-}" ] && ! vm_after >"$VM_RUN/after" 2>&1; then
		say_failed "after the run: $(cat "$VM_RUN/after")"
		[ "$_exit_st" -ne 0 ] || _exit_st=1
	fi
	exit "$_exit_st"
}

# This run's VALE switches, A and B, and its four ports in bhyve's slot
# order: vtnet0 and vtnet2 on A, vtnet1 and vtnet3 on B. RUN_ID (lower-case
# letters and digits) names them, random without it; bhyve takes a port name
# of at most 15 characters.
vale_names() {
	[ -n "${RUN_ID:-}" ] || RUN_ID=$(od -A n -N 3 -t x1 /dev/random | tr -d ' \n')
	case $RUN_ID in
	*[!0-9a-z]*) fail "RUN_ID '$RUN_ID' isn't lower-case letters and digits" ;;
	esac
	VALE_A=vale${RUN_ID}a
	VALE_B=vale${RUN_ID}b
	VALE_PORTS="$VALE_A:v0 $VALE_B:v1 $VALE_A:v2 $VALE_B:v3"
	for _p in $VALE_PORTS; do
		[ ${#_p} -le 15 ] || fail "the VALE port $_p is longer than bhyve's 15 characters; RUN_ID is too long"
	done
}

# bhyve's slot arguments for the guest's four cards, slots 5 to 8.
net_slots() {
	_slot=5
	for _p in $VALE_PORTS; do
		echo "-s $_slot,virtio-net,$_p"
		_slot=$((_slot + 1))
	done
}

# vale_inventory <file>: every VALE port valectl lists, as "<switch>:<port>
# <bridge> <port index>" lines sorted by name, in <file>. valectl's listing
# ends with status 1 whether or not it failed; anything on its standard
# error, another status, a line that isn't a port, a name or bridge and index
# listed twice, or a switch on two bridges or two switches on one, fails,
# saying why.
vale_inventory() {
	_st=0
	valectl >"$1.out" 2>"$1.err" || _st=$?
	if [ -s "$1.err" ]; then
		echo "valectl said: $(head -n 3 "$1.err" | tr '\n' ' ')"
		return 1
	fi
	if [ "$_st" -ne 1 ]; then
		echo "valectl ended with status $_st, not its listing's 1"
		return 1
	fi
	if grep -q -v -E "$VALE_LINE" "$1.out"; then
		echo "valectl listed a line that isn't a VALE port: '$(grep -v -E "$VALE_LINE" "$1.out" | head -n 1)'"
		return 1
	fi
	_odd=$(awk '{
		sw = $1
		sub(/:.*/, "", sw)
		if (names[$1]++) print "the port " $1 " twice"
		if (slots[$3 " " $5]++) print "bridge " $3 " port " $5 " twice"
		if ((sw in bridge) && bridge[sw] != $3) print "the switch " sw " on bridges " bridge[sw] " and " $3
		if (($3 in owner) && owner[$3] != sw) print "the switches " owner[$3] " and " sw " on bridge " $3
		bridge[sw] = $3
		owner[$3] = sw
	}' "$1.out" | head -n 1)
	if [ -n "$_odd" ]; then
		echo "valectl listed $_odd"
		return 1
	fi
	awk '{ print $1, $3, $5 }' "$1.out" | sort >"$1"
}

# vale_on_ours <inventory>: the names of the ports on this run's switches.
vale_on_ours() {
	awk -v a="$VALE_A" -v b="$VALE_B" '{ sw = $1; sub(/:.*/, "", sw) } sw == a || sw == b { print $1 }' "$1"
}

# vale_watch <seconds since bhyve started>: one look while the guest runs: on
# this run's switches no port but its four, all four within VM_OPEN_WAIT
# seconds, and none gone for VM_CLOSE_WAIT seconds while bhyve runs on;
# VM_SEEN=yes once all four are there.
vale_watch() {
	vale_inventory "$VM_RUN/vale.now" || return
	vale_on_ours "$VM_RUN/vale.now" >"$VM_RUN/vale.mine"
	_extra=$(grep -v -x -F -f "$VM_RUN/vale.ours" "$VM_RUN/vale.mine" || true)
	if [ -n "$_extra" ]; then
		echo "VALE ports not this run's are on its switches: $(echo $_extra)"
		return 1
	fi
	if cmp -s "$VM_RUN/vale.mine" "$VM_RUN/vale.ours"; then
		VM_SEEN=yes
		VM_SHORT_SINCE=
		return 0
	fi
	_missing=$(grep -v -x -F -f "$VM_RUN/vale.mine" "$VM_RUN/vale.ours" | paste -s -d ' ' -)
	if [ "$VM_SEEN" != yes ]; then
		[ "$1" -lt "$VM_OPEN_WAIT" ] || {
			echo "this run's ports didn't all open within ${VM_OPEN_WAIT}s: missing $_missing"
			return 1
		}
		return 0
	fi
	[ -n "${VM_SHORT_SINCE:-}" ] || VM_SHORT_SINCE=$1
	[ $(($1 - VM_SHORT_SINCE)) -le "$VM_CLOSE_WAIT" ] || {
		echo "this run's ports went while its guest ran on: missing $_missing"
		return 1
	}
}

# vale_left: none of this run's ports on its switches, within VM_STOP_WAIT
# seconds, and every other VALE port as before the run; says what's wrong.
vale_left() {
	_until=$(($(date +%s) + VM_STOP_WAIT))
	while :; do
		vale_inventory "$VM_RUN/vale.after" || return
		_mine=$(vale_on_ours "$VM_RUN/vale.after")
		[ -n "$_mine" ] && [ "$(date +%s)" -le "$_until" ] || break
		sleep 0.1
	done
	if [ -n "$_mine" ]; then
		echo "VALE ports are still on this run's switches: $(echo $_mine)"
		return 1
	fi
	cut -d ' ' -f 1 "$VM_RUN/vale.before" >"$VM_RUN/vale.before.names"
	cut -d ' ' -f 1 "$VM_RUN/vale.after" >"$VM_RUN/vale.after.names"
	if ! cmp -s "$VM_RUN/vale.before.names" "$VM_RUN/vale.after.names"; then
		echo "other VALE ports changed during the run: $(diff "$VM_RUN/vale.before.names" "$VM_RUN/vale.after.names" |
			sed -n 's/^</gone/p; s/^>/new/p' | tr '\n' ' ')"
		return 1
	fi
}

# host_state <when>: the host's interfaces and routes, in
# $VM_RUN/host-*.<when>, and the routes with each Expire countdown as
# "expires" in host-routes.<when>.cmp; false if either can't be read.
host_state() {
	ifconfig -a >"$VM_RUN/host-ifconfig.$1" 2>"$VM_RUN/host-ifconfig.$1.err" &&
		cp "$VM_RUN/host-ifconfig.$1" "$VM_RUN/host-ifconfig.$1.cmp" &&
		netstat -rn >"$VM_RUN/host-routes.$1" 2>"$VM_RUN/host-routes.$1.err" &&
		awk '
			$1 == "Destination" { columns = NF; expire = ($NF == "Expire") }
			$1 != "Destination" && expire && NF == columns && $NF ~ /^[0-9]+$/ { $NF = "expires" }
			{ print }
		' "$VM_RUN/host-routes.$1" >"$VM_RUN/host-routes.$1.cmp"
}

# vm_after: vale_left, and the host's interfaces and routes as before the
# run; says what differs.
vm_after() {
	_bad=0
	if [ "${VM_CLAIMED:-}" = yes ]; then
		vale_left || _bad=1
	fi
	if [ "${VM_HOST:-}" = yes ]; then
		if ! host_state after; then
			echo "can't read the host's interfaces and routes after the run: $(cat "$VM_RUN"/host-*.after.err)"
			return 1
		fi
		for _f in ifconfig routes; do
			if ! cmp -s "$VM_RUN/host-$_f.before.cmp" "$VM_RUN/host-$_f.after.cmp"; then
				echo "the host's $_f changed during the run:"
				diff -u "$VM_RUN/host-$_f.before.cmp" "$VM_RUN/host-$_f.after.cmp" | tail -n +3
				_bad=1
			fi
		done
	fi
	return $_bad
}

# Boot a guest and wait for it to stop. The VM is hnet-<name>-<RUN_ID>, its
# first serial port goes to <log>; returns bhyve's status (0 the guest
# restarted, 1 powered off, 2 halted, 3 triple fault, 4 error), 124 past
# <timeout> seconds or 125 after VM_STALL seconds with a silent console, each
# then stopped. While it runs, vale_watch looks at its switches; once its
# bhyve has ended, none of its ports is left and the other ports are as they
# were.
#
#   run_vm <name> <log> <timeout seconds> <bhyve -s slot args...>
run_vm() {
	_vm=hnet-$1-$RUN_ID _log=$2 _timeout=$3
	shift 3
	[ "${VM_CLAIMED:-}" = yes ] || fail "run_vm before vm_begin"
	[ ${#_vm} -le 31 ] || fail "the VM name $_vm is longer than vmm's 31 characters"
	[ ! -e "$VMM_DIR/$_vm" ] || fail "a VM named $_vm is in $VMM_DIR, not this run's; left alone"
	kldload -n vmm nmdm || fail "couldn't load vmm and nmdm"
	: >"$_log"
	( stty raw -echo && exec cat ) 9>&- <"$NMDM${_vm}B" >"$_log" 2>/dev/null &
	VM_READER=$!
	bhyve -c "$VM_CPUS" -m "$VM_MEM" -S -H -A -P \
		-s 0,hostbridge \
		"$@" \
		-s 31,lpc \
		-l com1,"$NMDM${_vm}A" \
		-l bootrom,"$FIRMWARE" \
		"$_vm" &
	VM_PID=$! VM_NAME=$_vm VM_SEEN=no VM_SHORT_SINCE=
	_t0=$(date +%s)
	_status=
	while kill -0 "$VM_PID" 2>/dev/null; do
		_now=$(date +%s)
		vale_watch $((_now - _t0)) >"$VM_RUN/watch" || fail "while $_vm ran: $(cat "$VM_RUN/watch")"
		if [ $((_now - _t0)) -ge "$_timeout" ]; then
			say "$_vm ran past ${_timeout}s; stopping it." >&2
			vm_stop || fail "couldn't stop $_vm"
			_status=124
			break
		fi
		if [ $((_now - $(stat -f %m "$_log"))) -gt "$VM_STALL" ]; then
			say "$_vm's console was silent for ${VM_STALL}s; stopping it as hung." >&2
			vm_stop || fail "couldn't stop $_vm"
			_status=125
			break
		fi
		sleep "$VM_POLL"
	done
	if [ -z "$_status" ]; then
		_status=0
		wait "$VM_PID" || _status=$?
	fi
	VM_PID=
	vm_forget || exit 1
	if ! vale_left >"$VM_RUN/left"; then
		VM_CLAIMED=reported
		fail "after $_vm: $(cat "$VM_RUN/left")"
	fi
	if [ "$VM_SEEN" != yes ] && [ "$_status" -le 1 ]; then
		fail "$_vm's four ports ($VALE_PORTS) were never all on its switches"
	fi
	return "$_status"
}

# vm_stop: this run's running guest destroyed, then up to VM_STOP_WAIT
# seconds for its bhyve to exit, then bhyve killed; false if it's still
# there.
vm_stop() {
	[ -n "${VM_PID:-}" ] || return 0
	if [ -e "$VMM_DIR/$VM_NAME" ]; then
		bhyvectl --destroy --vm="$VM_NAME" >/dev/null 2>&1 || true
	fi
	_pid_gone "$VM_PID" || {
		say "bhyve ($VM_PID) didn't exit; killing it" >&2
		kill -KILL "$VM_PID" 2>/dev/null || true
		_pid_gone "$VM_PID" || {
			say "bhyve ($VM_PID) is still running" >&2
			return 1
		}
	}
	wait "$VM_PID" 2>/dev/null || true
	VM_PID=
}

# _pid_gone <pid>: the process gone within VM_STOP_WAIT seconds.
_pid_gone() {
	_until=$(($(date +%s) + VM_STOP_WAIT))
	while kill -0 "$1" 2>/dev/null; do
		[ "$(date +%s)" -le "$_until" ] || return 1
		sleep 0.1
	done
}

# vm_forget: the console's reader stopped and, once this run's bhyve has
# ended, its VM destroyed if it's still there; false if it couldn't be.
vm_forget() {
	if [ -n "${VM_READER:-}" ]; then
		kill "$VM_READER" 2>/dev/null || true
		wait "$VM_READER" 2>/dev/null || true
		VM_READER=
	fi
	if [ -n "${VM_NAME:-}" ] && [ -z "${VM_PID:-}" ]; then
		if [ -e "$VMM_DIR/$VM_NAME" ]; then
			bhyvectl --destroy --vm="$VM_NAME" >/dev/null 2>&1 || return 1
		fi
		VM_NAME=
	fi
}

# FreeBSD's cloud-init VM image of the release (default $RELEASE), downloaded
# once and checked against the release's checksums.
cloud_image() {
	_rel=${1:-$RELEASE}
	_url=https://download.freebsd.org/releases/VM-IMAGES/$_rel/amd64/Latest
	_name=FreeBSD-$_rel-amd64-BASIC-CLOUDINIT-ufs.raw
	mkdir -p "$IMAGES"
	if [ ! -f "$IMAGES/$_name" ]; then
		say "Downloading $_name.xz" >&2
		rm -f "$IMAGES/CHECKSUM.SHA256" "$IMAGES/$_name.xz"
		fetch -q -o "$IMAGES/CHECKSUM.SHA256" "$_url/CHECKSUM.SHA256" || fail "couldn't download $_url/CHECKSUM.SHA256"
		fetch -q -o "$IMAGES/$_name.xz" "$_url/$_name.xz" || fail "couldn't download $_url/$_name.xz"
		_want=$(sed -n "s/^SHA256 ($_name.xz) = //p" "$IMAGES/CHECKSUM.SHA256")
		[ -n "$_want" ] || fail "CHECKSUM.SHA256 has no line for $_name.xz"
		[ "$(sha256 -q "$IMAGES/$_name.xz")" = "$_want" ] || fail "$_name.xz checksum mismatch"
		unxz "$IMAGES/$_name.xz" || fail "couldn't unpack $_name.xz"
	fi
	echo "$IMAGES/$_name"
}

# fetch_packages <dir> <package...>: FreeBSD's packages and the ones they
# depend on, fetched by the host's pkg from FreeBSD's repositories
# (/etc/pkg) into <dir>, with a package database of their own; then <dir>
# made a repository.
fetch_packages() {
	_dir=$1
	shift
	mkdir -p "$_dir" "$IMAGES/pkgdb"
	pkg -R /etc/pkg -o PKG_DBDIR="$IMAGES/pkgdb" fetch -q -y -d -o "$_dir" "$@" >&2 ||
		fail "pkg couldn't fetch $*"
	pkg repo -q "$_dir" >&2 || fail "pkg couldn't make $_dir a repository"
}

# guest_ran <log> <tag> <check...>: the guest's checks ended with status 0
# ("@@<tag> done status=0"), no check failed, and every named check passed
# ("@@check ok <name>"). Says what's wrong otherwise.
guest_ran() {
	_glog=$1 _tag=$2
	shift 2
	if ! grep -a -q -x "@@$_tag done status=0" "$_glog"; then
		echo "the guest's checks didn't end with status 0: $(grep -a "^@@$_tag done" "$_glog" | tail -n 1)"
		return 1
	fi
	_failed=$(grep -a '^@@check FAIL ' "$_glog" || true)
	if [ -n "$_failed" ]; then
		echo "$_failed"
		return 1
	fi
	for _name; do
		if ! grep -a -q -x -F "@@check ok $_name" "$_glog"; then
			echo "no check \"$_name\" passed"
			return 1
		fi
	done
}

# guest_shots <log> <dir> <name...>: <dir> holds exactly <name>.png for each
# name, each the size and SHA-256 the guest gave for it ("@@shot <name>.png
# <bytes> <sha256>"). Says what's wrong otherwise.
guest_shots() {
	_glog=$1 _gdir=$2
	shift 2
	_want=$(for _name; do echo "$_name.png"; done | sort)
	_have=$(find "$_gdir" -type f 2>/dev/null | sed 's|.*/||' | sort)
	if [ "$_want" != "$_have" ]; then
		echo "screenshots wanted: $(echo $_want); found: $(echo $_have)"
		return 1
	fi
	for _name; do
		_file=$_gdir/$_name.png
		_given=$(grep -a "^@@shot $_name.png " "$_glog" | tail -n 1 | cut -d ' ' -f 3,4)
		_got="$(stat -f %z "$_file") $(sha256 -q "$_file")"
		if [ "$_given" != "$_got" ]; then
			echo "$_name.png is $_got; the guest gave ${_given:-nothing} for it"
			return 1
		fi
	done
}
