# Evermore basics checklist

This is a product checklist for making Evermore's core desktop and control
panel understandable and manageable. It is based on the current prototype and
readiness notes; it is not a claim that these features are release-ready.

## Panel management settings

The Settings tab now includes theme, accent, density, text scale, reduced
motion, save feedback, and reset controls. The remaining settings should use
clear sections, show whether each value is device-local or identity-synced,
and explain what happens when its service is unavailable. See the phased
delivery gates in [`evermore-basics-roadmap.md`](evermore-basics-roadmap.md).

### Essentials

- [ ] **Appearance:** theme, accent colour, comfortable/compact density, text
  size, and reduced motion. Keep system accessibility preferences easy to find.
- [ ] **Desktop behaviour:** panel visibility/position, launcher shortcut,
  default workspace behaviour, and startup applications.
- [ ] **Notifications:** do-not-disturb schedule, quiet hours, per-source
  controls, and whether optional relay notifications are enabled.
- [ ] **Network and relays:** show local relay status; list configured external
  relays; add, remove, reorder, and test a relay; explain offline behaviour.
- [ ] **Identity and devices:** show the active public identity and editable
  public NIP-01 profile, signer/lock status, linked devices, last sync, and a
  safe way to revoke a device.
- [ ] **Privacy and AI:** choose which folders or sources AI can use, show
  exclusions such as `.no-ai`, configure redaction and retention, and expose
  the active model and whether inference is local.
- [ ] **Software and trust:** link into package management; show trusted
  publishers and a read-only package/model update check, with the consequences
  of trusting or removing a publisher.
- [ ] **Storage and sync:** show sync state, local storage use, upload failures,
  and conflict/recovery actions for Blossom files.
- [ ] **Accounts and access:** show the current user/session and provide clear
  lock, sign-out, and identity recovery paths. Avoid exposing secret keys.
- [ ] **Language and region:** choose an installed operating-system locale,
  preview regional date/time conventions, and state which apps and users are
  affected. Keep the Evermore display language explicit and fall back safely.
- [ ] **Date and time:** show current local time, time zone, and automatic
  synchronization status; allow authorized time-zone and synchronization
  changes, sync these preferences by identity, and explain how they affect
  local schedules such as quiet hours.
- [ ] **Power and devices:** surface battery/power status and provide working
  links to network, audio, display, Bluetooth, and screen-lock controls.

### Every setting should

- [ ] Save with an explicit success or error state and persist after restart.
- [ ] Say whether it syncs to other devices or only affects this machine.
- [ ] Offer a reset-to-default action and confirmation for consequential
  changes.
- [ ] Work with keyboard navigation and visible focus; label controls clearly.
- [ ] Explain service outages in plain language and provide retry/recovery.
- [ ] Keep dangerous actions attributable and require deliberate approval.

## Everyday desktop basics

- [ ] **First run:** guide the user through identity creation/import, optional
  phone signer pairing, network setup, and a short explanation of what syncs.
- [ ] **Status at a glance:** indicate network, relay, signer, sync, AI/model,
  and package update health without requiring log inspection.
- [ ] **Core apps:** provide working browser, files, terminal, settings,
  software, and help entry points, using established Linux applications.
- [ ] **System controls:** make Wi-Fi/network, audio, display, Bluetooth,
  battery/power, and screen lock discoverable through familiar desktop tools.
- [ ] **Updates and recovery:** explain available updates, failures, rollback or
  retry options, and how to recover from interrupted setup or installs.
- [ ] **Help and diagnostics:** include a searchable help/troubleshooting
  surface and a user-controlled way to export useful logs without secrets.
- [ ] **Accessibility and localization:** support scaling, keyboard-only use,
  screen readers where supported, high contrast/reduced motion, and translated
  interface text as the project matures.

## Suggested order

1. Make the current Settings tab a real settings home: sections, persistence
   feedback, reset, and clear local-versus-synced labels.
2. Add the settings most needed to operate the prototype: relays/network,
   notifications/DND, identity/signer status, and AI privacy/model status.
3. Add desktop comfort and accessibility controls: density, text size,
   reduced motion, shortcuts, and startup behaviour.
4. Surface package trust, sync health, diagnostics, and recovery links.
5. Validate each control through restart, offline/service-failure, keyboard,
   and multi-device scenarios before calling it supported.

## Already present in the prototype

- The panel has Settings, Notifications, identity, approvals, AI, and file
  search surfaces. Package management is handed off to `npack-gui`.
- Appearance is stored through `evermore-settingsd` and syncs by identity.
  The panel now exposes theme, accent, density, text scale, reduced motion,
  save feedback, and reset controls, alongside its technical debug disclosure.
- Settings now includes external relay add/remove/reorder and connection
  checks, source-level notification controls, and scheduled quiet hours using
  local device time. The panel now offers to restart `evermore-notifyd` after
  relay changes; source toggles and the DND schedule take effect live.
- AI context already has source allowlisting, one-turn/pinned scopes,
  `.no-ai` enforcement, redaction, and local model selection. Settings now
  shows the active identity's trusted publisher list, confirms add/revoke
  actions, and opens the existing software manager. AI conversation retention
  is selected from identity-synced Settings and applied to local session data.
  Settings checks package/model updates through the local operations service,
  reports available versions, revoked releases, and resolver failures, and
  leaves installation behind its existing approval flow. Phase acceptance
  remains open.
- The engineering image now packages Firefox, NetworkManager's connection
  editor, Pavucontrol, Blueman, Wdisplays, Swaylock, and Brightnessctl.
  Waybar opens network/audio tools; the Sway profile binds hardware keys for
  lock, brightness, and audio. Firefox is the system XDG browser default.
- Settings now shows the current Linux user, confirmed lock/sign-out actions,
  generated system locales, installed time zones, automatic time-sync state,
  and entry points to upstream network/audio/display/Bluetooth tools. The
  engineering image generates `en_US.UTF-8` and starts the LXQt Polkit agent
  for authorized system changes. Locale and time-zone preferences use the
  identity settings relay domain; automatic clock-sync preference also syncs.
  Applying these choices remains a confirmed OS action on each device.
  Evermore itself remains English-only; additional Linux
  account management and graphical power-policy settings remain open.
- The panel has searchable setup/recovery guidance and an opt-in diagnostics
  export. The report contains kernel details and normalized Evermore service
  states; it excludes logs, relay addresses, keys, and message content.
- Settings can inspect the per-identity Blossom sync systemd service, open its
  recent journal output, and explain the `BLOSSOM_URL` setup requirement. It
  distinguishes a missing unit from a running or failed unit, and reports the
  local `~/Nostr` file count and size, and recent upload/download failures from
  the service journal. Durable per-file outcomes and conflict recovery remain
  open.
- The project readiness notes identify interruption, unavailable-service,
  keyboard/focus, accessible error recovery, and settings persistence as areas
  that still need end-to-end acceptance.

