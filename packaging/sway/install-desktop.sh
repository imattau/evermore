#!/usr/bin/env bash
# Install Evermore's Sway, Waybar, Fuzzel, and SwayNC user configuration.
# Existing files are backed up before replacement; no packages are installed
# and the user's main Sway config is never edited.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/../.." && pwd)"
CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
BACKUP_SUFFIX="pre-evermore-$(date +%Y%m%d-%H%M%S)"

install_user_file() {
    local source="$1" destination="$2" backup
    if [[ -e "$destination" ]] && cmp -s -- "$source" "$destination"; then
        return
    fi
    if [[ -e "$destination" || -L "$destination" ]]; then
        backup="${destination}.${BACKUP_SUFFIX}"
        cp -a -- "$destination" "$backup"
        printf 'Backed up %s to %s\n' "$destination" "$backup"
    fi
    install -D -m 0644 -- "$source" "$destination"
    printf 'Installed %s\n' "$destination"
}

install_user_file "$REPO_ROOT/packaging/waybar/config" \
    "$CONFIG_HOME/waybar/config"
install_user_file "$REPO_ROOT/packaging/waybar/style.css" \
    "$CONFIG_HOME/waybar/style.css"
install_user_file "$REPO_ROOT/packaging/fuzzel/fuzzel.ini" \
    "$CONFIG_HOME/fuzzel/fuzzel.ini"
install_user_file "$REPO_ROOT/packaging/swaync/config.json" \
    "$CONFIG_HOME/swaync/config.json"
install_user_file "$REPO_ROOT/packaging/swaync/style.css" \
    "$CONFIG_HOME/swaync/style.css"
install_user_file "$REPO_ROOT/packaging/sway/evermore-desktop.conf" \
    "$CONFIG_HOME/sway/evermore-desktop.conf"
install_user_file "$REPO_ROOT/packaging/sway/evermore-waybar.conf" \
    "$CONFIG_HOME/sway/evermore-waybar.conf"
for desktop_file in "$REPO_ROOT"/packaging/applications/*.desktop; do
    install_user_file "$desktop_file" \
        "$DATA_HOME/applications/$(basename -- "$desktop_file")"
done

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$DATA_HOME/applications" >/dev/null
fi

printf '\nInstalled Evermore desktop configuration. Add these lines to %s/sway/config:\n' \
    "$CONFIG_HOME"
printf 'include %s/sway/evermore-desktop.conf\n' "$CONFIG_HOME"
printf 'include %s/sway/evermore-waybar.conf\n' "$CONFIG_HOME"
printf '\nRequired packages (install separately): waybar, fuzzel, swaync, wl-clipboard.\n'
