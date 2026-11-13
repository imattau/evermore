# Sway desktop bindings

Install `waybar`, `fuzzel`, `swaync`, and `wl-clipboard` from Arch's official
repositories. Install `evermore-launcher` through npack or build it from the
workspace. From the checkout, run:

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

Super+Space opens Evermore's eGUI app launcher as a centered floating window.
It discovers visible installed XDG applications, fuzzy-searches names and
metadata, and presents a favorites row followed by the app grid. Favorites and
recent ordering persist per user. Arrow keys move selection, Enter launches,
Ctrl+D pins or unpins the selected app, and Escape or focus loss closes the
launcher. It does not expose desktop actions or a command runner. Fuzzel stays
available for deterministic file and semantic-result pickers. The include
disables Sway's stock Super+D binding, leaving one default launcher shortcut.
The disposable VM check exercises the launcher and Waybar in a temporary
headless-plus-libinput Sway session because the regular VM Sway session has no
input devices. The default focus-mode toggle moves to `Super+Tab`.
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
in the default always-readable layout. The Waybar and eGUI launcher themes
share the graphite palette, cool-blue focus, and warm warning colors. The
launcher uses a compact floating app grid with clear hover and keyboard
selection states. It appears only when invoked and closes when it loses focus.
The engineering image includes Firefox, NetworkManager's connection editor,
Pavucontrol, Blueman, Wdisplays, Swaylock, and Brightnessctl. The launcher
discovers their desktop entries. Waybar's network and audio labels open the
matching upstream settings tools; the included Sway bindings provide screen
lock (`Ctrl+Alt+L`), brightness, and audio controls from standard hardware
keys. The image sets Firefox as the system XDG browser default; user-level
desktop preferences can select a different installed browser.
Fuzzel remains available for deterministic dmenu pickers. SwayNC stores the
same D-Bus notifications sent as
toasts, so the inbox has one notification-history surface. `evermore-notifyd`
polls the read-only pending-operation list and posts a `Review` action for new
requests; it opens a focused Evermore review card, where the owner still
chooses Approve or Deny.
