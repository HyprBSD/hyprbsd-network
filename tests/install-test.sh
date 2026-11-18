#!/bin/sh
#
# Checks make install and make dist: install copies a stand-in binary from
# CARGO_TARGET_DIR, with the helper, devd rule, manual page and polkit files,
# under DESTDIR and PREFIX, %%PREFIX%% filled in and cargo never run;
# tests/devd-test.sh checks each installed rule; dist makes
# hyprbsd-network-<version>.tar.gz from the version's tag. Then make,
# check-cargo and install in turn under other prefixes, with cargo, on a
# crate of this one's build.rs, src/paths.rs and tests/paths.rs: the
# installed program prints its two paths. Runs as anyone.
#
#   tests/install-test.sh
#
# SPDX-License-Identifier: BSD-2-Clause

set -eu
top=$(cd "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d -t network-install) && [ -d "$tmp" ] || exit 1
trap 'rm -rf "$tmp"' EXIT

failures=0
check() {
	if eval "$2"; then
		echo "ok    $1"
	else
		echo "FAIL  $1"
		failures=$((failures + 1))
	fi
}

mkdir -p "$tmp/target/release"
printf '#!/bin/sh\necho hyprbsd-network\n' >"$tmp/target/release/hyprbsd-network"

# submake [variable=value...] make [arguments]: make without the calling
# make's variables (MAKEFLAGS) or PREFIX, LOCALBASE, DESTDIR and
# CARGO_TARGET_DIR from the environment; the assignments go into its
# environment.
submake() {
	env -u MAKEFLAGS -u MFLAGS -u PREFIX -u LOCALBASE -u DESTDIR -u CARGO_TARGET_DIR "$@"
}
# install dir [make arguments]: make install into $tmp/dir, under /usr/local
# unless the arguments say otherwise, with cargo replaced by false.
install_to() {
	dir=$1
	shift
	submake make -C "$top" CARGO=false CARGO_TARGET_DIR="$tmp/target" DESTDIR="$tmp/$dir" \
		PREFIX=/usr/local LOCALBASE=/usr/local "$@" install >"$tmp/$dir.log" 2>&1
}
# lacks string file...: every file is there and none has the string.
lacks() {
	s=$1
	shift
	st=0
	grep -rqF -e "$s" "$@" || st=$?
	[ "$st" -eq 1 ]
}
# files dir: the files installed under $tmp/dir, relative to it.
files() {
	(cd "$tmp/$1" && find . -type f | sort)
}
# expected prefix: the files make install puts under PREFIX.
expected() {
	for f in bin/hyprbsd-network etc/devd/hyprbsd-network.conf libexec/hyprbsd-network-helper \
		share/applications/hyprbsd-network.desktop \
		share/icons/hicolor/scalable/apps/hyprbsd-network.svg share/man/man8/hyprbsd-network.8 \
		share/polkit-1/actions/org.hyprbsd.network.policy \
		share/polkit-1/rules.d/50-hyprbsd-network.rules; do
		echo ".$1/$f"
	done | sort
}
# filled dir prefix: the three files %%PREFIX%% is filled in.
filled() {
	echo "$tmp/$1$2/etc/devd/hyprbsd-network.conf $tmp/$1$2/share/man/man8/hyprbsd-network.8" \
		"$tmp/$1$2/share/polkit-1/actions/org.hyprbsd.network.policy"
}

expected /usr/local >"$tmp/expected"
expected /opt/network >"$tmp/expected-opt"

check "install: copies what make built, without cargo" "install_to default"
check "every file under DESTDIR/usr/local, and nothing else" "files default | cmp -s - '$tmp/expected'"
d=$tmp/default/usr/local
check "the binary is the built one, the helper the source's, both mode 555" \
	"cmp -s '$tmp/target/release/hyprbsd-network' '$d/bin/hyprbsd-network' &&
	cmp -s '$top/libexec/hyprbsd-network-helper' '$d/libexec/hyprbsd-network-helper' &&
	[ \"\$(stat -f %Lp '$d/bin/hyprbsd-network' '$d/libexec/hyprbsd-network-helper' | sort -u)\" = 555 ]"
check "the rest mode 444" \
	"[ \"\$(find '$d/share' '$d/etc' -type f -exec stat -f %Lp {} + | sort -u)\" = 444 ]"
check "the policy names the helper under PREFIX for each of its 5 actions" \
	"[ \"\$(grep -c '>/usr/local/libexec/hyprbsd-network-helper<' '$d/share/polkit-1/actions/org.hyprbsd.network.policy')\" = 5 ]"
check "the devd rule names it under PREFIX in each of its 4 actions" \
	"[ \"\$(grep -c 'action \".*/usr/local/libexec/hyprbsd-network-helper ' '$d/etc/devd/hyprbsd-network.conf')\" = 4 ]"
check "and its LINK_UP rules pass tests/devd-test.sh" \
	"'$top/tests/devd-test.sh' '$d/etc/devd/hyprbsd-network.conf' /usr/local >'$tmp/devd-default.log' 2>&1"
check "the manual page names the helper there" \
	"grep -q '^\.Pa /usr/local/libexec/hyprbsd-network-helper ,\$' '$d/share/man/man8/hyprbsd-network.8'"
check "no %%PREFIX%% left in the filled files" "lacks %% $(filled default /usr/local)"
check "no installed file names DESTDIR" "lacks '$tmp/default' '$tmp/default'"
check "the templates in the source tree are left alone" \
	"grep -q %%PREFIX%% '$top/hyprbsd-network.8.in' && grep -q %%PREFIX%% '$top/polkit/org.hyprbsd.network.policy.in' &&
	grep -q %%PREFIX%% '$top/devd/hyprbsd-network.conf.in'"
check "installing again over the first install works" "install_to default"

check "PREFIX and LOCALBASE apart, on make's command line" \
	"install_to opt PREFIX=/opt/network LOCALBASE=/opt/packages"
check "every file under DESTDIR/opt/network, and nothing else" "files opt | cmp -s - '$tmp/expected-opt'"
o=$tmp/opt/opt/network
check "the policy, rule and page name the helper under that PREFIX, and nothing under /usr/local" \
	"[ \"\$(grep -c '>/opt/network/libexec/hyprbsd-network-helper<' '$o/share/polkit-1/actions/org.hyprbsd.network.policy')\" = 5 ] &&
	[ \"\$(grep -c 'action \".*/opt/network/libexec/hyprbsd-network-helper ' '$o/etc/devd/hyprbsd-network.conf')\" = 4 ] &&
	grep -q '^\.Pa /opt/network/libexec/hyprbsd-network-helper ,\$' '$o/share/man/man8/hyprbsd-network.8' &&
	lacks /usr/local $(filled opt /opt/network)"
check "none of them names LOCALBASE" "lacks /opt/packages $(filled opt /opt/network)"
check "its LINK_UP rules pass tests/devd-test.sh for that PREFIX" \
	"'$top/tests/devd-test.sh' '$o/etc/devd/hyprbsd-network.conf' /opt/network >'$tmp/devd-opt.log' 2>&1"
check "no installed file names DESTDIR" "lacks '$tmp/opt' '$tmp/opt'"

check "PREFIX from the environment, as the ports framework passes it" \
	"submake PREFIX=/opt/env make -C '$top' CARGO=false CARGO_TARGET_DIR='$tmp/target' DESTDIR='$tmp/env' install \
	>'$tmp/env.log' 2>&1 && [ -f '$tmp/env/opt/env/bin/hyprbsd-network' ] &&
	grep -q '>/opt/env/libexec/hyprbsd-network-helper<' '$tmp/env/opt/env/share/polkit-1/actions/org.hyprbsd.network.policy'"
check "a calling make's PREFIX and LOCALBASE, and the environment's, don't move an install" \
	"(export MAKEFLAGS='PREFIX=/opt/caller LOCALBASE=/opt/caller' PREFIX=/opt/callerenv LOCALBASE=/opt/callerenv &&
	install_to caller) && files caller | cmp -s - '$tmp/expected' &&
	grep -q '>/usr/local/libexec/hyprbsd-network-helper<' '$tmp/caller/usr/local/share/polkit-1/actions/org.hyprbsd.network.policy'"

# A rule whose ethernet LINK_UP rule runs card-back before dhclient.
sed '/media-type.*"ethernet"/,/^};/s|action "service dhclient quietstart $subsystem; \(.*\)";|action "\1; service dhclient quietstart $subsystem";|' \
	"$d/etc/devd/hyprbsd-network.conf" >"$tmp/swapped.conf"
check "devd-test.sh fails a LINK_UP rule that runs card-back first" \
	"! cmp -s '$d/etc/devd/hyprbsd-network.conf' '$tmp/swapped.conf' &&
	! '$top/tests/devd-test.sh' '$tmp/swapped.conf' /usr/local >'$tmp/devd-swapped.log' 2>&1 &&
	grep -q 'no LINK_UP rule for ethernet ' '$tmp/devd-swapped.log' &&
	! grep -q 'no LINK_UP rule for 802.11 ' '$tmp/devd-swapped.log'"
sed '/media-type.*"802.11"/d' "$d/etc/devd/hyprbsd-network.conf" >"$tmp/nomedia.conf"
check "and a LINK_UP rule without a media type" \
	"! '$top/tests/devd-test.sh' '$tmp/nomedia.conf' /usr/local >'$tmp/devd-nomedia.log' 2>&1 &&
	grep -q 'a LINK_UP rule without a media type' '$tmp/devd-nomedia.log'"

rm "$tmp/target/release/hyprbsd-network"
check "with nothing built, install fails" "! install_to missing"

# A crate named hyprbsd-network of this one's build.rs, src/paths.rs and
# tests/paths.rs, with a main that prints HELPER and FONTS.
fx=$tmp/fixture
mkdir -p "$fx/src" "$fx/tests"
cp "$top/build.rs" "$top/rustfmt.toml" "$fx/"
cp "$top/src/paths.rs" "$fx/src/"
cp "$top/tests/paths.rs" "$fx/tests/"
printf '%s\n' '[package]' 'name = "hyprbsd-network"' 'version = "0.0.0"' 'edition = "2024"' 'publish = false' \
	>"$fx/Cargo.toml"
echo 'pub mod paths;' >"$fx/src/lib.rs"
printf '%s\n' 'use hyprbsd_network::paths::{FONTS, HELPER};' '' 'fn main() {' '    println!("{HELPER}");' \
	'    println!("{FONTS}");' '}' >"$fx/src/main.rs"
# fixture_make [make arguments]: this Makefile on the fixture, under
# /opt/fixture and /opt/fixture-base, its builds in the fixture.
fixture_make() {
	submake make -C "$top" MANIFEST="$fx/Cargo.toml" CARGO_TARGET_DIR="$fx/target" \
		OTHER_TARGET="$fx/target/other" PREFIX=/opt/fixture LOCALBASE=/opt/fixture-base "$@" \
		>>"$tmp/fixture.log" 2>&1
}
check "make, then check-cargo, then install, under other prefixes: the installed program names them" \
	"fixture_make all && fixture_make check-cargo && fixture_make DESTDIR='$tmp/fixture-dest' install &&
	[ \"\$('$tmp/fixture-dest/opt/fixture/bin/hyprbsd-network' | paste -s -d ' ' -)\" = \
	'/opt/fixture/libexec/hyprbsd-network-helper /opt/fixture-base/share/fonts/noto' ]"

# A repository with one commit, tagged with its Cargo.toml's version.
repo=$tmp/repo
mkdir -p "$repo"
printf '%s\n' '[package]' 'name = "hyprbsd-network"' 'version = "9.8.7"' >"$repo/Cargo.toml"
echo committed >"$repo/README.md"
git_repo() {
	git -C "$repo" -c user.name=test -c user.email=test@example.org "$@"
}
git_repo init -q
git_repo add -A
git_repo commit -q -m test
git_repo tag v9.8.7
echo changed >"$repo/README.md"
dist() {
	submake make -C "$repo" -f "$top/Makefile" CARGO_TARGET_DIR="$tmp/dist" dist >"$tmp/dist.log" 2>&1
}
tarball=$tmp/dist/hyprbsd-network-9.8.7.tar.gz
check "dist: hyprbsd-network-<version>.tar.gz" "dist && [ -f '$tarball' ]"
check "its files under hyprbsd-network-<version>/" \
	"[ \"\$(tar -tzf '$tarball' | sort | tr '\n' ' ')\" = 'hyprbsd-network-9.8.7/ hyprbsd-network-9.8.7/Cargo.toml hyprbsd-network-9.8.7/README.md ' ]"
check "as the tag has them, not the working tree" \
	"[ \"\$(tar -xOzf '$tarball' hyprbsd-network-9.8.7/README.md)\" = committed ]"
sed -i '' 's/9\.8\.7/9.8.8/' "$repo/Cargo.toml"
check "a version without its tag fails" "! dist"

[ "$failures" -eq 0 ] || {
	echo "install-test: $failures failed" >&2
	exit 1
}
echo "install: ok"
