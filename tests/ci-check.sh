#!/bin/sh
#
# make check as an ordinary user, for a CI guest that runs its job as root:
# makes the user ci (if missing), copies the source tree into its home, and
# runs make check there as ci. Exits with make check's status.
#
#   tests/ci-check.sh
#
# SPDX-License-Identifier: BSD-2-Clause

set -eu
[ "$(id -u)" -eq 0 ] || {
	echo "ci-check: run as root; it runs make check as the user ci" >&2
	exit 1
}
src=$(cd "$(dirname "$0")/.." && pwd)
id ci >/dev/null 2>&1 || pw useradd ci -m -s /bin/sh
home=$(pw usershow ci | cut -d : -f 9)
rm -rf "$home/src"
cp -R "$src" "$home/src"
chown -R ci "$home/src"
su -l ci -c 'set -e; [ "$(id -u)" -ne 0 ]; echo "make check as $(id -un), uid $(id -u)"; cd ~/src; make check'
