#!/usr/bin/env bash
# Install staged Evermore release artifacts into a clean Arch guest.
set -euo pipefail

STAGE="${1:?usage: install-image.sh STAGE_DIR}"
MODEL_STORE="${2:?usage: install-image.sh STAGE_DIR MODEL_STORE_DIR}"
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"

bins=(evermore-greeter evermore-enroll evermore-signer evermore-panel evermore-relay-supervisor \
  evermore-contextd evermore-indexd evermore-inferd evermore-npack evermore-operationsd \
  evermore-notifyd evermore-settingsd evermore-blossom-fs evermore-sway-ai-bridge evermore-agentd npack npack-gui)
for bin in "${bins[@]}"; do
  [[ -x "$STAGE/bin/$bin" ]] || { echo "missing staged executable: $STAGE/bin/$bin" >&2; exit 1; }
done
[[ -s "$STAGE/lib/evermore-pkcs11.so" ]] || { echo 'missing staged PKCS#11 module' >&2; exit 1; }
for file in "$STAGE/systemd/user/npackd.service" "$STAGE/share/io.npack.Gui.desktop" \
  "$STAGE/share/io.npack.Gui.svg" "$STAGE/share/io.npack.Gui.metainfo.xml"; do
  [[ -s "$file" ]] || { echo "missing staged npack desktop asset: $file" >&2; exit 1; }
done
for task in mcp panel; do
  model="$MODEL_STORE/p8.model.qwen2.5-1.5b.$task-1.0.0-dev.1"
  [[ -s "$model/base.gguf" && -s "$model/adapter.gguf" && -s "$model/manifest.json" ]] || {
    echo "missing Qwen $task model package: $model" >&2
    exit 1
  }
done
[[ -s "$MODEL_STORE/p7.model.bge-1.0.0/manifest.json" ]] || { echo 'missing BGE manifest' >&2; exit 1; }

install -d /usr/bin /usr/lib/pkcs11 /usr/share/p11-kit/modules /usr/lib/systemd/user \
  /etc/greetd /etc/sudoers.d /etc/skel/.config/sway /etc/skel/.config/waybar \
  /etc/skel/.config/wofi /etc/skel/.config/fuzzel /etc/skel/.config/swaync \
  /etc/skel/.config/evermore /etc/skel/.config/npack \
  /etc/skel/.local/share/applications /var/lib/evermore-profiles \
  /usr/share/applications /usr/share/icons/hicolor/scalable/apps /usr/share/metainfo
for bin in "${bins[@]}"; do install -m 0755 "$STAGE/bin/$bin" "/usr/bin/$bin"; done

# Register Evermore's own daemons/apps as real npack packages in every new
# user's skeleton home, so npack-gui has an honest record of them instead of
# showing nothing installed while the binaries sit on disk only because the
# loop above copied them there directly. Local installs need no signing or
# network (see packaging/npack/pack-for-image.sh for why), so this is pure
# bookkeeping: it doesn't touch the live /usr/bin copies or systemd units
# those already-working binaries and evermore-enroll wire up.
NPK_WORK="$(mktemp -d)"
trap 'rm -rf "$NPK_WORK"' EXIT
# Not mcp: unlike every other package build-and-publish.sh knows about,
# evermore-mcp is never staged into this image at all (it's spawned
# per-session over stdio by an external MCP client's own config, not run as
# a daemon here) -- packaging a binary that was never copied to /usr/bin
# would be registering something this image doesn't actually ship.
for package in settingsd notifyd blossom-fs contextd indexd inferd operationsd agentd sway-ai-bridge panel; do
  npk="$NPK_WORK/evermore-$package-0.1.0.npk"
  NPACK_BIN=/usr/bin/npack "$ROOT/packaging/npack/pack-for-image.sh" "$package" 0.1.0 "$ROOT" "$STAGE/bin" "$npk"
  HOME=/etc/skel XDG_DATA_HOME= /usr/bin/npack install "$npk" --user
done
rm -rf "$NPK_WORK"
trap - EXIT
chown -R root:root /etc/skel/.local
find /etc/skel/.local -type d -exec chmod 0755 {} +
find /etc/skel/.local -type f -exec chmod 0644 {} +
find /etc/skel/.local/bin -type f -exec chmod 0755 {} +

install -m 0644 "$STAGE/share/io.npack.Gui.desktop" /usr/share/applications/
install -m 0644 "$STAGE/share/io.npack.Gui.svg" /usr/share/icons/hicolor/scalable/apps/
sed -e 's/@VERSION@/0.9.4/' -e "s/@DATE@/$(date -u +%F)/" \
  "$STAGE/share/io.npack.Gui.metainfo.xml" > /usr/share/metainfo/io.npack.Gui.metainfo.xml
install -m 0644 "$STAGE/systemd/user/npackd.service" /usr/lib/systemd/user/
install -m 0644 "$ROOT/packaging/npack/default-config.toml" /etc/skel/.config/npack/config.toml
desktop-file-validate /usr/share/applications/io.npack.Gui.desktop
update-desktop-database /usr/share/applications
install -m 0755 "$STAGE/lib/evermore-pkcs11.so" /usr/lib/pkcs11/evermore-pkcs11.so
# Not /var/lib/evermore/models: evermore-pkcs11 and evermore-signer's
# device-identity bootstrap both reset /var/lib/evermore to 0700 on every
# run, which would lock every non-root reader (evermore-inferd included)
# out of a models/ subdirectory nested inside it.
install -d -m 0755 /var/lib/evermore-models
cp -a "$MODEL_STORE/." /var/lib/evermore-models/
chown -R root:root /var/lib/evermore-models
find /var/lib/evermore-models -type d -exec chmod 0755 {} +
find /var/lib/evermore-models -type f -exec chmod 0644 {} +
printf '{\n  "model": "evermore/p8.model.qwen2.5-1.5b.panel:1.0.0-dev.1"\n}\n' \
  > /etc/skel/.config/evermore/panel-ai.json
