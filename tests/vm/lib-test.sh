#!/bin/sh
#
# Checks tests/vm/lib.sh's hold on the host, with stand-ins for bhyve,
# bhyvectl, valectl and the host's ifconfig and netstat: the suite lock, held
# on by a bhyve whose run was killed; the run's VALE switch and VM names;
# valectl's listing read whole and fail-closed, each switch on a bridge of
# its own; a VM started only under a name /dev/vmm doesn't have, and
# destroyed only when this run started it; while the guest runs, no port on
# the run's switches but its four, all four open in time and none gone;
# afterwards none of them left and every other port as it was, after a
# power-off, a failed start, a time limit, a silent console or an
# interrupt; the host's interfaces and routes compared, an Expire countdown
# aside; then guest_ran, guest_shots and cloud_image. Runs as anyone,
# without bhyve.
#
#   tests/vm/lib-test.sh
#
# SPDX-License-Identifier: BSD-2-Clause

set -eu
here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d -t network-lib) && [ -d "$tmp" ] || exit 1
trap 'rm -rf "$tmp"' EXIT

failures=0
check() {
	if eval "$2"; then
		echo "ok    $1"
	else
		echo "FAIL  $1"
		sed 's/^/      | /' "$tmp/out" 2>/dev/null || true
		failures=$((failures + 1))
	fi
}

state=$tmp/state
mkdir -p "$tmp/bin" "$tmp/vm/tests/vm"
: >"$tmp/all-commands"

# Stand-ins. valectl lists the virtio-net ports bhyve attached while that
# bhyve runs.
cat >"$tmp/bin/bhyve" <<'EOF'
#!/bin/sh
s=$LIBTEST_STATE
echo "bhyve $*" >>"$s/commands"
rm -f "$s/destroyed"
ports=
for a; do
	case $a in
	*,virtio-net,*)
		p=${a#*,virtio-net,}
		ports="$ports ${p%%,*}"
		;;
	esac
	vm=$a
done
[ "${BHYVE_VM:-yes}" = no ] || : >"$VMM_DIR/$vm"
attach() {
	printf '%s\n' $ports >>"$s/bhyve.ports"
	[ -z "${BHYVE_EXTRA:-}" ] || echo "$BHYVE_EXTRA" >>"$s/bhyve.ports"
}
: >"$s/bhyve.ports"
[ "${BHYVE_LATE:-0}" -gt 0 ] || attach
echo $$ >"$s/bhyve.pid"
[ -z "${BHYVE_HOST:-}" ] || echo "$BHYVE_HOST" >>"$s/ifconfig"
[ -z "${BHYVE_ROUTE:-}" ] || echo "$BHYVE_ROUTE" >>"$s/routes"
if [ -n "${BHYVE_ROUTES:-}" ]; then
	sed "$BHYVE_ROUTES" "$s/routes" >"$s/routes.new"
	mv "$s/routes.new" "$s/routes"
fi
[ -z "${BHYVE_VALECTL:-}" ] || echo "$BHYVE_VALECTL" >"$s/valectl"
ended() {
	[ "${BHYVE_KEEP:-no}" = no ] || cat "$s/bhyve.ports" >>"$s/vale"
	if [ -n "${BHYVE_FOREIGN:-}" ]; then
		grep -v -x -F "$BHYVE_FOREIGN" "$s/vale" >"$s/vale.new" || true
		mv "$s/vale.new" "$s/vale"
	fi
	exit "$1"
}
i=0
while [ "$i" -lt "${BHYVE_RUN:-5}" ]; do
	[ "$i" -ne "${BHYVE_LATE:-0}" ] || [ "$i" -eq 0 ] || attach
	if [ "$i" -eq "${BHYVE_DROP:--1}" ]; then
		sed '$d' "$s/bhyve.ports" >"$s/bhyve.ports.new"
		mv "$s/bhyve.ports.new" "$s/bhyve.ports"
	fi
	[ ! -e "$s/destroyed" ] || [ "${BHYVE_STUCK:-no}" = yes ] || ended 4
	sleep 0.1
	i=$((i + 1))
done
ended "${BHYVE_STATUS:-1}"
EOF
cat >"$tmp/bin/bhyvectl" <<'EOF'
#!/bin/sh
s=$LIBTEST_STATE
echo "bhyvectl $*" >>"$s/commands"
[ "$1" = --destroy ] || exit 0
vm=${2#--vm=}
if [ ! -e "$VMM_DIR/$vm" ] || [ "${BHYVECTL_FAILS:-no}" = yes ]; then
	echo "vm_open: $vm could not be opened: No such file or directory" >&2
	exit 1
fi
rm -f "$VMM_DIR/$vm"
: >"$s/destroyed"
EOF
cat >"$tmp/bin/valectl" <<'EOF'
#!/bin/sh
s=$LIBTEST_STATE
echo "valectl $*" >>"$s/commands"
# Every port, a switch's ports on its own bridge, numbered on it.
ports() {
	cat "$s/vale"
	if [ -s "$s/bhyve.pid" ] && kill -0 "$(cat "$s/bhyve.pid")" 2>/dev/null; then
		cat "$s/bhyve.ports"
	fi
}
listing() {
	ports | awk '{
		sw = $0
		sub(/:.*/, "", sw)
		if (!(sw in bridge)) { bridge[sw] = bridges++; used[sw] = 0 }
		print $0 " bridge_idx " bridge[sw] " port_idx " used[sw]++
	}'
}
case $(cat "$s/valectl" 2>/dev/null || echo ok) in
ok) listing ;;
samebridge) listing | awk '$1 ~ /^valeabc123[ab]:/ { $3 = 9; $5 = NR + 10 } { print }' ;;
splitbridge) listing | awk 'NR == 1 { $3 = 7 } { print }' ;;
blank)
	listing
	echo
	;;
