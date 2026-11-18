#!/bin/sh
#
# Tests of hyprbsd-network-helper on scratch roots (-r): wifi-setup's rc.conf
# lines and wpa_supplicant.conf, wifi on|off, iface's rc.conf lines and DNS
# hooks, iface-switch, lagg, what each leaves alone and what each refuses;
# then iface-switch, iface, wifi off, card-gone, card-back and lagg on the
# running system against stand-ins.
# As root, the stand-ins and a failed write are skipped.
#
#   tests/helper-test.sh
#
# SPDX-License-Identifier: BSD-2-Clause

set -eu
top=$(cd "$(dirname "$0")/.." && pwd)
helper=$top/libexec/hyprbsd-network-helper
tmp=$(mktemp -d -t network-helper)
trap 'chmod -R u+w "$tmp"; rm -rf "$tmp"' EXIT
failures=0

fail() {
	echo "helper-test: $*" >&2
	failures=$((failures + 1))
}

# A scratch root named $1 with rc.conf holding $2.
root() {
	mkdir -p "$tmp/$1/etc"
	printf '%s' "$2" >"$tmp/$1/etc/rc.conf"
	echo "$tmp/$1"
}

# Runs wifi-setup on root $1 with adapters $2; its output in $1.out, its
# status in $st.
setup() {
	st=0
	HYPRBSD_NETWORK_WLAN_DEVICES=$2 "$helper" -r "$1" wifi-setup >"$1.out" 2>&1 || st=$?
}

rc_value() {
	sysrc -f "$1/etc/rc.conf" -n "$2" 2>/dev/null || true
}

mode() {
	stat -f %Lp "$1"
}

header='ctrl_interface=DIR=/var/run/wpa_supplicant GROUP=wheel
update_config=1
bss_expiration_age=3600'

