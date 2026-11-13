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
- [ ] **Identity and devices:** show the active public identity, signer/lock
  status, linked devices, last sync, and a safe way to revoke a device.
- [ ] **Privacy and AI:** choose which folders or sources AI can use, show
  exclusions such as `.no-ai`, configure redaction and retention, and expose
  the active model and whether inference is local.
- [ ] **Software and trust:** link into package management; show trusted
  publishers, package/model updates, and the consequences of trusting or
  removing a publisher.
- [ ] **Storage and sync:** show sync state, local storage use, upload failures,
  and conflict/recovery actions for Blossom files.
- [ ] **Accounts and access:** show the current user/session and provide clear
  lock, sign-out, and identity recovery paths. Avoid exposing secret keys.

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
  local device time. External relay subscription changes require restarting
  `evermore-notifyd`; source toggles and the DND schedule take effect live.
- The project readiness notes identify interruption, unavailable-service,
  keyboard/focus, accessible error recovery, and settings persistence as areas
  that still need end-to-end acceptance.