duplicate)
	listing
	listing | head -n 1
	;;
dupname)
	listing
	listing | head -n 1 | awk '{ $NF = 99; print }'
	;;
error)
	listing | head -n 1
	echo "failed to list all: Operation not permitted" >&2
	;;
denied) echo "/dev/netmap: Permission denied" >&2 ;;
zero)
	listing
	exit 0
	;;
odd)
	listing
	echo "valeother: something else"
	;;
esac
exit 1
EOF
cat >"$tmp/bin/ifconfig" <<'EOF'
#!/bin/sh
echo "ifconfig $*" >>"$LIBTEST_STATE/commands"
cat "$LIBTEST_STATE/ifconfig"
EOF
cat >"$tmp/bin/netstat" <<'EOF'
#!/bin/sh
echo "netstat $*" >>"$LIBTEST_STATE/commands"
[ "${NETSTAT_FAILS:-no}" = no ] || exit 1
cat "$LIBTEST_STATE/routes"
EOF
for c in kldload pfctl sysctl route; do
	printf '#!/bin/sh\necho "%s $*" >>"$LIBTEST_STATE/commands"\n' "$c" >"$tmp/bin/$c"
done
echo '[ "${KLDLOAD_FAILS:-no}" = no ]' >>"$tmp/bin/kldload"
printf '#!/bin/sh\necho "fetch: transfer failed" >&2\nexit 1\n' >"$tmp/bin/fetch"
chmod +x "$tmp/bin"/*

# A suite of BOOTS boots (one unless set), as the bhyve tests use the library.
ln -s "$here/lib.sh" "$tmp/vm/tests/vm/lib.sh"
cat >"$tmp/vm/tests/vm/run.sh" <<'EOF'
. "$(dirname "$0")/lib.sh"
mkdir -p "$WORK"
echo $$ >"$WORK/run.pid"
vm_begin "$WORK/vm/${DIRNAME:-lib-test}"
[ -z "${PREP:-}" ] || sleep "$PREP"
boot=1
while [ "$boot" -le "${BOOTS:-1}" ]; do
	st=0
	run_vm "${VMNAME:-lib-test}" "$dir/boot$boot.log" "${LIMIT:-30}" -s 2,virtio-blk,"$dir/disk.img" $(net_slots) ||
		st=$?
	echo "run_vm returned $st"
	[ "$st" -eq 1 ] || fail "the guest ended with status $st"
	boot=$((boot + 1))
done
vm_end
echo "suite finished"
EOF

# The host as the stand-ins show it: another program's VALE port, one card
# and a default route.
reset() {
	rm -rf "$state" "$tmp/vmm" "$tmp/work"
	mkdir -p "$state" "$tmp/vmm"
	printf '%s\n' valeother:p0 valeother:p1 >"$state/vale"
	echo 'em0: flags=8843<UP,BROADCAST,RUNNING,SIMPLEX,MULTICAST> metric 0 mtu 1500' >"$state/ifconfig"
	cat >"$state/routes" <<'EOF'
Routing tables

Internet:
Destination        Gateway            Flags         Netif Expire
default            10.0.0.1           UGS             em0
10.0.0.0/24        link#1             U               em0

Internet6:
Destination                       Gateway                       Flags         Netif Expire
default                           fe80::1%em0                   UG              em0   1790
::1                               link#2                        UHS             lo0
EOF
	: >"$state/commands"
}

# The stand-ins' environment for a suite; the consoles' path is one that
# doesn't exist.
stand_ins="PATH=$tmp/bin:$PATH WORK=$tmp/work VM_LOCK=$tmp/lock VMM_DIR=$tmp/vmm NMDM=$tmp/nmdm/
	LIBTEST_STATE=$state VM_POLL=0.1 VM_STOP_WAIT=1 VM_CLOSE_WAIT=1 RUN_ID=abc123"

# suite [variable=value...]: run.sh with the stand-ins and the assignments;
# its output in $tmp/out, its status in $st.
suite() {
	st=0
	env $stand_ins "$@" sh "$tmp/vm/tests/vm/run.sh" >"$tmp/out" 2>&1 || st=$?
	cat "$state/commands" >>"$tmp/all-commands"
}

# suite_bg [variable=value...]: suite in the background, its pid in $run.
suite_bg() {
	env $stand_ins "$@" sh "$tmp/vm/tests/vm/run.sh" >"$tmp/out" 2>&1 &
	run=$!
}

# interrupted: the background suite's status in $st once it has ended.
interrupted() {
	st=0
	wait "$run" || st=$?
	cat "$state/commands" >>"$tmp/all-commands"
}

# appears <file>: the file there and not empty, within 10 s.
appears() {
	i=0
	while [ ! -s "$1" ] && [ $i -lt 100 ]; do
		sleep 0.1
		i=$((i + 1))
	done
	[ -s "$1" ]
}

said() {
	grep -q -F -e "$1" "$tmp/out"
}
ran() {
	grep -q "^$1 " "$state/commands"
}
# Every bhyvectl call destroys this run's VM, and there's at least one.
destroyed_ours() {
	ran bhyvectl && [ -z "$(grep '^bhyvectl ' "$state/commands" | grep -v -x -F 'bhyvectl --destroy --vm=hnet-lib-test-abc123')" ]
}
vm_gone() {
	[ ! -e "$tmp/vmm/hnet-lib-test-abc123" ]
}
bhyve_gone() {
	! kill -0 "$(cat "$state/bhyve.pid")" 2>/dev/null
}
# No port on this run's switches, and the other program's port still there.
ports_as_before() {
	_now=$(env LIBTEST_STATE="$state" "$tmp/bin/valectl" | cut -d ' ' -f 1 || true)
	[ "$_now" = "$(printf '%s\n' valeother:p0 valeother:p1)" ]
}
lock_free() {
	lockf -s -t 0 "$tmp/lock" true
}
# The lock free within 2 s, once a killed stand-in's own children have ended.
lock_free_soon() {
	lockf -s -t 2 "$tmp/lock" true
}

reset
suite
check "a guest that powers off: run_vm returns 1 and the suite finishes" \
	"[ $st -eq 0 ] && said 'run_vm returned 1' && said 'suite finished'"
check "bhyve gets the four cards, vtnet0 and vtnet2 on switch A, vtnet1 and vtnet3 on B" \
	"grep -q -F -e '-s 5,virtio-net,valeabc123a:v0 -s 6,virtio-net,valeabc123b:v1 -s 7,virtio-net,valeabc123a:v2 -s 8,virtio-net,valeabc123b:v3 ' '$state/commands'"
check "as the VM hnet-lib-test-abc123, the run's own name" "grep -q '^bhyve .* hnet-lib-test-abc123\$' '$state/commands'"
check "which is destroyed afterwards, and nothing else is" "destroyed_ours && vm_gone"
check "valectl looked while the guest ran" "[ \$(grep -c '^valectl ' '$state/commands') -ge 4 ]"
check "no port of the run's is left, the other program's is as it was" ports_as_before
check "the lock is free afterwards" lock_free

reset
: >"$state/vale"
suite
check "with no VALE port on the host (valectl's empty listing) the run works" \
	"[ $st -eq 0 ] && said 'suite finished'"

reset
suite RUN_ID=abcdefg
check "a run name of 7 characters makes ports of 15 characters, bhyve's longest" \
	"[ $st -eq 0 ] && grep -q -F -e '-s 5,virtio-net,valeabcdefga:v0 ' '$state/commands'"
reset
suite RUN_ID=abcdefgh
check "one of 8 is refused before anything runs" \
	"[ $st -ne 0 ] && said 'valeabcdefgha:v0' && ! ran bhyve && ! ran valectl"
reset
suite RUN_ID=ab:c
check "and so is one that isn't letters and digits" "[ $st -ne 0 ] && ! ran bhyve && ! ran valectl"
reset
suite RUN_ID=
first=$(sed -n 's/.*-s 5,virtio-net,\(vale[0-9a-f]*a\):v0 .*/\1/p' "$state/commands")
reset
suite RUN_ID=
second=$(sed -n 's/.*-s 5,virtio-net,\(vale[0-9a-f]*a\):v0 .*/\1/p' "$state/commands")
check "without RUN_ID each run's switches have names of their own" \
	"[ $st -eq 0 ] && echo '$first' | grep -q -x 'vale[0-9a-f]\{6\}a' && [ '$first' != '$second' ]"

