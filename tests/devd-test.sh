#!/bin/sh
#
# Checks an installed devd rule. devd runs only the first matching rule, so
# each LINK_UP rule, one for each media type /etc/devd/dhclient.conf has,
# starts dhclient as that file's rules do, then runs card-back from the helper
# under PREFIX; and no LINK_UP rule goes without a media type. Reads the rule;
# runs none of its actions.
#
#   tests/devd-test.sh <rule> <prefix>
#
# SPDX-License-Identifier: BSD-2-Clause

set -eu
[ $# -eq 2 ] || {
	echo "usage: tests/devd-test.sh rule prefix" >&2
	exit 2
}
rule=$1
prefix=$2
[ -f "$rule" ] && [ -r "$rule" ] || {
	echo "devd-test: can't read $rule" >&2
	exit 1
}
failures=0

fail() {
	echo "devd-test: $*" >&2
	failures=$((failures + 1))
}

rules=$(awk '
/^notify / { block = 1; type = media = action = ""; next }
block && /match "type"/ { type = $3; gsub(/[";]/, "", type) }
block && /media-type/ { media = $2; gsub(/[";]/, "", media) }
block && /action / { sub(/^[[:space:]]*action "/, ""); sub(/";$/, ""); action = $0 }
block && /^};/ { block = 0; print type "|" media "|" action }
' "$rule")
for media in ethernet 802.11; do
	echo "$rules" | grep -qF "LINK_UP|$media|service dhclient quietstart \$subsystem; $prefix/libexec/hyprbsd-network-helper card-back \$subsystem " ||
		fail "$rule: no LINK_UP rule for $media that starts dhclient, then card-back: $(echo $rules)"
done
! echo "$rules" | grep -q '^LINK_UP||' || fail "$rule: a LINK_UP rule without a media type: $(echo $rules)"

[ "$failures" -eq 0 ] || exit 1
echo "devd rule: ok"
