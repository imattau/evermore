# Application readiness

This is the working support contract for Phase 8.5. The current repository is
an early prototype, so **implemented** means a feature exists and has some
engineering evidence; it does not mean the feature is supported for daily use.
Until a row passes its acceptance criteria on the documented environment, treat
it as experimental.

## Initial application inventory

| App or flow | Current state and evidence | Readiness gap | Acceptance for supported status |
| --- | --- | --- | --- |
| `evermore-greeter` + `evermore-enroll` | First-user wizard, homed account creation, passphrase unlock, optional NIP-46/PKCS#11 pairing, and Sway handoff are implemented. VM image boot reached the greeter. | Fresh-install flow with a real phone signer, interrupted enrollment recovery, and key cleanup on logout are not accepted. A post-create error now attempts to remove the account created by that enrollment transaction; power loss during enrollment still needs recovery handling. | Fresh machine with zero users can create/import an identity, reboot, unlock and start Sway; wrong credentials and an interrupted setup are recoverable without touching another account; logout removes session key material. Repeat local-key and phone-pairing paths. |
| `evermore-signer` and identity services | Unix-socket signing, identity loading, and session-keyring handling have VM development coverage. | Recovery, key lifetime across all logout/session paths, and fresh-install behavior are not yet release accepted. | Verify socket ownership and caller boundary, sign/encrypt/decrypt through supported clients, restart and relock behavior, and key revocation after the final session ends. No client receives the secret key. |
| `evermore-panel` settings and identity controls | Panel integrates settings, identity, operation approvals, package management, notifications, and AI surfaces. Feature-specific VM checks exist. | No end-to-end support contract yet; interruption, unavailable-service states, keyboard/focus usability, and accessible error recovery need consistent review. | Each exposed control has a clear state, actionable error, owning service, and recovery path. Settings persist over restart; sensitive actions are attributable and explicitly approved. |
| Notifications and approvals | SwayNC owns native notifications; VM scripts cover approval deep links and inbox de-duplication. | Needs repeated-session, restart, DND, unavailable relay/service, and stale-approval behavior acceptance. | Local notifications and pending approvals appear once, open the correct review screen, preserve explicit owner approval, and recover cleanly after service/compositor restart. Optional Nostr sources can be disabled without losing local desktop notifications. |
| Local AI, context, and file search | Offline inference, semantic search, redaction, `.no-ai` exclusion, cancellation, and approval flows have focused VM checks. | Release evaluation is not complete; responsiveness and graceful behavior under memory pressure, unavailable model, cancellation, and stale context need acceptance. | Core desktop remains usable with inference stopped. Context scope and redaction are visible; excluded data never enters a request; model errors/cancellation do not hang the panel; writes remain approval-gated. |
| `npack` CLI and software UI | Trust-gated discovery, install/remove, and panel flow were exercised end to end in a VM; the engineering image includes upstream `npack`/`npack-gui`. | Upgrade/rollback, interrupted install, disk-full, invalid signature/hash, trust removal, and clear capability prompts need systematic acceptance. Default trust UX/config handoff needs to be discoverable. | Install/update/remove a trusted package and model; refuse untrusted or corrupt artifacts; recover from interrupted operations without an ambiguous installed state; explain trust and service-manager capability decisions. |
| Sway desktop, launcher, and ordinary apps | Sway/Waybar/`evermore-launcher`/Fuzzel/SwayNC configuration and keyboard launch paths have disposable-VM coverage. The eGUI launcher is packaged as `launcher` and included in the default image. | Current VM is headless and does not establish hardware coverage, multi-session reliability, portal/polkit behavior, suspend/resume, or a complete everyday app set. | On a documented reference device, log in, launch/focus/close ordinary apps, use audio/network/clipboard/file chooser/notifications, and recover from compositor and user-service restarts. |
| Blossom sync, mount, and file actions | Same-VM sync and mount checks passed; Nautilus publish and Send-to-AI hooks exist. | The two-device acceptance is still outstanding; offline conflicts, deletion recovery, and upload failures need clear user-facing handling. | Verify sync across two devices, reconcile concurrent changes safely, and ensure network loss or remote deletion never silently loses the only local copy. |

## Supported desktop boundary

The intended baseline is the upstream Sway desktop and ordinary Linux apps,
with Evermore-specific identity, trust, approvals, software, notifications,
and AI context supplied by Evermore services. Do not build replacement office,
browser, terminal, media, or file-manager applications as part of readiness
work. Promote a feature from experimental only when its user flow, failure
behavior, and recovery are accepted; update this table with the environment and
evidence at that time.

## Daily-use scenario

Run this scenario on a clean user profile and repeat it after reboot:

1. Enroll or unlock an identity and enter the Sway session.
2. Launch and switch between ordinary desktop applications from the launcher.
3. Change a setting, receive a local notification, and review a pending
   approval without approving it accidentally.
4. Lock/unlock the signer, perform one explicitly approved local AI action, and
   confirm that unavailable inference does not break the rest of the desktop.
5. Install and remove a signed package through the software UI, then repeat
   with an unavailable network/relay and confirm the result is clear and
   recoverable.
6. Interrupt enrollment and a package operation in disposable test state;
   verify recovery preserves unrelated accounts and user data.

Record the hardware/VM configuration, package and model versions, checks run,
failures found, and unresolved non-blocking issues alongside the results. A
green build alone is not acceptance evidence.