reset
echo valeabc123a:v9 >>"$state/vale"
suite
check "a port already on the run's switch A: refused, naming it, before any VM" \
	"[ $st -ne 0 ] && said 'valeabc123a:v9' && ! ran bhyve && ! ran bhyvectl"
check "and left alone" "grep -q -x valeabc123a:v9 '$state/vale'"
reset
echo valeabc123b:em0 >>"$state/vale"
suite
check "on switch B, the same" "[ $st -ne 0 ] && said 'valeabc123b:em0' && ! ran bhyve && ! ran bhyvectl"

for mode in error denied zero odd blank duplicate dupname splitbridge; do
	reset
	echo "$mode" >"$state/valectl"
	suite
	check "a valectl listing that fails or doesn't add up ($mode) refuses the run before any VM" \
		"[ $st -ne 0 ] && said \"can't list the VALE ports\" && ! ran bhyve && ! ran bhyvectl"
done
reset
suite BHYVE_VALECTL=samebridge BHYVE_RUN=50
check "this run's four ports on one bridge while the guest runs: the run fails" \
	"[ $st -ne 0 ] && ! said 'suite finished' && bhyve_gone && destroyed_ours"

reset
: >"$tmp/vmm/hnet-lib-test-old999"
suite
check "another run's VM hnet-lib-test-old999: named, left alone, and this run goes on" \
	"[ $st -eq 0 ] && said hnet-lib-test-old999 && [ -e '$tmp/vmm/hnet-lib-test-old999' ] && destroyed_ours"
