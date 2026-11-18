# hyprbsd-network's builds, checks and tests, for FreeBSD's make.
#
#   make                the window and command (cargo)
#   make install        what make built, the root helper, its devd rule, the
#                       manual page, polkit's actions and rule, the menu entry
#                       and the icon, under DESTDIR and PREFIX
#   make dist           hyprbsd-network-<version>.tar.gz in CARGO_TARGET_DIR,
#                       from the tag v<version>
#   make check          check-cargo, then check-files
#   make check-cargo    rustfmt's check, the tests, the paths test again under
#                       other prefixes with tests/binary-paths-test.sh on a
#                       build with them, and clippy
#   make check-files    the manual page, tests/install-test.sh,
#                       tests/helper-test.sh, the bhyve tests' library
#                       (tests/vm/lib-test.sh) and their shell syntax
#   make deny           cargo-deny: the crates' licences, RustSec's
#                       advisories, duplicate versions and sources (deny.toml)
#   make test           the cards in a bhyve guest: DHCP, Manual, off and on
#                       across restarts, the window from the keyboard,
#                       polkit's actions, no Wi-Fi (tests/vm/cards-test.sh)
#   make test-wifi      Wi-Fi in a bhyve guest with this host's Wi-Fi card
#                       lent to it (tests/vm/wifi-test.sh); needs
#                       WIFI_FIRMWARE, the card's firmware package file
#   make clean          cargo's build output
#
# make takes PREFIX, where hyprbsd-network looks for its helper, and
# LOCALBASE, where it finds Noto's fonts, both /usr/local unless set. make
# install takes PREFIX, the same one, and DESTDIR.
#
# The cargo targets run as yourself, the bhyve tests as root through $DOAS.
# They take WORK (where they keep images and guests, /var/hyprbsd-network),
# RELEASE (the guests' FreeBSD, 15.1-RELEASE, of the host's major version),
# VM_MEM, VM_CPUS and CARGO_TARGET_DIR; test-wifi also WIFI_FIRMWARE and
# WIFI_SSID.
#
# SPDX-License-Identifier: BSD-2-Clause

CARGO?=		cargo
DOAS?=		doas
PREFIX?=	/usr/local
LOCALBASE?=	/usr/local
CARGO_TARGET_DIR?=	${.CURDIR}/target
MANIFEST=	${.CURDIR}/Cargo.toml
VM_TESTS=	${.CURDIR}/tests/vm
VERSION!=	sed -n '/^\[package\]/,/^\[/s/^version = "\(.*\)"/\1/p' ${MANIFEST}

# Installed under PREFIX, as path:source.
BIN=		bin/hyprbsd-network
SCRIPTS=	libexec/hyprbsd-network-helper:libexec/hyprbsd-network-helper
SHARE=		share/applications/hyprbsd-network.desktop:hyprbsd-network.desktop \
		share/icons/hicolor/scalable/apps/hyprbsd-network.svg:hyprbsd-network.svg \
		share/polkit-1/rules.d/50-hyprbsd-network.rules:polkit/50-hyprbsd-network.rules
FILLED=		etc/devd/hyprbsd-network.conf:devd/hyprbsd-network.conf.in \
		share/man/man8/hyprbsd-network.8:hyprbsd-network.8.in \
		share/polkit-1/actions/org.hyprbsd.network.policy:polkit/org.hyprbsd.network.policy.in