# No adapter: nothing written.
r=$(root none 'ifconfig_DEFAULT="DHCP"
')
setup "$r" ""
[ "$st" -eq 0 ] || fail "no adapter: status $st"
[ ! -e "$r/etc/wpa_supplicant.conf" ] || fail "no adapter: wrote wpa_supplicant.conf"
[ "$(cat "$r/etc/rc.conf")" = 'ifconfig_DEFAULT="DHCP"' ] || fail "no adapter: changed rc.conf"

# A wired install: wlan0 with wpa_supplicant and DHCP, a private umask, and
# a new wpa_supplicant.conf holding only the settings.
r=$(root fresh 'ifconfig_DEFAULT="DHCP"
')
setup "$r" iwlwifi0
[ "$st" -eq 0 ] || fail "fresh: status $st: $(cat "$r.out")"
[ "$(rc_value "$r" wlans_iwlwifi0)" = wlan0 ] || fail "fresh: wlans_iwlwifi0 is '$(rc_value "$r" wlans_iwlwifi0)'"
[ "$(rc_value "$r" ifconfig_wlan0)" = "WPA DHCP" ] || fail "fresh: ifconfig_wlan0 is '$(rc_value "$r" ifconfig_wlan0)'"
[ "$(rc_value "$r" wpa_supplicant_umask)" = 077 ] || fail "fresh: wpa_supplicant_umask is '$(rc_value "$r" wpa_supplicant_umask)'"
[ "$(cat "$r/etc/wpa_supplicant.conf")" = "$header" ] || fail "fresh: wpa_supplicant.conf is:
$(cat "$r/etc/wpa_supplicant.conf")"
[ "$(mode "$r/etc/wpa_supplicant.conf")" = 600 ] || fail "fresh: wpa_supplicant.conf is mode $(mode "$r/etc/wpa_supplicant.conf")"

# The first adapter of several.
r=$(root two '')
setup "$r" "rtw880 iwlwifi0"
[ "$(rc_value "$r" wlans_rtw880)" = wlan0 ] || fail "two adapters: wlans_rtw880 is '$(rc_value "$r" wlans_rtw880)'"
[ -z "$(rc_value "$r" wlans_iwlwifi0)" ] || fail "two adapters: also set wlans_iwlwifi0"

# A name another adapter's line already takes isn't reused.
r=$(root taken 'wlans_ath0="wlan0"
')
setup "$r" iwlwifi0
[ "$(rc_value "$r" wlans_iwlwifi0)" = wlan1 ] || fail "wlan0 taken: wlans_iwlwifi0 is '$(rc_value "$r" wlans_iwlwifi0)'"
[ "$(rc_value "$r" ifconfig_wlan1)" = "WPA DHCP" ] || fail "wlan0 taken: ifconfig_wlan1 is '$(rc_value "$r" ifconfig_wlan1)'"

# A file another program wrote, with other values for the settings: each
# replaced where it is, the networks and other settings kept, the mode made
# private.
r=$(root saved 'wlans_iwlwifi0="wlan0"
ifconfig_wlan0="WPA DHCP"
')
cat >"$r/etc/wpa_supplicant.conf" <<'EOF'
ctrl_interface=/var/run/wpa_supplicant
ctrl_interface_group=wheel
update_config=0
country=GB
ap_scan=1
bss_expiration_age=180
network={
	ssid="Home {1}"
	psk=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
}
network={
	ssid="a}b"
	key_mgmt=NONE
	priority=5
}
p2p_disabled=1
EOF
chmod 0644 "$r/etc/wpa_supplicant.conf"
setup "$r" iwlwifi0
[ "$st" -eq 0 ] || fail "saved: status $st: $(cat "$r.out")"
expected="ctrl_interface=DIR=/var/run/wpa_supplicant GROUP=wheel
update_config=1
country=GB
ap_scan=1
bss_expiration_age=3600
network={
	ssid=\"Home {1}\"
	psk=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
}
network={
	ssid=\"a}b\"
	key_mgmt=NONE
	priority=5
}
p2p_disabled=1"
[ "$(cat "$r/etc/wpa_supplicant.conf")" = "$expected" ] || fail "saved: wpa_supplicant.conf is:
$(cat "$r/etc/wpa_supplicant.conf")"
[ "$(mode "$r/etc/wpa_supplicant.conf")" = 600 ] || fail "saved: wpa_supplicant.conf is mode $(mode "$r/etc/wpa_supplicant.conf")"

# Comments as wpa_supplicant reads them (# outside quotes to the end of the
# line): a block closed by "} # comment", a commented-out "network={", and a
# # inside a quoted name. The older settings after each are still replaced, and
# the one missing is added first.
r=$(root comments 'wlans_iwlwifi0="wlan0"
ifconfig_wlan0="WPA DHCP"
')
cat >"$r/etc/wpa_supplicant.conf" <<'EOF'
network={
	ssid="Cafe #2 {free}" # the one downstairs
	key_mgmt=NONE
} # home network
ctrl_interface=/var/run/old-wpa
# Example network={
update_config=0
	# ctrl_interface_group=operator
EOF
setup "$r" iwlwifi0
[ "$st" -eq 0 ] || fail "comments: status $st: $(cat "$r.out")"
expected="bss_expiration_age=3600
network={
	ssid=\"Cafe #2 {free}\" # the one downstairs
	key_mgmt=NONE
} # home network
ctrl_interface=DIR=/var/run/wpa_supplicant GROUP=wheel
# Example network={
update_config=1
	# ctrl_interface_group=operator"
[ "$(cat "$r/etc/wpa_supplicant.conf")" = "$expected" ] || fail "comments: wpa_supplicant.conf is:
$(cat "$r/etc/wpa_supplicant.conf")"

# A second run changes no bytes.
cp "$r/etc/rc.conf" "$tmp/saved-rc.conf"
cp "$r/etc/wpa_supplicant.conf" "$tmp/saved-wpa.conf"
setup "$r" iwlwifi0
[ "$st" -eq 0 ] || fail "second run: status $st: $(cat "$r.out")"
cmp -s "$r/etc/rc.conf" "$tmp/saved-rc.conf" || fail "second run: changed rc.conf"
cmp -s "$r/etc/wpa_supplicant.conf" "$tmp/saved-wpa.conf" || fail "second run: changed wpa_supplicant.conf"

# A file wpa_supplicant saved, in its own order (country before
# bss_expiration_age), is already set up and keeps its bytes.
r=$(root wpa-saved 'wlans_iwlwifi0="wlan0"
ifconfig_wlan0="WPA DHCP"
wpa_supplicant_umask="077"
')
cat >"$r/etc/wpa_supplicant.conf" <<'EOF'
ctrl_interface=DIR=/var/run/wpa_supplicant GROUP=wheel
update_config=1
country=GB
bss_expiration_age=3600

network={
	ssid="Home"
	psk=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
}
EOF
chmod 0600 "$r/etc/wpa_supplicant.conf"
cp "$r/etc/wpa_supplicant.conf" "$tmp/wpa-saved.conf"
setup "$r" iwlwifi0
[ "$st" -eq 0 ] && grep -q "already set up" "$r.out" || fail "wpa_supplicant's own file: status $st, said: $(cat "$r.out")"
cmp -s "$r/etc/wpa_supplicant.conf" "$tmp/wpa-saved.conf" || fail "wpa_supplicant's own file: changed it to:
$(cat "$r/etc/wpa_supplicant.conf")"

# The owner's own interface (another name, a static address) is kept.
r=$(root own 'wlans_iwlwifi0="wlan1"
ifconfig_wlan1="WPA inet 192.168.1.5/24"
')
setup "$r" iwlwifi0
[ "$st" -eq 0 ] || fail "own: status $st: $(cat "$r.out")"
[ "$(rc_value "$r" ifconfig_wlan1)" = "WPA inet 192.168.1.5/24" ] || fail "own: ifconfig_wlan1 is '$(rc_value "$r" ifconfig_wlan1)'"
[ -z "$(rc_value "$r" ifconfig_wlan0)" ] || fail "own: added ifconfig_wlan0"

# rc takes WPA in any case, as a word of its own.
r=$(root lower 'wlans_iwlwifi0="wlan0"
ifconfig_wlan0="wpa DHCP"
')
setup "$r" iwlwifi0
[ "$st" -eq 0 ] || fail "lower-case wpa: status $st: $(cat "$r.out")"
[ "$(rc_value "$r" ifconfig_wlan0)" = "wpa DHCP" ] || fail "lower-case wpa: ifconfig_wlan0 is '$(rc_value "$r" ifconfig_wlan0)'"
r=$(root wpaword 'wlans_iwlwifi0="wlan0"
ifconfig_wlan0="inet 10.0.0.2/24 ssid WPAnet"
')
setup "$r" iwlwifi0
[ "$st" -ne 0 ] || fail "WPA inside a word: status 0"

# Wi-Fi set up without wpa_supplicant is refused and left as it is.
r=$(root nowpa 'wlans_iwlwifi0="wlan0"
ifconfig_wlan0="inet 10.0.0.2/24 ssid Cafe"
')
cp "$r/etc/rc.conf" "$tmp/nowpa-rc.conf"
setup "$r" iwlwifi0
[ "$st" -ne 0 ] || fail "no WPA: status 0"
grep -q ifconfig_wlan0 "$r.out" || fail "no WPA: the message doesn't name ifconfig_wlan0: $(cat "$r.out")"
cmp -s "$r/etc/rc.conf" "$tmp/nowpa-rc.conf" || fail "no WPA: changed rc.conf"
[ ! -e "$r/etc/wpa_supplicant.conf" ] || fail "no WPA: wrote wpa_supplicant.conf"

# A write that fails leaves the old file as it was (root writes in a
# read-only directory, so not as root).
if [ "$(id -u)" -ne 0 ]; then
	r=$(root readonly '')
	printf 'network={\n\tssid="Home"\n\tkey_mgmt=NONE\n}\n' >"$r/etc/wpa_supplicant.conf"
	cp "$r/etc/wpa_supplicant.conf" "$tmp/readonly-wpa.conf"
	chmod 0555 "$r/etc"
	setup "$r" iwlwifi0
	chmod 0755 "$r/etc"
	[ "$st" -ne 0 ] || fail "read-only: status 0"
	cmp -s "$r/etc/wpa_supplicant.conf" "$tmp/readonly-wpa.conf" || fail "read-only: changed wpa_supplicant.conf"
fi

# An adapter named on the command line, even one not yet in the list.
r=$(root named '')
st=0
HYPRBSD_NETWORK_WLAN_DEVICES=rtw880 "$helper" -r "$r" wifi-setup iwlwifi0 >"$r.out" 2>&1 || st=$?
[ "$st" -eq 0 ] || fail "named adapter: status $st: $(cat "$r.out")"
[ "$(rc_value "$r" wlans_iwlwifi0)" = wlan0 ] || fail "named adapter: wlans_iwlwifi0 is '$(rc_value "$r" wlans_iwlwifi0)'"
[ -z "$(rc_value "$r" wlans_rtw880)" ] || fail "named adapter: also set wlans_rtw880"

# auto runs wifi-setup -a $1 on root $2 with rc.conf $3, the adapters
# attached $4 (default $1).
auto() {
	r=$(root "$2" "$3")
	st=0
	HYPRBSD_NETWORK_WLAN_DEVICES=${4:-$1} "$helper" -r "$r" wifi-setup -a "$1" >"$r.out" 2>&1 || st=$?
}

# -a (devd's rule) sets up an adapter when no Wi-Fi is set up yet.
auto iwlwifi0 auto-first 'ifconfig_DEFAULT="DHCP"
'
[ "$st" -eq 0 ] || fail "-a, nothing set up: status $st: $(cat "$r.out")"
[ "$(rc_value "$r" wlans_iwlwifi0)" = wlan0 ] || fail "-a, nothing set up: wlans_iwlwifi0 is '$(rc_value "$r" wlans_iwlwifi0)'"

# -a leaves a second adapter alone while another has Wi-Fi.
second='wlans_iwlwifi0="wlan0"
ifconfig_wlan0="WPA DHCP"'
auto rtw880 auto-second "$second
" "iwlwifi0 rtw880"
[ "$st" -eq 0 ] || fail "-a, another adapter set up: status $st: $(cat "$r.out")"
[ "$(cat "$r/etc/rc.conf")" = "$second" ] || fail "-a, another adapter set up: changed rc.conf:
$(cat "$r/etc/rc.conf")"
[ ! -e "$r/etc/wpa_supplicant.conf" ] || fail "-a, another adapter set up: wrote wpa_supplicant.conf"

# -a moves the Wi-Fi line of an adapter that isn't attached to the one that
# is, keeping its wlan and settings.
auto iwlwifi0 auto-moved 'wlans_iwm0="wlan0"
ifconfig_wlan0="WPA DHCP NOAUTO"
'
[ "$st" -eq 0 ] || fail "-a, the line's adapter gone: status $st: $(cat "$r.out")"
[ "$(rc_value "$r" wlans_iwlwifi0)" = wlan0 ] || fail "-a, the line's adapter gone: wlans_iwlwifi0 is '$(rc_value "$r" wlans_iwlwifi0)'"
! grep -q '^wlans_iwm0=' "$r/etc/rc.conf" || fail "-a, the line's adapter gone: wlans_iwm0 kept"
[ "$(rc_value "$r" ifconfig_wlan0)" = "WPA DHCP NOAUTO" ] || fail "-a, the line's adapter gone: ifconfig_wlan0 is '$(rc_value "$r" ifconfig_wlan0)'"
grep -q iwm0 "$r.out" || fail "-a, the line's adapter gone: the message doesn't name iwm0: $(cat "$r.out")"

# -a refuses to move a gone adapter's wlan that doesn't use wpa_supplicant,
# and leaves rc.conf as it is.
nowpa_gone='wlans_iwm0="wlan0"
ifconfig_wlan0="inet 192.0.2.2/24 mode 11g"'
auto iwlwifi0 auto-moved-nowpa "$nowpa_gone
"
[ "$st" -ne 0 ] || fail "-a, a gone adapter's wlan without WPA: status 0"
[ "$(cat "$r/etc/rc.conf")" = "$nowpa_gone" ] || fail "-a, a gone adapter's wlan without WPA: changed rc.conf:
$(cat "$r/etc/rc.conf")"

# -a leaves a gone adapter's line alone while an attached one has Wi-Fi.
both='wlans_iwm0="wlan0"
wlans_rtw880="wlan1"'
auto iwlwifi0 auto-gone-and-set "$both
" "rtw880 iwlwifi0"
[ "$st" -eq 0 ] || fail "-a, a gone adapter and a set-up one: status $st: $(cat "$r.out")"
[ "$(cat "$r/etc/rc.conf")" = "$both" ] || fail "-a, a gone adapter and a set-up one: changed rc.conf:
$(cat "$r/etc/rc.conf")"

# -a sets up an adapter when another adapter's wlans_ line is empty.
auto iwlwifi0 auto-empty 'wlans_ath0=""
'
[ "$st" -eq 0 ] || fail "-a, an empty wlans_ line: status $st: $(cat "$r.out")"
[ "$(rc_value "$r" wlans_iwlwifi0)" = wlan0 ] || fail "-a, an empty wlans_ line: wlans_iwlwifi0 is '$(rc_value "$r" wlans_iwlwifi0)'"

# -a does nothing with hyprbsd_network_autosetup="NO", even for its own adapter.
auto iwlwifi0 auto-off 'hyprbsd_network_autosetup="NO"
wlans_iwlwifi0="wlan0"
ifconfig_wlan0="WPA DHCP"
'
[ "$st" -eq 0 ] || fail "-a, autosetup NO: status $st: $(cat "$r.out")"
[ ! -e "$r/etc/wpa_supplicant.conf" ] && [ -z "$(rc_value "$r" wpa_supplicant_umask)" ] ||
	fail "-a, autosetup NO: changed the settings"

# -a brings the settings of an adapter that has Wi-Fi up to date.
auto iwlwifi0 auto-own 'wlans_iwlwifi0="wlan0"
ifconfig_wlan0="WPA DHCP"
'
[ "$st" -eq 0 ] || fail "-a, own adapter: status $st: $(cat "$r.out")"
[ "$(rc_value "$r" wpa_supplicant_umask)" = 077 ] && grep -qx update_config=1 "$r/etc/wpa_supplicant.conf" ||
	fail "-a, own adapter: settings not brought up to date"

# The file rc gives wpa_supplicant (wpa_supplicant_conf_file).
r=$(root conffile 'wpa_supplicant_conf_file="/etc/wifi.conf"
')
setup "$r" iwlwifi0
[ "$st" -eq 0 ] || fail "wpa_supplicant_conf_file: status $st: $(cat "$r.out")"
grep -qx update_config=1 "$r/etc/wifi.conf" 2>/dev/null || fail "wpa_supplicant_conf_file: etc/wifi.conf not written"
[ ! -e "$r/etc/wpa_supplicant.conf" ] || fail "wpa_supplicant_conf_file: wrote etc/wpa_supplicant.conf"

# A name that isn't an adapter's is refused.
r=$(root badname '')
st=0
HYPRBSD_NETWORK_WLAN_DEVICES=iwlwifi0 "$helper" -r "$r" wifi-setup 'iwl;wifi0' >"$r.out" 2>&1 || st=$?
[ "$st" -ne 0 ] || fail "bad adapter name: status 0"
[ -z "$(cat "$r/etc/rc.conf")" ] || fail "bad adapter name: changed rc.conf"

# switch runs wifi $1 on root $2 with rc.conf $3.
switch() {
	r=$(root "$2" "$3")
	st=0
	"$helper" -r "$r" wifi "$1" >"$r.out" 2>&1 || st=$?
}

set_up='wlans_iwlwifi0="wlan0"
ifconfig_wlan0="WPA DHCP"
'

# Wi-Fi off keeps the wlan's settings and adds NOAUTO, so it stays off after a restart.
switch off off "$set_up"
[ "$st" -eq 0 ] || fail "off: status $st: $(cat "$r.out")"
[ "$(rc_value "$r" ifconfig_wlan0)" = "WPA DHCP NOAUTO" ] || fail "off: ifconfig_wlan0 is '$(rc_value "$r" ifconfig_wlan0)'"

# Off twice adds NOAUTO once.
st=0
"$helper" -r "$r" wifi off >"$r.out" 2>&1 || st=$?
[ "$st" -eq 0 ] && [ "$(rc_value "$r" ifconfig_wlan0)" = "WPA DHCP NOAUTO" ] ||
	fail "off twice: status $st, ifconfig_wlan0 is '$(rc_value "$r" ifconfig_wlan0)'"

# toggle turns Wi-Fi off while a wlan is on, and on when every one is off.
switch toggle toggle "$set_up"
[ "$st" -eq 0 ] && [ "$(rc_value "$r" ifconfig_wlan0)" = "WPA DHCP NOAUTO" ] && grep -qx 'Wi-Fi is off.' "$r.out" ||
	fail "toggle from on: status $st, ifconfig_wlan0 is '$(rc_value "$r" ifconfig_wlan0)', said: $(cat "$r.out")"
st=0
"$helper" -r "$r" wifi toggle >"$r.out" 2>&1 || st=$?
[ "$st" -eq 0 ] && [ "$(rc_value "$r" ifconfig_wlan0)" = "WPA DHCP" ] && grep -qx 'Wi-Fi is on.' "$r.out" ||
	fail "toggle from off: status $st, ifconfig_wlan0 is '$(rc_value "$r" ifconfig_wlan0)', said: $(cat "$r.out")"
switch toggle toggle-mixed 'wlans_iwlwifi0="wlan0"
wlans_rtw880="wlan1"
ifconfig_wlan0="WPA DHCP NOAUTO"
ifconfig_wlan1="WPA DHCP"
'
[ "$st" -eq 0 ] && [ "$(rc_value "$r" ifconfig_wlan0)" = "WPA DHCP NOAUTO" ] && [ "$(rc_value "$r" ifconfig_wlan1)" = "WPA DHCP NOAUTO" ] ||
	fail "toggle, one wlan on: status $st, ifconfig_wlan0 '$(rc_value "$r" ifconfig_wlan0)', ifconfig_wlan1 '$(rc_value "$r" ifconfig_wlan1)'"

# Wi-Fi on takes NOAUTO out, in any case and anywhere, and keeps the rest.
switch on on 'wlans_iwlwifi0="wlan0"
ifconfig_wlan0="WPA noauto DHCP"
'
[ "$st" -eq 0 ] || fail "on: status $st: $(cat "$r.out")"
[ "$(rc_value "$r" ifconfig_wlan0)" = "WPA DHCP" ] || fail "on: ifconfig_wlan0 is '$(rc_value "$r" ifconfig_wlan0)'"

# Every wlan with wpa_supplicant goes off; one without it is left alone.
switch off off-all 'wlans_iwlwifi0="wlan0"
ifconfig_wlan0="WPA DHCP"
wlans_rtw880="wlan1 wlan2"
ifconfig_wlan1="WPA SYNCDHCP"
ifconfig_wlan2="inet 10.0.0.2/24"
'
[ "$st" -eq 0 ] || fail "off, three wlans: status $st: $(cat "$r.out")"
[ "$(rc_value "$r" ifconfig_wlan0) | $(rc_value "$r" ifconfig_wlan1) | $(rc_value "$r" ifconfig_wlan2)" = \
	"WPA DHCP NOAUTO | WPA SYNCDHCP NOAUTO | inet 10.0.0.2/24" ] ||
	fail "off, three wlans: $(rc_value "$r" ifconfig_wlan0) | $(rc_value "$r" ifconfig_wlan1) | $(rc_value "$r" ifconfig_wlan2)"

# Without Wi-Fi set up, it's refused and nothing is written.
switch off unset 'ifconfig_DEFAULT="DHCP"
'
[ "$st" -ne 0 ] || fail "off, not set up: status 0"
[ "$(cat "$r/etc/rc.conf")" = 'ifconfig_DEFAULT="DHCP"' ] || fail "off, not set up: changed rc.conf"

# Anything but on or off is a usage error.
switch sideways sideways "$set_up"
[ "$st" -eq 2 ] || fail "wifi sideways: status $st"

# A root that doesn't exist.
st=0
HYPRBSD_NETWORK_WLAN_DEVICES=iwlwifi0 "$helper" -r "$tmp/missing" wifi-setup >/dev/null 2>&1 || st=$?
[ "$st" -ne 0 ] || fail "missing root: status 0"

# iface runs "iface $2..." on root $1; its output in $1.out, its status in $st.
iface() {
	_r=$1
	shift
	st=0
	"$helper" -r "$_r" "$@" >"$_r.out" 2>&1 || st=$?
}

# rc.conf on root $1 without the lines for the variables $2... .
rc_rest() {
	_f=$1/etc/rc.conf
	shift
	_pat=$(printf '%s|' "$@")
	grep -Ev "^(${_pat%|})=" "$_f" || true
}

dns_up='# hyprbsd-network: em0'\''s DNS servers, through resolvconf(8).
printf '\''nameserver %s\n'\'' 192.0.2.53 192.0.2.54 | /sbin/resolvconf -a em0:static'
dns_down='# hyprbsd-network: em0'\''s DNS servers, through resolvconf(8).
/sbin/resolvconf -f -d em0:static'

others='hostname="desk"
ifconfig_DEFAULT="DHCP"
ifconfig_em1="inet 10.9.0.2/16"
ifconfig_em0_alias0="inet 192.168.1.9/32"
ifconfig_em0_ipv6="inet6 accept_rtadv"
sshd_enable="YES"
'

# Automatic on a card that only had ifconfig_DEFAULT: its own DHCP line, the
# rest of rc.conf as it was.
r=$(root dhcp "$others")
iface "$r" iface em0 dhcp
[ "$st" -eq 0 ] || fail "dhcp: status $st: $(cat "$r.out")"
[ "$(rc_value "$r" ifconfig_em0)" = DHCP ] || fail "dhcp: ifconfig_em0 is '$(rc_value "$r" ifconfig_em0)'"
[ "$(rc_rest "$r" ifconfig_em0)" = "$(printf %s "$others")" ] || fail "dhcp: changed other lines:
$(cat "$r/etc/rc.conf")"

# Manual: the address, the gateway as defaultrouter, the DNS servers in the
# card's start and stop hooks (readable, for the window); nothing else.
r=$(root manual "$others")
iface "$r" iface em0 manual 192.168.1.5/24 192.168.1.1 192.0.2.53 192.0.2.54
[ "$st" -eq 0 ] || fail "manual: status $st: $(cat "$r.out")"
[ "$(rc_value "$r" ifconfig_em0)" = "inet 192.168.1.5/24" ] || fail "manual: ifconfig_em0 is '$(rc_value "$r" ifconfig_em0)'"
[ "$(rc_value "$r" defaultrouter)" = 192.168.1.1 ] || fail "manual: defaultrouter is '$(rc_value "$r" defaultrouter)'"
[ "$(rc_rest "$r" ifconfig_em0 defaultrouter)" = "$(printf %s "$others")" ] || fail "manual: changed other lines:
$(cat "$r/etc/rc.conf")"
[ "$(cat "$r/etc/start_if.em0" 2>/dev/null)" = "$dns_up" ] || fail "manual: start_if.em0 is:
$(cat "$r/etc/start_if.em0" 2>/dev/null)"
[ "$(cat "$r/etc/stop_if.em0" 2>/dev/null)" = "$dns_down" ] || fail "manual: stop_if.em0 is:
$(cat "$r/etc/stop_if.em0" 2>/dev/null)"
[ "$(mode "$r/etc/start_if.em0")" = 644 ] || fail "manual: start_if.em0 is mode $(mode "$r/etc/start_if.em0")"

# The same again changes no bytes.
cp "$r/etc/rc.conf" "$tmp/manual-rc.conf"
iface "$r" iface em0 manual 192.168.1.5/24 192.168.1.1 192.0.2.53 192.0.2.54
[ "$st" -eq 0 ] || fail "manual again: status $st: $(cat "$r.out")"
cmp -s "$r/etc/rc.conf" "$tmp/manual-rc.conf" || fail "manual again: changed rc.conf"

# Back to Automatic: DHCP, the card's gateway and DNS hooks gone, the rest kept.
iface "$r" iface em0 dhcp
[ "$st" -eq 0 ] || fail "manual to dhcp: status $st: $(cat "$r.out")"
[ "$(rc_value "$r" ifconfig_em0)" = DHCP ] || fail "manual to dhcp: ifconfig_em0 is '$(rc_value "$r" ifconfig_em0)'"
[ "$(rc_value "$r" defaultrouter)" = NO ] || fail "manual to dhcp: defaultrouter is '$(rc_value "$r" defaultrouter)'"
[ ! -e "$r/etc/start_if.em0" ] && [ ! -e "$r/etc/stop_if.em0" ] || fail "manual to dhcp: DNS hooks left"
[ "$(rc_rest "$r" ifconfig_em0)" = "$(printf %s "$others")" ] || fail "manual to dhcp: changed other lines:
$(cat "$r/etc/rc.conf")"

# Manual keeps the words it doesn't set (WPA, NOAUTO, mtu), with the address
# first, where ifconfig takes it; Automatic keeps SYNCDHCP.
r=$(root words 'ifconfig_wlan0="WPA SYNCDHCP mtu 1400 NOAUTO"
')
iface "$r" iface wlan0 manual 10.0.0.5/24
[ "$st" -eq 0 ] || fail "words: status $st: $(cat "$r.out")"
[ "$(rc_value "$r" ifconfig_wlan0)" = "WPA inet 10.0.0.5/24 mtu 1400 NOAUTO" ] ||
	fail "words: ifconfig_wlan0 is '$(rc_value "$r" ifconfig_wlan0)'"
r=$(root sync 'ifconfig_em0="SYNCDHCP mtu 9000"
')
iface "$r" iface em0 dhcp
[ "$st" -eq 0 ] && [ "$(rc_value "$r" ifconfig_em0)" = "SYNCDHCP mtu 9000" ] ||
	fail "SYNCDHCP: status $st, ifconfig_em0 is '$(rc_value "$r" ifconfig_em0)'"
# A bare first address is the card's address, as ifconfig takes it: Automatic
# drops it with its gateway, and Manual replaces it.
r=$(root bare 'ifconfig_em0="192.0.2.5/24"
defaultrouter="192.0.2.1"
')
iface "$r" iface em0 dhcp
[ "$st" -eq 0 ] && [ "$(rc_value "$r" ifconfig_em0)" = DHCP ] && [ "$(rc_value "$r" defaultrouter)" = NO ] ||
	fail "bare address to dhcp: status $st, ifconfig_em0 is '$(rc_value "$r" ifconfig_em0)', defaultrouter '$(rc_value "$r" defaultrouter)'"
r=$(root bare-netmask 'ifconfig_em0="192.0.2.5 netmask 255.255.255.0 mtu 9000"
')
iface "$r" iface em0 manual 198.51.100.5/24
[ "$st" -eq 0 ] && [ "$(rc_value "$r" ifconfig_em0)" = "inet 198.51.100.5/24 mtu 9000" ] ||
	fail "bare address to manual: status $st, ifconfig_em0 is '$(rc_value "$r" ifconfig_em0)'"

# FreeBSD's cloud images' default: accept_rtadv (an IPv6 flag, a command of
# ifconfig's) stays, with DHCP or an address.
r=$(root cloud 'ifconfig_DEFAULT="SYNCDHCP accept_rtadv"
')
iface "$r" iface em0 dhcp
[ "$st" -eq 0 ] && [ "$(rc_value "$r" ifconfig_em0)" = "SYNCDHCP accept_rtadv" ] ||
	fail "cloud image's default to dhcp: status $st, ifconfig_em0 is '$(rc_value "$r" ifconfig_em0)': $(cat "$r.out")"
iface "$r" iface em0 manual 10.0.0.5/24
[ "$st" -eq 0 ] && [ "$(rc_value "$r" ifconfig_em0)" = "inet 10.0.0.5/24 accept_rtadv" ] ||
	fail "cloud image's default to manual: status $st, ifconfig_em0 is '$(rc_value "$r" ifconfig_em0)': $(cat "$r.out")"

r=$(root after-up 'ifconfig_em0="up 192.0.2.5/24"
defaultrouter="192.0.2.1"
')
iface "$r" iface em0 dhcp
[ "$st" -eq 0 ] && [ "$(rc_value "$r" ifconfig_em0)" = "DHCP up" ] && [ "$(rc_value "$r" defaultrouter)" = NO ] ||
	fail "address after up to dhcp: status $st, ifconfig_em0 is '$(rc_value "$r" ifconfig_em0)', defaultrouter '$(rc_value "$r" defaultrouter)'"
r=$(root after-mtu 'ifconfig_em0="mtu 9000 192.0.2.5 netmask 255.255.255.0"
')
iface "$r" iface em0 manual 198.51.100.5/24
[ "$st" -eq 0 ] && [ "$(rc_value "$r" ifconfig_em0)" = "inet 198.51.100.5/24 mtu 9000" ] ||
	fail "address after mtu to manual: status $st, ifconfig_em0 is '$(rc_value "$r" ifconfig_em0)'"

# iface-switch keeps any line as it is, a * too.
r=$(root glob 'ifconfig_em0="DHCP description *"
')
(cd "$tmp" && iface "$r" iface-switch em0 off)
[ "$(rc_value "$r" ifconfig_em0)" = "DHCP description * NOAUTO" ] ||
	fail "a * in the line: ifconfig_em0 is '$(rc_value "$r" ifconfig_em0)'"
r=$(root netmask 'ifconfig_em0="inet 10.0.0.5 netmask 255.255.255.0 broadcast 10.0.0.255 mtu 9000"
')
iface "$r" iface em0 dhcp
[ "$st" -eq 0 ] && [ "$(rc_value "$r" ifconfig_em0)" = "DHCP mtu 9000" ] ||
	fail "netmask to dhcp: status $st, ifconfig_em0 is '$(rc_value "$r" ifconfig_em0)'"

# The gateway: one for the computer. A gateway on another card moves it; no
# gateway on a card leaves another card's alone, and takes this card's away.
r=$(root gw 'ifconfig_em1="inet 10.9.0.2/16"
defaultrouter="10.9.0.1"
')
iface "$r" iface em0 manual 192.168.1.5/24 -
[ "$st" -eq 0 ] && [ "$(rc_value "$r" defaultrouter)" = 10.9.0.1 ] ||
	fail "no gateway, another card's: status $st, defaultrouter is '$(rc_value "$r" defaultrouter)'"
iface "$r" iface em0 manual 192.168.1.5/24 192.168.1.1
[ "$st" -eq 0 ] && [ "$(rc_value "$r" defaultrouter)" = 192.168.1.1 ] ||
	fail "gateway moved: status $st, defaultrouter is '$(rc_value "$r" defaultrouter)'"
iface "$r" iface em1 manual 10.9.0.2/16
[ "$st" -eq 0 ] && [ "$(rc_value "$r" defaultrouter)" = 192.168.1.1 ] ||
	fail "the other card without one: status $st, defaultrouter is '$(rc_value "$r" defaultrouter)'"
iface "$r" iface em0 manual 192.168.1.5/24
[ "$st" -eq 0 ] && [ "$(rc_value "$r" defaultrouter)" = NO ] ||
	fail "gateway removed: status $st, defaultrouter is '$(rc_value "$r" defaultrouter)'"

# DNS without a gateway; then no DNS takes the hooks away.
r=$(root dnsonly '')
iface "$r" iface em0 manual 192.168.1.5/24 - 192.0.2.53 192.0.2.54
[ "$st" -eq 0 ] && [ "$(cat "$r/etc/start_if.em0" 2>/dev/null)" = "$dns_up" ] ||
	fail "DNS without a gateway: status $st, start_if.em0 is: $(cat "$r/etc/start_if.em0" 2>/dev/null)"
[ "$(rc_value "$r" defaultrouter)" = NO ] || fail "DNS without a gateway: defaultrouter is '$(rc_value "$r" defaultrouter)'"
iface "$r" iface em0 manual 192.168.1.5/24
[ "$st" -eq 0 ] && [ ! -e "$r/etc/start_if.em0" ] && [ ! -e "$r/etc/stop_if.em0" ] ||
	fail "no DNS: status $st, hooks left"

# The owner's own hooks: Manual DNS is refused and they're kept; Automatic
# leaves them alone.
r=$(root ownhook 'ifconfig_em0="DHCP"
')
echo 'ifconfig em0 media 100baseTX' >"$r/etc/start_if.em0"
cp "$r/etc/rc.conf" "$tmp/ownhook-rc.conf"
iface "$r" iface em0 manual 192.168.1.5/24 192.168.1.1 192.0.2.53
[ "$st" -ne 0 ] || fail "own hook: status 0"
grep -q start_if.em0 "$r.out" || fail "own hook: the message doesn't name start_if.em0: $(cat "$r.out")"
cmp -s "$r/etc/rc.conf" "$tmp/ownhook-rc.conf" || fail "own hook: changed rc.conf"
[ "$(cat "$r/etc/start_if.em0")" = 'ifconfig em0 media 100baseTX' ] || fail "own hook: changed start_if.em0"
iface "$r" iface em0 manual 192.168.1.5/24
[ "$st" -eq 0 ] && [ "$(cat "$r/etc/start_if.em0")" = 'ifconfig em0 media 100baseTX' ] ||
	fail "own hook, no DNS: status $st, start_if.em0 is '$(cat "$r/etc/start_if.em0")'"

# With resolvconf off in resolvconf.conf, nothing would take the DNS servers:
# Manual DNS is refused; Manual without it isn't.
dhcp_line='ifconfig_em0="DHCP"
'
for off in NO no false 0; do
	r=$(root "resolvconf-$off" "$dhcp_line")
	printf 'resolvconf=%s\n' "$off" >"$r/etc/resolvconf.conf"
	cp "$r/etc/rc.conf" "$tmp/resolvconf-rc.conf"
	iface "$r" iface em0 manual 192.168.1.5/24 - 192.0.2.53
	[ "$st" -ne 0 ] || fail "resolvconf=$off, DNS: status 0"
	grep -q resolvconf.conf "$r.out" || fail "resolvconf=$off: the message doesn't name resolvconf.conf: $(cat "$r.out")"
	cmp -s "$r/etc/rc.conf" "$tmp/resolvconf-rc.conf" && [ ! -e "$r/etc/start_if.em0" ] ||
		fail "resolvconf=$off, DNS: wrote rc.conf or a hook"
done
iface "$r" iface em0 manual 192.168.1.5/24
[ "$st" -eq 0 ] || fail "resolvconf off, no DNS: status $st: $(cat "$r.out")"
r=$(root resolvconf-yes "$dhcp_line")
printf 'resolvconf=\nname_servers_append=192.0.2.1\n' >"$r/etc/resolvconf.conf"
iface "$r" iface em0 manual 192.168.1.5/24 - 192.0.2.53
[ "$st" -eq 0 ] && [ -e "$r/etc/start_if.em0" ] || fail "resolvconf empty, DNS: status $st: $(cat "$r.out")"

# Refused, with nothing written: each bad input, and lines the page doesn't
# set up.
refused() {
	_name=$1
	_rc=$2
	shift 2
	r=$(root "refused-$_name" "$_rc")
	cp "$r/etc/rc.conf" "$tmp/refused-rc.conf"
	iface "$r" "$@"
	[ "$st" -ne 0 ] || fail "$_name: status 0"
	cmp -s "$r/etc/rc.conf" "$tmp/refused-rc.conf" || fail "$_name: changed rc.conf:
$(cat "$r/etc/rc.conf")"
	[ ! -e "$r/etc/start_if.em0" ] || fail "$_name: wrote start_if.em0"
}
plain='ifconfig_em0="DHCP"
'
refused octet "$plain" iface em0 manual 192.168.1.256/24
refused leading-zero "$plain" iface em0 manual 010.0.0.5/24
refused no-prefix "$plain" iface em0 manual 10.0.0.5
refused prefix-0 "$plain" iface em0 manual 10.0.0.5/0
refused prefix-33 "$plain" iface em0 manual 10.0.0.5/33
refused network "$plain" iface em0 manual 10.0.0.0/24
refused broadcast "$plain" iface em0 manual 10.0.0.255/24
refused loopback "$plain" iface em0 manual 127.0.0.5/8
refused multicast "$plain" iface em0 manual 224.0.0.5/24
refused zero "$plain" iface em0 manual 0.0.0.5/24
refused shell "$plain" iface em0 manual '10.0.0.5/24;reboot'
refused subst "$plain" iface em0 manual '$(id)'
refused gw-outside "$plain" iface em0 manual 10.0.0.5/24 10.0.1.1
refused gw-self "$plain" iface em0 manual 10.0.0.5/24 10.0.0.5
refused gw-broadcast "$plain" iface em0 manual 10.0.0.5/24 10.0.0.255
refused gw-host-route "$plain" iface em0 manual 10.0.0.5/32 10.0.0.1
refused dns-bad "$plain" iface em0 manual 10.0.0.5/24 - 1.2.3
refused dns-multicast "$plain" iface em0 manual 10.0.0.5/24 - 239.1.1.1
refused dns-four "$plain" iface em0 manual 10.0.0.5/24 - 1.1.1.1 1.0.0.1 9.9.9.9 8.8.8.8
refused loopback-card "$plain" iface lo0 dhcp
refused name-shell "$plain" iface 'em0;id' dhcp
refused name-path "$plain" iface ../em0 dhcp
refused name-long "$plain" iface abcdefghijklmnop0 dhcp
refused mode "$plain" iface em0 static 10.0.0.5/24
refused inet6 'ifconfig_em0="inet6 2001:db8::5 prefixlen 64"
' iface em0 dhcp
refused alias 'ifconfig_em0="inet 10.0.0.5/24 alias"
' iface em0 dhcp
refused two-addresses 'ifconfig_em0="inet 10.0.0.5/24 inet 10.0.0.6/24"
' iface em0 dhcp
refused description-address 'ifconfig_em0="DHCP description 192.0.2.5"
' iface em0 dhcp
refused description 'ifconfig_em0="DHCP description home"
' iface em0 manual 10.0.0.5/24
refused media 'ifconfig_em0="DHCP media 1000baseT"
' iface em0 dhcp
refused capability 'ifconfig_em0="-tso DHCP"
' iface em0 dhcp
refused mtu-word 'ifconfig_em0="DHCP mtu big"
' iface em0 dhcp
refused inet6-in-the-line 'ifconfig_em0="DHCP inet6 accept_rtadv"
' iface em0 manual 10.0.0.5/24
refused two-bare-addresses 'ifconfig_em0="10.0.0.5/24 10.0.0.6/24"
' iface em0 dhcp
refused inet-then-bare 'ifconfig_em0="inet 10.0.0.5/24 up 10.0.0.6"
' iface em0 dhcp
refused hostap 'ifconfig_wlan0="HOSTAP inet 10.0.0.1/24"
' iface wlan0 dhcp
refused switch-wlan 'ifconfig_wlan0="WPA DHCP"
' iface-switch wlan0 off

# A /32 without a gateway, and a /31's other address as its gateway.
r=$(root host32 '')
iface "$r" iface em0 manual 10.0.0.5/32
[ "$st" -eq 0 ] && [ "$(rc_value "$r" ifconfig_em0)" = "inet 10.0.0.5/32" ] ||
	fail "/32: status $st, ifconfig_em0 is '$(rc_value "$r" ifconfig_em0)'"
iface "$r" iface em0 manual 10.0.0.4/31 10.0.0.5
[ "$st" -eq 0 ] && [ "$(rc_value "$r" defaultrouter)" = 10.0.0.5 ] ||
	fail "/31: status $st, defaultrouter is '$(rc_value "$r" defaultrouter)'"

# Off: NOAUTO on the card's line (its own line from ifconfig_DEFAULT), once;
# on takes it out and keeps the rest.
r=$(root iface-off 'ifconfig_DEFAULT="DHCP"
')
iface "$r" iface-switch em0 off
[ "$st" -eq 0 ] && [ "$(rc_value "$r" ifconfig_em0)" = "DHCP NOAUTO" ] ||
	fail "off: status $st, ifconfig_em0 is '$(rc_value "$r" ifconfig_em0)'"
iface "$r" iface-switch em0 off
[ "$st" -eq 0 ] && [ "$(rc_value "$r" ifconfig_em0)" = "DHCP NOAUTO" ] ||
	fail "off twice: status $st, ifconfig_em0 is '$(rc_value "$r" ifconfig_em0)'"
r=$(root iface-on 'ifconfig_em0="inet 10.0.0.5/24 noauto mtu 9000"
')
iface "$r" iface-switch em0 on
[ "$st" -eq 0 ] && [ "$(rc_value "$r" ifconfig_em0)" = "inet 10.0.0.5/24 mtu 9000" ] ||
	fail "on: status $st, ifconfig_em0 is '$(rc_value "$r" ifconfig_em0)'"
r=$(root iface-sideways "$plain")
iface "$r" iface-switch em0 sideways
[ "$st" -eq 2 ] || fail "iface-switch sideways: status $st"
# iface-switch on em0.10, with a wlan named em0x10.
r=$(root iface-dotted 'ifconfig_DEFAULT="DHCP"
wlans_iwlwifi0="em0x10"
')
iface "$r" iface-switch em0.10 off
[ "$st" -eq 0 ] && [ "$(rc_value "$r" ifconfig_em0_10)" = "DHCP NOAUTO" ] ||
	fail "iface-switch off, a dotted card: status $st: $(cat "$r.out")"

# lagg: a new one, its ports up, in cloned_interfaces with the others.
lagged='cloned_interfaces="bridge0"
ifconfig_em0="DHCP"
ifconfig_em1="DHCP"
ifconfig_em2="DHCP NOAUTO"
'
r=$(root lagg-new "$lagged")
iface "$r" lagg lagg0 failover em0,em1 dhcp
[ "$st" -eq 0 ] && [ "$(rc_value "$r" cloned_interfaces)" = "bridge0 lagg0" ] &&
	[ "$(rc_value "$r" ifconfig_lagg0)" = "DHCP up laggproto failover laggport em0 laggport em1" ] &&
	[ "$(rc_value "$r" ifconfig_em0)" = up ] && [ "$(rc_value "$r" ifconfig_em1)" = up ] ||
	fail "lagg, new: status $st: $(cat "$r.out"); rc.conf: $(cat "$r/etc/rc.conf")"
# Another mode and ports, Manual: em0 goes back to DHCP, em2 is up.
iface "$r" lagg lagg0 roundrobin em1,em2 manual 10.0.0.5/24 10.0.0.1
[ "$st" -eq 0 ] && [ "$(rc_value "$r" cloned_interfaces)" = "bridge0 lagg0" ] &&
	[ "$(rc_value "$r" ifconfig_lagg0)" = "inet 10.0.0.5/24 up laggproto roundrobin laggport em1 laggport em2" ] &&
	[ "$(rc_value "$r" defaultrouter)" = 10.0.0.1 ] && [ "$(rc_value "$r" ifconfig_em0)" = DHCP ] &&
	[ "$(rc_value "$r" ifconfig_em2)" = up ] ||
	fail "lagg, changed: status $st: $(cat "$r.out"); rc.conf: $(cat "$r/etc/rc.conf")"
# Removed: its ports on DHCP, its gateway gone, the other clone kept.
iface "$r" lagg lagg0 remove
[ "$st" -eq 0 ] && [ "$(rc_value "$r" cloned_interfaces)" = bridge0 ] &&
	! grep -q '^ifconfig_lagg0=\|^defaultrouter=' "$r/etc/rc.conf" && [ "$(rc_value "$r" ifconfig_em1)" = DHCP ] &&
	[ "$(rc_value "$r" ifconfig_em2)" = DHCP ] ||
	fail "lagg, removed: status $st: $(cat "$r.out"); rc.conf: $(cat "$r/etc/rc.conf")"

# A lagg changed keeps its mtu and accept_rtadv (or -accept_rtadv).
for v6 in accept_rtadv -accept_rtadv; do
	r=$(root lagg-kept "cloned_interfaces=\"lagg0\"
ifconfig_lagg0=\"DHCP up mtu 1400 $v6 laggproto failover laggport em0 laggport em1\"
ifconfig_em0=\"up\"
ifconfig_em1=\"up\"
")
	iface "$r" lagg lagg0 roundrobin em0,em1 dhcp
	[ "$st" -eq 0 ] &&
		[ "$(rc_value "$r" ifconfig_lagg0)" = "DHCP up laggproto roundrobin laggport em0 laggport em1 mtu 1400 $v6" ] ||
		fail "lagg, its other settings ($v6): status $st: $(cat "$r.out"); ifconfig_lagg0 is '$(rc_value "$r" ifconfig_lagg0)'"
done

# A Manual card that becomes a port loses its gateway and DNS hooks.
r=$(root lagg-manual-port 'ifconfig_em0="inet 10.0.0.5/24"
ifconfig_em1="DHCP"
defaultrouter="10.0.0.1"
')
iface "$r" iface em0 manual 10.0.0.5/24 10.0.0.1 192.0.2.53
iface "$r" lagg lagg0 failover em0,em1 dhcp
[ "$st" -eq 0 ] && ! grep -q '^defaultrouter=' "$r/etc/rc.conf" && [ ! -e "$r/etc/start_if.em0" ] &&
	[ ! -e "$r/etc/stop_if.em0" ] || fail "lagg, a Manual port: status $st: $(cat "$r.out"); rc.conf: $(cat "$r/etc/rc.conf")"

# lagg refuses, writing nothing: another mode, one card, a card twice, the
# lagg as its own port, a name that isn't a lagg's, a port set up by hand, a
# lagg set up by hand, and removing a lagg that isn't there.
r=$(root lagg-refused "${lagged}cloned_interfaces=\"lagg1\"
ifconfig_lagg1=\"laggproto lacp lagghash l2 laggport em0 laggport em1 DHCP\"
ifconfig_em3=\"DHCP description office\"
")
before=$(cat "$r/etc/rc.conf")
for args in 'lagg2 bonding em0,em1 dhcp' 'lagg2 failover em0 dhcp' 'lagg2 failover em0,em0 dhcp' \
	'lagg2 failover lagg2,em0 dhcp' 'bridge0 failover em0,em1 dhcp' 'lagg2 failover em0,em3 dhcp' \
	'lagg1 lacp em0,em1 dhcp' 'lagg1 remove' 'lagg2 remove' 'lagg2 failover em0,em1 manual 10.0.0.0/24'; do
	iface "$r" lagg $args
	[ "$st" -eq 1 ] && [ "$(cat "$r/etc/rc.conf")" = "$before" ] ||
		fail "lagg $args: status $st, should refuse: $(cat "$r.out")"
done

# iface-switch, iface, wifi off, card-gone, card-back and lagg on the running
# system, against stand-ins for lockf, service, route, resolvconf, ifconfig
# (em0, lagg0's port em0x10, and the cards in $tmp/net, laggs only there) and
# sysrc (on a scratch rc.conf) that log what they're asked, with the start_if
# hooks in the scratch etc; not as root.
if [ "$(id -u)" -ne 0 ]; then
	stubs=$tmp/stubs
	mkdir -p "$stubs"
	run=$tmp/run-helper
	sed -e "s|^PATH=/sbin:/bin:/usr/sbin:/usr/bin\$|PATH=$stubs:/sbin:/bin:/usr/sbin:/usr/bin|" \
		-e 's|^\[ -n "\$R" \] .*run this as root."$|:|' \
		-e "s|\\\$R/etc/start_if\\.|$tmp/run/etc/start_if.|" "$helper" >"$run"
	[ "$(grep -c "^PATH=$stubs:" "$run")" -eq 1 ] && ! grep -q 'run this as root' "$run" &&
		! grep -q '\$R/etc/start_if' "$run" ||
		fail "the stand-in helper didn't take its PATH, root check and start_if path"
	chmod +x "$run"
	# dhclient gets a lease with a router on the cards named in $tmp/net/leases,
	# one "<card> [<card the route goes through>]" line each.
	cat >"$stubs/service" <<EOF
#!/bin/sh
echo "service \$*" >>"$tmp/log"
[ "\$1 \$2" = "dhclient restart" ] || exit 0
set -- \$(awk -v c="\$3" '\$1 == c' "$tmp/net/leases" 2>/dev/null)
[ \$# -ge 1 ] || exit 0
echo "\${2:-\$1}" >"$tmp/net/default"
echo "\$1" >>"$tmp/net/dns"
EOF
	# resolvconf has DNS servers for the cards named in $tmp/net/dns.
	printf '#!/bin/sh\necho "resolvconf $*" >>"%s/log"\n[ "$1" != -i ] || grep -qxF "$2" "%s/net/dns" 2>/dev/null\n' "$tmp" "$tmp" >"$stubs/resolvconf"
	printf '#!/bin/sh\necho "lockf $*" >>"%s/log"\nshift 2\nexec "$@"\n' "$tmp" >"$stubs/lockf"
	# The default route goes through the card named in $tmp/net/default; one
	# added goes through the first card with an address on the gateway's /24.
	cat >"$stubs/route" <<EOF
#!/bin/sh
echo "route \$*" >>"$tmp/log"
case "\$*" in
"-n get default") [ -s "$tmp/net/default" ] && printf '  interface: %s\n' "\$(cat "$tmp/net/default")" ;;
"-n delete default") rm -f "$tmp/net/default" ;;
"-n add default "*)
	for f in "$tmp"/net/*; do
		! grep -q "inet \${4%.*}\\." "\$f" 2>/dev/null || { basename "\$f" >"$tmp/net/default"; break; }
	done
	;;
esac
EOF
	cat >"$stubs/ifconfig" <<EOF
#!/bin/sh
echo "ifconfig \$*" >>"$tmp/log"
[ "\$1" != -n ] || shift
if [ \$# -eq 1 ] && [ -f "$tmp/net/\$1" ]; then
	cat "$tmp/net/\$1"
	exit
fi
case "\$*" in
-C) echo "bridge lagg" ;;
-a) printf 'lagg0: flags=8843<UP> mtu 1500\n\tlaggport: em0x10 flags=0<>\n' ;;
-l) ls "$tmp/net" 2>/dev/null | grep -vxE 'default|dns|leases' | paste -s -d ' ' - ;;
em0) printf 'em0: flags=8843<UP> mtu 1500\n\tether 02:00:00:00:00:10\n\tinet 10.0.0.5 netmask 0xffffff00 broadcast 10.0.0.255\n' ;;
lagg[0-9] | lagg[0-9][0-9]) exit 1 ;;
esac
EOF
	printf '#!/bin/sh\nexec /usr/sbin/sysrc -f "%s/run/etc/rc.conf" "$@"\n' "$tmp" >"$stubs/sysrc"
	chmod +x "$stubs"/*

	# switch_on runs iface-switch em0 on with rc.conf $1; the stand-ins' log in $tmp/log.
	switch_on() {
		mkdir -p "$tmp/run/etc"
		printf '%s' "$1" >"$tmp/run/etc/rc.conf"
		: >"$tmp/log"
		st=0
		"$run" iface-switch em0 on >"$tmp/run.out" 2>&1 || st=$?
	}

	# A DHCP card with words iface doesn't change still gets dhclient.
	switch_on 'ifconfig_em0="DHCP NOAUTO description office"
'
	[ "$st" -eq 0 ] && grep -qx 'service netif start em0' "$tmp/log" && grep -qx 'service dhclient quietstart em0' "$tmp/log" ||
		fail "on, DHCP with a description: status $st: $(cat "$tmp/run.out"); asked: $(cat "$tmp/log")"

	# A manual card gets the gateway on its address back, whatever else its line has.
	switch_on 'ifconfig_em0="inet 10.0.0.5/24 NOAUTO description office"
defaultrouter="10.0.0.1"
'
	[ "$st" -eq 0 ] && grep -qx 'route -n add default 10.0.0.1' "$tmp/log" && ! grep -q dhclient "$tmp/log" ||
		fail "on, manual with a description: status $st: $(cat "$tmp/run.out"); asked: $(cat "$tmp/log")"

	# A gateway on another network isn't this card's.
	switch_on 'ifconfig_em0="inet 10.0.0.5/24 NOAUTO"
defaultrouter="192.168.1.1"
'
	[ "$st" -eq 0 ] && ! grep -q 'route -n add' "$tmp/log" ||
		fail "on, another network's gateway: status $st; asked: $(cat "$tmp/log")"

	# stand_in <rc.conf> <helper arguments>: the stand-in helper on that rc.conf.
	stand_in() {
		mkdir -p "$tmp/run/etc"
		printf '%s' "$1" >"$tmp/run/etc/rc.conf"
		shift
		: >"$tmp/log"
		st=0
		"$run" "$@" >"$tmp/run.out" 2>&1 || st=$?
	}
	asked() {
		grep -qxF "$1" "$tmp/log"
	}

	# iface-switch off, iface and wifi off take the card's DNS servers out.
	stand_in 'ifconfig_em0="DHCP"
' iface-switch em0 off
	[ "$st" -eq 0 ] && asked 'service netif stop em0' && asked 'resolvconf -f -d em0' &&
		asked 'resolvconf -f -d em0:static' || fail "off, the DNS servers: status $st: $(cat "$tmp/run.out"); asked: $(cat "$tmp/log")"
	stand_in 'ifconfig_em0="DHCP"
' iface em0 dhcp
	[ "$st" -eq 0 ] && asked 'resolvconf -f -d em0' ||
		fail "iface, the old DNS servers: status $st: $(cat "$tmp/run.out"); asked: $(cat "$tmp/log")"
	stand_in 'wlans_iwlwifi0="wlan0"
ifconfig_wlan0="WPA DHCP"
' wifi off
	[ "$st" -eq 0 ] && asked 'service netif stop wlan0' && asked 'resolvconf -f -d wlan0' ||
		fail "wifi off, the DNS servers: status $st: $(cat "$tmp/run.out"); asked: $(cat "$tmp/log")"

	# iface-switch on em0.10, beside lagg0's port em0x10.
	stand_in 'ifconfig_DEFAULT="DHCP"
' iface-switch em0.10 off
	[ "$st" -eq 0 ] && asked 'service netif stop em0.10' ||
		fail "off, a dotted card beside a lagg: status $st: $(cat "$tmp/run.out"); asked: $(cat "$tmp/log")"

	# cards <if> | -: new stand-in cards, the default route through <if>,
	# which has DNS servers.
	cards() {
		rm -rf "$tmp/net"
		mkdir "$tmp/net"
		[ "$1" = - ] || echo "$1" | tee "$tmp/net/default" >"$tmp/net/dns"
	}
	# card <if> <flags> <status> [<address> <netmask>]: a stand-in card.
	card() {
		{
			echo "$1: flags=8843<$2> mtu 1500"
			[ $# -lt 4 ] || printf '\tinet %s netmask %s\n' "$4" "$5"
			printf '\tstatus: %s\n' "$3"
		} >"$tmp/net/$1"
	}
	gone_failed() {
		fail "$1: status $st: $(cat "$tmp/run.out"); asked: $(cat "$tmp/log")"
	}
	two='ifconfig_em0="DHCP"
wlans_run0="wlan0"
ifconfig_wlan0="WPA DHCP"
'

	# The cable out of em0, which has the route; Wi-Fi on another network
	# gets it, with its DNS servers, by DHCP.
	cards em0
	card em0 UP 'no carrier' 10.1.9.5 0xffffff00
	card wlan0 UP associated 10.1.7.172 0xffffff00
	stand_in "$two" card-gone em0
	[ "$st" -eq 0 ] && asked "lockf -k /var/run/hyprbsd-network.lock $run card-gone em0" &&
		asked 'route -n delete default' && asked 'resolvconf -f -d em0' &&
		asked 'service dhclient restart wlan0' && ! grep -q /32 "$tmp/log" || gone_failed "card-gone, cable out"

	# On one network, em0's address drops to /32 so the network goes through Wi-Fi.
	cards em0
	card em0 UP 'no carrier' 10.0.0.5 0xffffff00
	card wlan0 UP associated 10.0.0.7 0xffffff00
	stand_in "$two" card-gone em0
	[ "$st" -eq 0 ] && asked 'ifconfig em0 inet 10.0.0.5/32' && asked 'service dhclient restart wlan0' ||
		gone_failed "card-gone, one network"

	# em0 down (ifconfig em0 down: dhclient's FAIL leaves its address and
	# route): Wi-Fi gets the route.
	cards em0
	card em0 BROADCAST active 10.1.9.5 0xffffff00
	card wlan0 UP associated 10.1.7.172 0xffffff00
	stand_in "$two" card-gone em0
	[ "$st" -eq 0 ] && asked 'route -n delete default' && asked 'service dhclient restart wlan0' ||
		gone_failed "card-gone, card down"

	# The link back by then, or the route through another card: nothing moves.
	cards em0
	card em0 UP active 10.1.9.5 0xffffff00
	card wlan0 UP associated 10.1.7.172 0xffffff00
	stand_in "$two" card-gone em0
	[ "$st" -eq 0 ] && ! grep -q 'route -n delete\|service' "$tmp/log" || gone_failed "card-gone, link back"
	cards wlan0
	card em0 UP 'no carrier' 10.1.9.5 0xffffff00
	card wlan0 UP associated 10.1.7.172 0xffffff00
	stand_in "$two" card-gone em0
	[ "$st" -eq 0 ] && ! grep -q 'route -n delete\|service' "$tmp/log" || gone_failed "card-gone, route elsewhere"

	# em0's lease ended, taking its route: Wi-Fi gets it.
	cards -
	card em0 UP active
	card wlan0 UP associated 10.1.7.172 0xffffff00
	stand_in "$two" card-gone em0
	[ "$st" -eq 0 ] && asked 'service dhclient restart wlan0' || gone_failed "card-gone, lease ended"

	# Wi-Fi gone: the Manual card on defaultrouter's network gets it, not
	# one that's off.
	cards -
	card em0 BROADCAST active
	card em1 UP active 10.0.0.5 0xffffff00
	stand_in 'ifconfig_em0="DHCP NOAUTO"
ifconfig_em1="inet 10.0.0.5/24"
defaultrouter="10.0.0.1"
' card-gone wlan0
	[ "$st" -eq 0 ] && asked 'route -n add default 10.0.0.1' && ! grep -q service "$tmp/log" ||
		gone_failed "card-gone, a Manual card"

	# The route through a DHCP card without DNS servers, after card-gone or
	# Apply (one network: em0's /24 moved to Wi-Fi as its address went): Wi-Fi
	# asks for its lease again.
	cards wlan0
	: >"$tmp/net/dns"
	card em0 UP active 10.0.0.5 0xffffff00
	card wlan0 UP associated 10.0.0.7 0xffffff00
	stand_in "$two" card-gone em0
	[ "$st" -eq 0 ] && asked 'service dhclient restart wlan0' || gone_failed "card-gone, the route's DNS"
	stand_in "$two" iface em0 dhcp
	[ "$st" -eq 0 ] && asked 'service dhclient restart wlan0' || gone_failed "iface, the route's DNS"

	# Switching off the card with the route, wired or Wi-Fi, hands it on.
	cards em0
	card em0 UP active 10.1.9.5 0xffffff00
	card wlan0 UP associated 10.1.7.172 0xffffff00
	stand_in "$two" iface-switch em0 off
	[ "$st" -eq 0 ] && asked 'route -n delete default' && asked 'service dhclient restart wlan0' ||
		gone_failed "iface-switch off, the route"
	cards wlan0
	card em0 UP active 10.1.9.5 0xffffff00
	card wlan0 UP associated 10.1.7.172 0xffffff00
	stand_in "$two" wifi off
	[ "$st" -eq 0 ] && asked 'route -n delete default' && asked 'service dhclient restart em0' ||
		gone_failed "wifi off, the route"

	# em1 gets no router by DHCP: em2, the next card, gets the route.
	cards em0
	echo em2 >"$tmp/net/leases"
	card em0 UP 'no carrier' 10.0.0.5 0xffffff00
	card em1 UP active 10.1.0.5 0xffffff00
	card em2 UP active 192.0.2.5 0xffffff00
	stand_in 'ifconfig_em0="DHCP"
ifconfig_em1="DHCP"
ifconfig_em2="DHCP"
' card-gone em0
	[ "$st" -eq 0 ] && asked 'service dhclient restart em1' && asked 'service dhclient restart em2' &&
		[ "$(cat "$tmp/net/default")" = em2 ] || gone_failed "card-gone, no router on the first card"

	manual='ifconfig_em0="inet 10.0.0.5/24"
ifconfig_em1="DHCP"
defaultrouter="10.0.0.1"
'
	# The cable out of Manual em0, which has the route: its DNS servers go
	# when em1 takes it.
	cards em0
	echo em0:static >"$tmp/net/dns"
	echo em1 >"$tmp/net/leases"
	card em0 UP 'no carrier' 10.0.0.5 0xffffff00
	card em1 UP active 10.1.0.5 0xffffff00
	stand_in "$manual" card-gone em0
	[ "$st" -eq 0 ] && asked 'service dhclient restart em1' && asked 'resolvconf -f -d em0:static' ||
		gone_failed "card-gone, a Manual card's DNS servers"

	# No other card to take it: the route stays through Manual em0, with its
	# DNS servers, for when the cable's back.
	cards em0
	echo em0:static >"$tmp/net/dns"
	card em0 UP 'no carrier' 10.0.0.5 0xffffff00
	stand_in "$manual" card-gone em0
	[ "$st" -eq 0 ] && [ "$(cat "$tmp/net/default" 2>/dev/null)" = em0 ] && ! asked 'resolvconf -f -d em0:static' ||
		gone_failed "card-gone, a Manual card alone"

	# em1's lease puts the route through em0, on its network: em1 hasn't
	# taken it, and em2 gets it.
	cards em0
	printf 'em1 em0\nem2\n' >"$tmp/net/leases"
	card em0 UP 'no carrier' 10.0.0.5 0xffffff00
	card em1 UP active 10.0.0.7 0xffffff00
	card em2 UP active 192.0.2.5 0xffffff00
	stand_in "${manual}ifconfig_em2=\"DHCP\"
" card-gone em0
	[ "$st" -eq 0 ] && asked 'service dhclient restart em2' && [ "$(cat "$tmp/net/default")" = em2 ] &&
		[ "$(grep -c 'route -n delete default' "$tmp/log")" -eq 2 ] ||
		gone_failed "card-gone, a lease with the route through the card gone"

	# Manual em0 takes the route without its DNS servers: its start_if puts
	# them back.
	cards em1
	card em0 UP active 10.0.0.5 0xffffff00
	card em1 UP 'no carrier' 10.1.0.5 0xffffff00
	mkdir -p "$tmp/run/etc"
	printf "# hyprbsd-network: em0's DNS servers, through resolvconf(8).\nresolvconf -a em0:static </dev/null\n" >"$tmp/run/etc/start_if.em0"
	stand_in "$manual" card-gone em1
	[ "$st" -eq 0 ] && asked 'route -n add default 10.0.0.1' && asked 'resolvconf -a em0:static' ||
		gone_failed "card-gone, a Manual card's DNS servers back"
	rm -f "$tmp/run/etc/start_if.em0"

	# Switching off Manual em0, with the only DNS servers, while the route
	# goes through em1 on its network: em1 asks for its lease again.
	cards em1
	echo em0:static >"$tmp/net/dns"
	card em0 UP active 10.0.0.5 0xffffff00
	card em1 UP active 10.0.0.7 0xffffff00
	stand_in "$manual" iface-switch em0 off
	[ "$st" -eq 0 ] && asked 'service dhclient restart em1' || gone_failed "iface-switch off, the route's DNS"

	# On one network, Manual em0's address drops to /32 so em1 gets the
	# network and the route.
	cards em0
	echo em1 >"$tmp/net/leases"
	card em0 UP 'no carrier' 10.0.0.5 0xffffff00
	card em1 UP active 10.0.0.7 0xffffff00
	stand_in "$manual" card-gone em0
	[ "$st" -eq 0 ] && asked 'ifconfig em0 inet 10.0.0.5/32' && asked 'service dhclient restart em1' ||
		gone_failed "card-gone, a Manual card on one network"

	# card-back: em0's link back, its /24 back; the route stays through em1.
	cards em1
	card em0 UP active 10.0.0.5 0xffffffff
	card em1 UP active 10.0.0.7 0xffffff00
	stand_in "$manual" card-back em0
	[ "$st" -eq 0 ] && asked 'ifconfig em0 inet 10.0.0.5/24' && ! asked 'route -n add default 10.0.0.1' ||
		gone_failed "card-back, a Manual card's netmask"

	# card-back with no route anywhere: Manual em0 gets defaultrouter.
	cards -
	card em0 UP active 10.0.0.5 0xffffff00
	stand_in "$manual" card-back em0
	[ "$st" -eq 0 ] && asked 'route -n add default 10.0.0.1' && ! grep -q 'ifconfig em0 inet' "$tmp/log" ||
		gone_failed "card-back, no route"

	# card-back leaves a DHCP card to dhclient.
	cards -
	card em1 UP active 10.0.0.7 0xffffffff
	stand_in "$manual" card-back em1
	[ "$st" -eq 0 ] && ! grep -q 'ifconfig em1 inet\|route -n add\|service' "$tmp/log" ||
		gone_failed "card-back, a DHCP card"

	# The cable out of Manual em0 while the route goes through em1: its DNS
	# servers go.
	cards em1
	echo em0:static >>"$tmp/net/dns"
	card em0 UP 'no carrier' 10.0.0.5 0xffffff00
	card em1 UP active 10.1.0.5 0xffffff00
	stand_in "$manual" card-gone em0
	[ "$st" -eq 0 ] && asked 'resolvconf -f -d em0:static' && ! grep -q 'route -n delete' "$tmp/log" ||
		gone_failed "card-gone, a Manual card without the route"

	# card-back: Manual em0's link back puts its DNS servers back, the route
	# through em1 or not.
	cards em1
	card em0 UP active 10.0.0.5 0xffffff00
	card em1 UP active 10.1.0.5 0xffffff00
	mkdir -p "$tmp/run/etc"
	printf "# hyprbsd-network: em0's DNS servers, through resolvconf(8).\nresolvconf -a em0:static </dev/null\n" >"$tmp/run/etc/start_if.em0"
	stand_in "$manual" card-back em0
	[ "$st" -eq 0 ] && asked 'resolvconf -a em0:static' || gone_failed "card-back, a Manual card's DNS servers"

	# card-back after the link went again, without a carrier or down: nothing
	# comes back.
	for flags_status in 'UP|no carrier' 'BROADCAST|active'; do
		cards -
		card em0 "${flags_status%|*}" "${flags_status#*|}" 10.0.0.5 0xffffffff
		stand_in "$manual" card-back em0
		[ "$st" -eq 0 ] && ! grep -q 'resolvconf -a\|ifconfig em0 inet\|route -n add' "$tmp/log" ||
			gone_failed "card-back, em0 gone again ($flags_status)"
	done
	rm -f "$tmp/run/etc/start_if.em0"

	# lagg with a wlan port: em0 takes wlan0's MAC address, and the ports start
	# after the lagg stops (which takes them down) and before it starts.
	cards -
	printf 'em0: flags=8843<UP> mtu 1500\n\tether 02:00:00:00:00:10\n\tstatus: active\n' >"$tmp/net/em0"
	printf 'wlan0: flags=8843<UP> mtu 1500\n\tether 02:00:00:00:00:13\n\tgroups: wlan\n\tstatus: associated\n' >"$tmp/net/wlan0"
	wired_wifi='ifconfig_em0="DHCP"
wlans_iwlwifi0="wlan0"
ifconfig_wlan0="WPA DHCP"
'
	stand_in "$wired_wifi" lagg lagg0 failover em0,wlan0 dhcp
	[ "$st" -eq 0 ] && [ "$(rc_value "$tmp/run" ifconfig_em0)" = "ether 02:00:00:00:00:13" ] &&
		[ "$(rc_value "$tmp/run" ifconfig_wlan0)" = WPA ] &&
		[ "$(grep -n 'service netif start' "$tmp/log" | cut -d ' ' -f 4 | paste -s -d ' ' -)" = "em0 wlan0 lagg0" ] &&
		[ "$(grep -n -m 1 'service netif stop lagg0' "$tmp/log" | cut -d : -f 1)" -lt \
			"$(grep -n 'service netif start em0' "$tmp/log" | cut -d : -f 1)" ] ||
		gone_failed "lagg, a wlan port"

	# Removing it: em0's own MAC address back, the lagg destroyed, the ports
	# back on DHCP.
	printf 'em0: flags=8843<UP> mtu 1500\n\tether 02:00:00:00:00:13\n\thwaddr 02:00:00:00:00:10\n\tstatus: active\n' >"$tmp/net/em0"
	printf 'lagg0: flags=8843<UP> mtu 1500\n\tgroups: lagg\n\tstatus: active\n' >"$tmp/net/lagg0"
	stand_in 'cloned_interfaces="lagg0"
ifconfig_lagg0="DHCP up laggproto failover laggport em0 laggport wlan0"
ifconfig_em0="ether 02:00:00:00:00:13"
wlans_iwlwifi0="wlan0"
ifconfig_wlan0="WPA"
' lagg lagg0 remove
	[ "$st" -eq 0 ] && asked 'ifconfig em0 ether 02:00:00:00:00:10' && asked 'ifconfig lagg0 destroy' &&
		[ "$(rc_value "$tmp/run" ifconfig_em0)" = DHCP ] && [ "$(rc_value "$tmp/run" ifconfig_wlan0)" = "WPA DHCP" ] &&
		asked 'service dhclient quietstart em0' || gone_failed "lagg, removed on the running system"
fi

[ "$failures" -eq 0 ] || exit 1
echo "network helper: ok"
