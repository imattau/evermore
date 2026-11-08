# Sway desktop bindings

Install `waybar`, `wofi`, `fuzzel`, `swaync`, and `wl-clipboard` from Arch's official
repositories. From the checkout, run:

```bash
./packaging/sway/install-desktop.sh
```

The installer copies the configs and desktop entries to the XDG user
directories, backs up any files it replaces, does not install packages, and
does not edit the main Sway config. It prints the two `include` lines to add at
the **end** of `~/.config/sway/config`, where the launcher bindings override
Sway's defaults:

```conf
include ~/.config/sway/evermore-desktop.conf
include ~/.config/sway/evermore-waybar.conf
```

If you have a customized bar, keep its block and replace its `status_command`
with `swaybar_command waybar` instead; in that case omit the Waybar include.

The desktop entries provide searchable Evermore
Settings, Notifications, AI, and selected-file search entry points. Install
both desktop entries: `org.evermore.SendToAI.desktop` registers a standard
“Send to Evermore AI” Open With action for text files in file managers that
support desktop MIME associations. It opens the AI drawer with the file
attached for one turn, using the same size, format, active-scope, and `.no-ai`
checks as drag-and-drop. Nothing is sent until **Send locally** is pressed;
the integration does not depend on a particular file manager.
The desktop include starts SwayNC and binds `Super+N` to its control center.
Waybar shows
the live notification count; left click opens the inbox and right click
toggles Do Not Disturb.

Super+Space opens Wofi in desktop-application mode. Its empty-query view uses
a five-column app grid, and typing switches to fuzzy matching over installed
`.desktop` entries. The launcher has no generic command-runner provider or
resident helper service. Desktop actions can be expanded with Tab. Fuzzel
remains installed for Evermore's deterministic file and semantic-result
pickers. The include disables Sway's stock Super+D binding, leaving one default
launcher shortcut. Wofi's VM check verified Super+Space, the empty-query grid,
immediate typed filtering, Enter launch, Escape dismissal, and outside-click
focus dismissal. Explicit favourites and recent ordering are still open. The known
`nwg-drawer` candidate was rejected because its focus handoff consumes the
first typed character. The VM's default headless-only Sway session has no
input devices, so `vm/phase10-launcher-waybar.sh` starts a temporary
headless-plus-libinput Sway session that attaches the VM's AT keyboard. The
default focus-mode toggle moves to `Super+Tab`.
`Super+Shift+F` opens deterministic search over files already
selected in the local index; it uses Fuzzel's dmenu input and does not invoke
embeddings or inference. Files marked with a `.no-ai` sidecar are excluded by
the index service. Drop an explicitly selected text file into the AI drawer to
attach it for one turn. Its Context tray also lists active file scopes that can
be dragged into the prompt or attached with **Use in this turn**. Press
**Send locally** to transmit its redacted text through `ai-contextd`.
`Super+Shift+V` places plain-text clipboard contents in
the AI drawer input and does not submit automatically. `Super+Shift+A` opens Evermore's
separate operation palette. The palette searches the shared catalogue,
submits through the operations daemon, and retains exact agent-grant and
owner-approval checks; it has no shell command path.

The Waybar preset uses standard Sway workspaces/mode, window title, network,
audio, battery, clock, and tray modules. Native module groups organize the
workspace, active-window, and system-status areas; spacing, text contrast, and
the active-workspace underline provide the visual hierarchy without outlined
module boxes. The panel uses text labels, so it does not require a special icon
font. Sway supervises Waybar through its native `swaybar_command` integration.
Waybar's native module-group drawers can reveal a compact status cluster with
a built-in transition if the panel later needs more room; they are not enabled
in the default always-readable layout. The Waybar and Wofi themes share the
graphite palette, cool-blue focus, and warm warning colors. Wofi uses a single
soft floating surface with transparent app cells, a five-column grid, and an
underline for the selected app; it appears only when invoked and keeps
keyboard focus through native drun mode.
Wofi uses Wayland layer-shell and exclusive keyboard focus; its drun mode is
only application search. Fuzzel remains available for deterministic dmenu
pickers. SwayNC stores the same D-Bus notifications sent as
toasts, so the inbox has one notification-history surface. `evermore-notifyd`
polls the read-only pending-operation list and posts a `Review` action for new
requests; it opens a focused Evermore review card, where the owner still
chooses Approve or Deny.