reset
suite VMNAME=a-name-much-too-long-for-vmm
check "a VM name longer than vmm's 31 characters is refused before bhyve" "[ $st -ne 0 ] && ! ran bhyve"

reset
suite BHYVE_LATE=3 BHYVE_RUN=10 VM_OPEN_WAIT=2
check "ports that open a moment after bhyve starts are fine" "[ $st -eq 0 ] && said 'suite finished'"
reset
suite BHYVE_LATE=40 BHYVE_RUN=300 VM_OPEN_WAIT=1 LIMIT=8
check "ports that don't all open within VM_OPEN_WAIT: the run fails and stops the guest" \
	"[ $st -ne 0 ] && said \"didn't all open within 1s\" && bhyve_gone && destroyed_ours"
reset
suite BHYVE_DROP=5 BHYVE_RUN=300
check "one of the run's ports gone while its guest runs on: the run fails, naming it" \
	"[ $st -ne 0 ] && said 'valeabc123b:v3' && bhyve_gone && destroyed_ours"
reset
suite BHYVE_DROP=8 BHYVE_RUN=10
check "ports going as the guest's bhyve ends are fine" "[ $st -eq 0 ] && said 'suite finished'"

reset
suite NETSTAT_FAILS=yes
check "the host's routes that can't be read refuse the run before any VM" \
	"[ $st -ne 0 ] && said routes && ! ran bhyve"

