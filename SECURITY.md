# Security

hyprbsd-network's helper runs as root and writes rc.conf, wpa_supplicant's file and the DNS
hooks netif runs, so a bug in it can cut a machine off its network or give someone root. Please
report anything that lets it write outside the lines it owns, run with arguments or input it
shouldn't accept, or be reached by someone polkit shouldn't allow; or that lets the window or
command reach wpa_supplicant's control socket, or read a saved network's key, without being in
`wheel`.

## Reporting a problem

Don't open a public issue. Report it privately, either:

- through GitHub: the repository's **Security** tab, **Report a vulnerability**; or
- by email to security@hyprbsd.org.

Please include the FreeBSD version (`freebsd-version -ku`), hyprbsd-network's version (on
HyprBSD, `pkg info hyprbsd-network`; built from source, the tag or commit), and the steps that
show the problem.

## Supported versions

Fixes go into the latest release.
