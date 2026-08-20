# Releasing

No v1 tag is permitted until every required Linear issue is complete and each parity row is Pass on macOS Intel, macOS Apple Silicon, Arch/Omarchy, and Android, except the documented hosted-VM exclusion.

Release CI must build signed/notarised macOS artifacts, Arch/Omarchy packages and headless service assets, and the Android artifact; test clean installation and supported upgrades; produce SHA-256 checksums and a signed release manifest; run security, migration/recovery, protocol, performance, accessibility, provider round-trip, and parity suites; then publish immutable GitHub release artifacts and known limitations.