reset
mkdir -p "$tmp/work"
: >"$tmp/work/keep"
suite DIRNAME=..
check "a run's directory that isn't one under \$WORK/vm is refused and nothing removed" \
	"[ $st -ne 0 ] && [ -e '$tmp/work/keep' ] && ! ran valectl"

reset
: >"$tmp/vmm/hnet-lib-test-abc123"
suite
check "a VM hnet-lib-test-abc123 already there: refused, not started or destroyed" \
	"[ $st -ne 0 ] && said hnet-lib-test-abc123 && ! ran bhyve && ! ran bhyvectl && [ -e '$tmp/vmm/hnet-lib-test-abc123' ]"

reset
mkdir -p "$tmp/work/vm/lib-test"
: >"$tmp/work/vm/lib-test/first-run"
sh -c 'exec 9>>"$1" && lockf -s -t 0 9 && echo held >"$2" && exec sleep 30' sh "$tmp/lock" "$tmp/held" &
holder=$!
appears "$tmp/held" || true
suite
kill "$holder" 2>/dev/null || true
wait "$holder" 2>/dev/null || true
check "a second run while another holds the lock: refused" "[ $st -ne 0 ] && said '$tmp/lock'"
check "and runs nothing, not even valectl" "[ ! -s '$state/commands' ]"
check "and the first run's directory is left as it was" "[ -e '$tmp/work/vm/lib-test/first-run' ]"

reset
suite BHYVE_EXTRA=valeabc123a:v7
check "a port not the run's on its switch while the guest runs: the run fails, naming it" \
	"[ $st -ne 0 ] && said 'valeabc123a:v7'"
check "and its VM is stopped and destroyed" "bhyve_gone && destroyed_ours && vm_gone && lock_free"

reset
suite BHYVE_VALECTL=error BHYVE_RUN=50
check "a valectl listing that fails while the guest runs: the run fails" \
	"[ $st -ne 0 ] && ! said 'suite finished'"
check "and its VM is stopped and destroyed" "bhyve_gone && destroyed_ours && vm_gone"

reset
suite BHYVE_KEEP=yes
check "the run's ports still there after its bhyve ended: the run fails, naming them" \
	"[ $st -ne 0 ] && said 'valeabc123a:v0' && ! said 'suite finished'"
