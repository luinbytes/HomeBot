#!/bin/sh
set -eu

package=${1:?usage: verify-arch-package.sh PACKAGE UPDATE_PACKAGE}
update_package=${2:?usage: verify-arch-package.sh PACKAGE UPDATE_PACKAGE}
pacman -Qip "$package" >/dev/null
pacman -Qip "$update_package" >/dev/null
pacman -U --noconfirm "$package"
test -x /usr/bin/homebot-desktop
test -x /usr/bin/homebot-server
test -f /usr/share/applications/dev.homebot.desktop.desktop
test -f /usr/share/icons/hicolor/scalable/apps/homebot.svg
test -f /usr/lib/systemd/user/homebot.service
desktop-file-validate /usr/share/applications/dev.homebot.desktop.desktop
systemd-analyze verify /usr/lib/systemd/user/homebot.service
pacman -U --noconfirm "$update_package"
pacman -Q homebot | grep -q -- '-2$'

data_sentinel=/tmp/homebot-package-user-data
mkdir -p "$data_sentinel"
printf 'preserve\n' > "$data_sentinel/sentinel"
pacman -Rns --noconfirm homebot
test ! -e /usr/bin/homebot-desktop
test ! -e /usr/bin/homebot-server
test "$(cat "$data_sentinel/sentinel")" = preserve
