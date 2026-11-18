# stop_supplicant and the checks it uses, sourced by hnetteststuck and
# wifi-checks.
#
# SPDX-License-Identifier: BSD-2-Clause

# stop_supplicant <pidfile> <devd pidfile> <record> <seconds>: <record>
# removed; within the seconds, <pidfile> names a running process called
# wpa_supplicant; with no devd running at <devd pidfile>'s pid, it's sent
# SIGSTOP and must show state T within 2 s. Its pid then goes to <record> and
# is printed. Otherwise says why, and nothing is recorded.
stop_supplicant() {
	rm -f "$3" || {
		echo "couldn't remove $3"
		return 1
	}
	_until=$(($(date +%s) + $4))
	while ! _pid=$(supplicant_in "$1"); do
		if [ "$(date +%s)" -ge "$_until" ]; then
			echo "$_pid"
			return 1
		fi
		sleep 0.2
	done
	_devd=$(cat "$2" 2>/dev/null || true)
	if [ -n "$_devd" ] && [ "$(ps -o comm= -p "$_devd" 2>/dev/null)" = devd ]; then
		echo "devd is already running (pid $_devd)"
		return 1
	fi
	kill -STOP "$_pid" || {
		echo "couldn't send wpa_supplicant ($_pid) SIGSTOP"
		return 1
	}
	_i=0
	while ! is_stopped "$_pid"; do
		if [ $_i -ge 10 ]; then
			echo "wpa_supplicant ($_pid) isn't stopped after SIGSTOP: state '$(ps -o state= -p "$_pid" 2>/dev/null)'"
			return 1
		fi
		sleep 0.2
		_i=$((_i + 1))
	done
	(echo "$_pid" >"$3") 2>/dev/null || {
		echo "couldn't write $3"
		return 1
	}
	echo "$_pid"
}

is_stopped() {
	case $(ps -o state= -p "$1" 2>/dev/null) in
	*T*) return 0 ;;
	esac
	return 1
}

# supplicant_in <pidfile>: the pid in it, of a running process called
# wpa_supplicant; says what's wrong otherwise.
supplicant_in() {
	_p=$(cat "$1" 2>/dev/null || true)
	case $_p in
	'' | *[!0-9]*)
		echo "no pid in $1"
		return 1
		;;
	esac
	_comm=$(ps -o comm= -p "$_p" 2>/dev/null || true)
	if [ -z "$_comm" ]; then
		echo "wpa_supplicant's pid $_p ($1) isn't running"
		return 1
	fi
	if [ "$_comm" != wpa_supplicant ]; then
		echo "pid $_p in $1 is $_comm, not wpa_supplicant"
		return 1
	fi
	echo "$_p"
}