check "once" "[ \$(grep -c '^FAIL: .*valeabc123a:v0' '$tmp/out') -eq 1 ]"

reset
suite BOOTS=2
check "two boots: each its own bhyve, each VM destroyed after it" \
	"[ $st -eq 0 ] && [ \$(grep -c '^bhyve ' '$state/commands') -eq 2 ] && [ \$(grep -c '^bhyvectl ' '$state/commands') -eq 2 ] && destroyed_ours"
reset
suite BOOTS=2 BHYVE_KEEP=yes
check "the run's ports still there after the first boot: the run stops before the second" \
	"[ $st -ne 0 ] && said 'after hnet-lib-test-abc123' && [ \$(grep -c '^bhyve ' '$state/commands') -eq 1 ]"

reset
suite BHYVE_FOREIGN=valeother:p0
check "another program's port gone during the run: the run fails, naming it" \
	"[ $st -ne 0 ] && said 'valeother:p0' && ! said 'suite finished'"

reset
suite BHYVE_HOST='tap9: flags=8802<BROADCAST,SIMPLEX,MULTICAST> metric 0 mtu 1500'
check "a host interface that changed during the run: the run fails, showing it" \
	"[ $st -ne 0 ] && said 'tap9' && ! said 'suite finished'"
check "once" "[ \$(grep -c '^FAIL: after the run' '$tmp/out') -eq 1 ]"
reset
suite BHYVE_ROUTE='10.77.0.0/24       link#9             U          tap9'
check "a host route that changed during the run: the run fails, showing it" \
	"[ $st -ne 0 ] && said '10.77.0.0/24' && ! said 'suite finished'"
reset
suite BHYVE_ROUTES='s/ 1790$/ 1700/'
check "a route's Expire countdown alone isn't a change" "[ $st -eq 0 ] && said 'suite finished'"
reset
suite BHYVE_ROUTES='s/^default            10.0.0.1 /default            10.0.0.2 /'
check "a default route through another gateway is" "[ $st -ne 0 ] && said '10.0.0.2' && ! said 'suite finished'"
reset
suite BHYVE_ROUTES='/^::1 /d'
check "and so is a route gone" "[ $st -ne 0 ] && said '::1' && ! said 'suite finished'"
reset
suite BHYVE_ROUTES='s/ 1790$//'
check "and so is a route's expiry gone" "[ $st -ne 0 ] && ! said 'suite finished'"

reset
suite BHYVE_STATUS=4 BHYVE_RUN=0
check "bhyve failing: run_vm returns its status" "[ $st -ne 0 ] && said 'run_vm returned 4'"
check "and the VM it made is destroyed, nothing else, other ports as they were" \
	"destroyed_ours && vm_gone && lock_free && ports_as_before"
reset
suite BHYVE_STATUS=4 BHYVE_RUN=0 BHYVE_VM=no
check "bhyve failing before it made a VM: nothing is destroyed" \
	"[ $st -ne 0 ] && said 'run_vm returned 4' && ! ran bhyvectl"

reset
suite BHYVECTL_FAILS=yes
check "the run's VM that can't be destroyed after its guest: the run fails, naming it" \
	"[ $st -ne 0 ] && said \"couldn't destroy hnet-lib-test-abc123\" && ! said 'suite finished'"
check "once" "[ \$(grep -c \"couldn't destroy\" '$tmp/out') -eq 1 ]"
reset
suite BHYVECTL_FAILS=yes BOOTS=2
check "and stops before the next boot" "[ $st -ne 0 ] && [ \$(grep -c '^bhyve ' '$state/commands') -eq 1 ]"

reset
suite BHYVE_RUN=300 LIMIT=1
check "a guest past its time limit: run_vm stops it and returns 124" \
	"said 'run_vm returned 124' && bhyve_gone && destroyed_ours && vm_gone && ports_as_before"
