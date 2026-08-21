#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
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
runuser -u builder -- "$script_dir/process-resource-budget.sh" /usr/bin/homebot-server
pacman -U --noconfirm "$update_package"
pacman -Q homebot | grep -q -- '-2$'

data_sentinel=/home/builder/.local/share/homebot
install -d -o builder -g builder -m 700 "$data_sentinel"
runuser -u builder -- sh -c 'printf "preserve\n" > "$HOME/.local/share/homebot/sentinel"'
pacman -Rns --noconfirm homebot
test ! -e /usr/bin/homebot-desktop
test ! -e /usr/bin/homebot-server
test "$(cat "$data_sentinel/sentinel")" = preserve