DIRS=		${BIN:H} ${SCRIPTS:C/:.*//:H} ${SHARE:C/:.*//:H} ${FILLED:C/:.*//:H}

# cargo's environment for a build under PREFIX and LOCALBASE, into CARGO_TARGET_DIR.
BUILD_ENV=	HYPRBSD_NETWORK_PREFIX=${PREFIX} HYPRBSD_NETWORK_LOCALBASE=${LOCALBASE} \
		CARGO_TARGET_DIR=${CARGO_TARGET_DIR}

# check's build under other prefixes, and a DESTDIR the binary mustn't name.
OTHER_PREFIX=	/opt/network
OTHER_LOCALBASE=	/opt/packages
OTHER_DESTDIR=	/nonexistent/hyprbsd-network-destdir
OTHER_ENV=	HYPRBSD_NETWORK_PREFIX=${OTHER_PREFIX} HYPRBSD_NETWORK_LOCALBASE=${OTHER_LOCALBASE} \
		DESTDIR=${OTHER_DESTDIR}
OTHER_TARGET=	${.CURDIR}/target/paths

# Variables passed through doas to the bhyve tests.
TEST_ENV=
.for v in WORK RELEASE VM_MEM VM_CPUS CARGO_TARGET_DIR WIFI_FIRMWARE WIFI_SSID
.if defined(${v})
TEST_ENV+=	${v}=${${v}:Q}
.endif
.endfor

.PHONY: all install dist check check-cargo check-files deny test test-wifi clean

all:
	env ${BUILD_ENV} ${CARGO} build --release --manifest-path ${MANIFEST}

install:
	${INSTALL} -d -m 755 ${DIRS:O:u:S,^,${DESTDIR}${PREFIX}/,}
	${INSTALL} -m 555 ${CARGO_TARGET_DIR}/release/${BIN:T} ${DESTDIR}${PREFIX}/${BIN}
.for f in ${SCRIPTS}
	${INSTALL} -m 555 ${.CURDIR}/${f:C/^[^:]*://} ${DESTDIR}${PREFIX}/${f:C/:.*//}
.endfor
.for f in ${SHARE}
	${INSTALL} -m 444 ${.CURDIR}/${f:C/^[^:]*://} ${DESTDIR}${PREFIX}/${f:C/:.*//}
.endfor
.for f in ${FILLED}
	rm -f ${DESTDIR}${PREFIX}/${f:C/:.*//}
	sed 's|%%PREFIX%%|${PREFIX}|g' ${.CURDIR}/${f:C/^[^:]*://} >${DESTDIR}${PREFIX}/${f:C/:.*//}
	chmod 444 ${DESTDIR}${PREFIX}/${f:C/:.*//}
.endfor

dist:
	mkdir -p ${CARGO_TARGET_DIR}
	git -C ${.CURDIR} archive --format=tar.gz --prefix=hyprbsd-network-${VERSION}/ \
		-o ${CARGO_TARGET_DIR}/hyprbsd-network-${VERSION}.tar.gz v${VERSION}

check: check-cargo .WAIT check-files

check-cargo:
	${CARGO} fmt --check --manifest-path ${MANIFEST}
	env ${BUILD_ENV} ${CARGO} test --release --manifest-path ${MANIFEST}
	env ${OTHER_ENV} ${CARGO} test --manifest-path ${MANIFEST} --test paths --target-dir ${OTHER_TARGET}
	env ${OTHER_ENV} ${CARGO} build --manifest-path ${MANIFEST} --bin hyprbsd-network --target-dir ${OTHER_TARGET}
	${.CURDIR}/tests/binary-paths-test.sh ${OTHER_TARGET}/debug/hyprbsd-network ${OTHER_PREFIX} \
		${OTHER_LOCALBASE} ${OTHER_DESTDIR}
	env ${BUILD_ENV} ${CARGO} clippy --all-targets --locked --manifest-path ${MANIFEST} -- -D warnings

check-files:
	mandoc -T lint -W warning ${.CURDIR}/hyprbsd-network.8.in
	${.CURDIR}/tests/install-test.sh
	${.CURDIR}/tests/helper-test.sh
	${VM_TESTS}/lib-test.sh
	@for f in ${VM_TESTS}/*.sh ${VM_TESTS}/guest/*; do sh -n "$$f" || exit 1; done && \
		echo "bhyve tests' shell syntax: ok"

deny:
	${CARGO} deny --manifest-path ${MANIFEST} check

test: all
	${DOAS} env ${TEST_ENV} ${VM_TESTS}/cards-test.sh

test-wifi: all
	${DOAS} env ${TEST_ENV} ${VM_TESTS}/wifi-test.sh

clean:
	${CARGO} clean --manifest-path ${MANIFEST}