chmod 0644 /etc/skel/.config/evermore/panel-ai.json
install -m 0644 "$ROOT/packaging/pkcs11/evermore-pkcs11.module" /usr/share/p11-kit/modules/
install -m 0644 "$ROOT/packaging/greetd/config.toml" /etc/greetd/config.toml
install -m 0440 "$ROOT/packaging/greetd/evermore-first-user.sudoers" /etc/sudoers.d/evermore-first-user
visudo -cf /etc/sudoers.d/evermore-first-user
# Numbered low so it sorts (and is processed) before any later, more
# specific drop-in -- sudo's rule is "last match wins" per @includedir's
# alphabetical file order, not "most specific wins". On the provisioning
# VM this matters concretely: cloud-init's own 90-cloud-init-users grants
# `builder` NOPASSWD sudo, builder is *also* in group wheel (the base
# cloud image's own default), and build-image.sh's final step depends on
# that NOPASSWD surviving to remove the builder account non-interactively
# and power off. A same-or-higher-sorting %wheel rule silently overrides
# it (verified: that step failed with "sudo: a password is required"
# until this was renumbered), leaving builder's account, home and SSH key
# in the shipped image instead of a real first-boot setup.
install -m 0440 "$ROOT/packaging/greetd/evermore-wheel.sudoers" /etc/sudoers.d/10-evermore-wheel
visudo -cf /etc/sudoers.d/10-evermore-wheel
install -m 0644 "$ROOT/packaging/systemd/evermore-first-user.service" /usr/lib/systemd/system/
install -m 0644 "$ROOT/packaging/systemd/evermore-signer.service" /usr/lib/systemd/user/
install -m 0644 "$ROOT/packaging/systemd/evermore-relay.service" /usr/lib/systemd/user/
install -m 0644 "$ROOT/packaging/systemd/evermore-contextd.service" /usr/lib/systemd/user/
install -m 0644 "$ROOT/packaging/systemd/evermore-indexd.service" /usr/lib/systemd/user/
install -m 0644 "$ROOT/packaging/systemd/evermore-inferd.service" /usr/lib/systemd/user/
install -m 0644 "$ROOT/packaging/systemd/evermore-sway-ai-bridge.service" /usr/lib/systemd/user/
for unit in evermore-blossom-fs@.service evermore-notifyd@.service evermore-operationsd@.service \
  evermore-settingsd@.service evermore-agentd@.service; do install -m 0644 "$ROOT/packaging/systemd/$unit" /usr/lib/systemd/user/; done
install -m 0644 "$ROOT/packaging/systemd/evermore-profiles.conf" /usr/lib/tmpfiles.d/evermore-profiles.conf

install -m 0644 "$ROOT/packaging/sway/evermore-desktop.conf" /etc/skel/.config/sway/
install -m 0644 "$ROOT/packaging/sway/evermore-waybar.conf" /etc/skel/.config/sway/
install -m 0644 /etc/sway/config /etc/skel/.config/sway/config
install -m 0644 "$ROOT/packaging/waybar/config" /etc/skel/.config/waybar/
install -m 0644 "$ROOT/packaging/waybar/style.css" /etc/skel/.config/waybar/
install -m 0644 "$ROOT/packaging/wofi/config" /etc/skel/.config/wofi/
install -m 0644 "$ROOT/packaging/wofi/style.css" /etc/skel/.config/wofi/
install -m 0644 "$ROOT/packaging/fuzzel/fuzzel.ini" /etc/skel/.config/fuzzel/
install -m 0644 "$ROOT/packaging/swaync/config.json" /etc/skel/.config/swaync/
install -m 0644 "$ROOT/packaging/swaync/style.css" /etc/skel/.config/swaync/
for file in "$ROOT"/packaging/applications/*.desktop; do install -m 0644 "$file" /etc/skel/.local/share/applications/; done
printf '\ninclude ~/.config/sway/evermore-desktop.conf\ninclude ~/.config/sway/evermore-waybar.conf\n' \
  >> /etc/skel/.config/sway/config
install -d -m 0700 /tmp/evermore-sway-config-check
HOME=/etc/skel XDG_RUNTIME_DIR=/tmp/evermore-sway-config-check \
  WLR_BACKENDS=headless WLR_RENDERER=pixman \
  sway --validate --config /etc/skel/.config/sway/config
rm -rf /tmp/evermore-sway-config-check
install -d /usr/lib/systemd/user/default.target.wants
for unit in evermore-signer.service evermore-relay.service evermore-contextd.service \
  evermore-indexd.service evermore-inferd.service evermore-sway-ai-bridge.service npackd.service; do
  ln -sfn "../$unit" "/usr/lib/systemd/user/default.target.wants/$unit"
done
chmod 1777 /var/lib/evermore-profiles
systemctl enable systemd-homed.service greetd.service NetworkManager.service seatd.service
systemctl enable evermore-first-user.service
systemctl set-default graphical.target

# Remove cloud-image provisioning and access so first boot is a real product
# setup: no builder user, SSH key, or cloud-init account survives in the image.
cloud-init clean --logs || true
systemctl disable cloud-init-local.service cloud-init.service cloud-config.service cloud-final.service sshd.service || true
pacman -Rns --noconfirm cloud-init || true
rm -rf /root/.ssh /etc/ssh/ssh_host_* /var/lib/cloud /var/log/cloud-init*
rm -f /etc/machine-id
touch /etc/machine-id
rm -rf /var/lib/systemd/random-seed
sync