reset
suite BHYVE_RUN=300 VM_STALL=1
check "a guest whose console is silent: run_vm stops it and returns 125" \
	"said 'run_vm returned 125' && bhyve_gone && destroyed_ours && vm_gone && ports_as_before"
reset
suite KLDLOAD_FAILS=yes
check "vmm or nmdm that won't load: refused before bhyve" "[ $st -ne 0 ] && said 'vmm' && ! ran bhyve"

# sigint: the suite in the foreground, sent SIGINT once its guest runs; its
# status in $st.
sigint() {
	(
		appears "$state/bhyve.pid" && sleep 0.5 && kill -INT "$(cat "$tmp/work/run.pid")"
	) &
	killer=$!
	suite BHYVE_RUN=300
	wait "$killer" || true
}

for sig in TERM:143 HUP:129 INT:130; do
	want=${sig#*:}
	sig=${sig%:*}
	reset
	if [ "$(sh -c "kill -$sig \$\$ && echo ignored" 2>/dev/null)" = ignored ]; then
		echo "skip  SIG$sig: this shell ignores it"
		continue
	fi
	if [ "$sig" = INT ]; then
		sigint
	else
		suite_bg BHYVE_RUN=300
		appears "$state/bhyve.pid" || true
		sleep 0.5
		kill -"$sig" "$run" 2>/dev/null || true
		interrupted
	fi
	check "SIG$sig while the guest runs: the run ends with status $want, saying so" \
		"[ $st -eq $want ] && said 'Interrupted' && ! said 'suite finished'"
	check "after stopping and destroying its VM" "bhyve_gone && destroyed_ours && vm_gone"
	check "with no port of its left and the other program's as it was" ports_as_before
	check "and the lock free" lock_free
done

reset
suite_bg BHYVE_RUN=300 BHYVE_STUCK=yes
appears "$state/bhyve.pid" || true
sleep 0.5
kill -TERM "$run" 2>/dev/null || true
interrupted
check "a bhyve that doesn't end when its VM is destroyed is killed" \
	"[ $st -ne 0 ] && bhyve_gone && destroyed_ours && vm_gone && ports_as_before && lock_free_soon"

# A run killed outright, its console's reader left waiting to open the
# console as it waits for nmdm's other side (a FIFO with no writer here).
reset
mkdir -p "$tmp/fifo"
mkfifo "$tmp/fifo/hnet-lib-test-abc123B"
suite_bg BHYVE_RUN=300 NMDM="$tmp/fifo/"
appears "$state/bhyve.pid" || true
sleep 0.5
kill -KILL "$run" 2>/dev/null || true
interrupted
orphan=$(cat "$state/bhyve.pid")
st=0
env $stand_ins WORK="$tmp/work2" sh "$tmp/vm/tests/vm/run.sh" >"$tmp/out" 2>&1 || st=$?
check "a run killed outright leaves its bhyve, which keeps the lock: another run, other WORK, is refused" \
	"[ $st -ne 0 ] && said '$tmp/lock' && [ ! -e '$tmp/work2/vm' ]"
kill -KILL "$orphan" 2>/dev/null || true
check "and once that bhyve ends, the lock is free, though the console's reader still waits" lock_free_soon
timeout 2 sh -c ': >"$1"' sh "$tmp/fifo/hnet-lib-test-abc123B" || true
rm -f "$tmp/vmm"/*

reset
suite_bg PREP=2
sleep 0.5
kill -TERM "$run" 2>/dev/null || true
interrupted
check "SIGTERM before the guest starts: the run ends with 143 and starts no VM" \
	"[ $st -eq 143 ] && said 'Interrupted' && ! said 'suite finished' && ! ran bhyve && ! ran bhyvectl && lock_free"

check "lib.sh never attaches or detaches a VALE port: valectl only lists" \
	"[ -z \"\$(grep '^valectl ' '$tmp/all-commands' | grep -v -x 'valectl ')\" ] && ! grep -v '^[[:space:]]*#' '$here/lib.sh' | grep -q -E 'valectl[[:space:]]+-'"
check "and destroys no VM but hnet-lib-test-<run>" \
	"[ -z \"\$(grep '^bhyvectl ' '$tmp/all-commands' | grep -v -x -E 'bhyvectl --destroy --vm=hnet-lib-test-[0-9a-z]+')\" ]"
check "and its guests' consoles are the test's, never /dev/nmdm" \
	"! grep '^bhyve ' '$tmp/all-commands' | grep -q -v -F -e '-l com1,$tmp/nmdm/hnet-' -e '-l com1,$tmp/fifo/hnet-'"
check "and otherwise only loads vmm and nmdm and reads the host's interfaces and routes: no pfctl, sysctl or route" \
	"[ -z \"\$(grep -E '^(ifconfig|netstat|kldload|pfctl|sysctl|route) ' '$tmp/all-commands' | grep -v -x -e 'ifconfig -a' -e 'netstat -rn' -e 'kldload -n vmm nmdm')\" ]"

# guest_ran <log> <tag> <check...>, on serial logs.
guest_ran_on() {
	(. "$tmp/vm/tests/vm/lib.sh" && guest_ran "$tmp/serial.txt" cards "$@") >"$tmp/out" 2>&1
}
printf '@@check ok first\n@@check ok second\n@@cards done status=0\n' >"$tmp/serial.txt"
check "guest_ran: every named check passed and the checks ended with 0" "guest_ran_on first second"
check "a named check the guest never passed fails, naming it" "! guest_ran_on first second third && said third"
printf '@@check ok first one\n@@cards done status=0\n' >"$tmp/serial.txt"
check "a check passes only by its whole name" "! guest_ran_on first"
printf '@@check ok first\n@@check FAIL second: why\n@@cards done status=0\n' >"$tmp/serial.txt"
check "a check that failed fails it, even one not named" "! guest_ran_on first && said 'second: why'"
printf '@@check ok first\n' >"$tmp/serial.txt"
check "checks that never ended fail it" "! guest_ran_on first"
printf '@@check ok first\n@@cards done status=1\n' >"$tmp/serial.txt"
check "checks that ended with another status fail it" "! guest_ran_on first"

# guest_shots <log> <dir> <name...>, on a log and a directory of screenshots.
mkdir -p "$tmp/shots"
printf 'one' >"$tmp/shots/a.png"
printf 'two' >"$tmp/shots/b.png"
printf '@@shot a.png 3 %s\n@@shot b.png 3 %s\n' "$(sha256 -q "$tmp/shots/a.png")" "$(sha256 -q "$tmp/shots/b.png")" \
	>"$tmp/serial.txt"
guest_shots_on() {
	(. "$tmp/vm/tests/vm/lib.sh" && guest_shots "$tmp/serial.txt" "$tmp/shots" "$@") >"$tmp/out" 2>&1
}
check "guest_shots: every screenshot there, with the size and SHA-256 the guest gave" "guest_shots_on a b"
check "a screenshot missing fails, naming it" "! guest_shots_on a b c && said c.png"
check "one more than named fails" "! guest_shots_on a"
printf 'owt' >"$tmp/shots/b.png"
check "one whose bytes aren't the guest's fails" "! guest_shots_on a b && said b.png"

# cloud_image with downloads that fail.
cloud_image_fails() {
	(env PATH="$tmp/bin:$PATH" WORK="$tmp/images-work" sh -c '. "$1" && cloud_image' sh "$tmp/vm/tests/vm/lib.sh") \
		>"$tmp/out" 2>&1
}
check "cloud_image fails when the image can't be downloaded" "! cloud_image_fails && said 'FAIL'"

[ "$failures" -eq 0 ] || {
	echo "lib-test: $failures failed" >&2
	exit 1
}
echo "lib: ok"
