#!/bin/sh
#
# Checks a built hyprbsd-network's loaded strings (strings(1) without -a): the
# helper under PREFIX and Noto's fonts under LOCALBASE, neither at its
# /usr/local path when the build was given others, and nothing naming the
# DESTDIR the build ran with. Reads the file; doesn't run it.
#
#   tests/binary-paths-test.sh <binary> <prefix> <localbase> <destdir>
#
# SPDX-License-Identifier: BSD-2-Clause

set -eu
[ $# -eq 4 ] || {
	echo "usage: tests/binary-paths-test.sh binary prefix localbase destdir" >&2
	exit 2
}
bin=$1
helper=$2/libexec/hyprbsd-network-helper
fonts=$3/share/fonts/noto
destdir=$4
[ -f "$bin" ] && [ -x "$bin" ] || {
	echo "binary-paths-test: no program at $bin" >&2
	exit 1
}
tmp=$(mktemp -t network-strings) && [ -f "$tmp" ] || exit 1
trap 'rm -f "$tmp"' EXIT
strings "$bin" >"$tmp" || {
	echo "binary-paths-test: strings failed on $bin" >&2
	exit 1
}
failures=0

fail() {
	echo "binary-paths-test: $bin $*" >&2
	failures=$((failures + 1))
}

has() {
	grep -qF -e "$1" "$tmp"
}

has "$helper" || fail "doesn't name $helper"
has "$fonts" || fail "doesn't name $fonts"
for old in /usr/local/libexec/hyprbsd-network-helper /usr/local/share/fonts/noto; do
	case $old in
	"$helper" | "$fonts") continue ;;
	esac
	! has "$old" || fail "names $old"
done
! has "$destdir" || fail "names DESTDIR, $destdir"

[ "$failures" -eq 0 ] || exit 1
echo "binary paths: ok"
