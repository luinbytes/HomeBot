# Arch Linux and Omarchy packaging

`PKGBUILD.in` is the AUR-ready release template. `scripts/render-arch-pkgbuild.sh` requires the exact release version and GitHub source-archive SHA-256; it never permits a `SKIP` checksum. The package builds both native Rust binaries and installs the desktop entry, original HomeBot SVG icon, MIT license, documentation, and a systemd user service.

CI creates pkgrel 1 and pkgrel 2 packages in a current `archlinux:base-devel` environment, installs pkgrel 1, verifies the desktop/service layout, upgrades to pkgrel 2, uninstalls, and proves separately created user data survives. Both packages, the rendered PKGBUILD, checksum file, and release manifest are uploaded as workflow artifacts.

The service binds to loopback by default and reads its owner credential through systemd's encrypted credential facility. It deliberately does not embed a token in the unit or an environment file. `server.env` is reserved for non-secret overrides such as an explicit private-network bind and provider paths.

The eframe build enables both Wayland and X11. Graphical launch uses the desktop-session environment. A systemd user manager does not evaluate shell profiles; configure explicit absolute provider executable paths in HomeBot, or add a narrow non-secret `PATH` override to `~/.config/homebot/server.env`.
