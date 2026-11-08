# Evermore's own components, as npack packages

One manifest per installable component, under `<id>/manifest.json`. Each
`id` is the package's `kind:32267`/`kind:30063` `d` tag. `build-and-publish.sh`
builds the matching binary, bundles its systemd unit (when it has one),
and runs `evermore-npack publish`.

## Why only some components are here

`evermore-npack install`/`update` need a signer and a local relay already
running to resolve trust and fetch releases, so those two -- plus
`evermore-enroll` and `evermore-pkcs11`, which run before any user session
exists -- can't be installed by npack; they're baked into the ISO
(`packaging/archiso/`, not started) instead. Once they're up, everything
below updates through the same `install`/`update` path a first install
would use, baked-in or not.

| Package | Depends on | systemd unit |
| --- | --- | --- |
| `sh.evermore.settingsd` | — | `evermore-settingsd@.service` (per-identity) |
| `sh.evermore.notifyd` | `sh.evermore.settingsd` | `evermore-notifyd@.service` (per-identity) |
| `sh.evermore.blossom-fs` | — | `evermore-blossom-fs@.service` (per-identity) |
| `sh.evermore.contextd` | — | `evermore-contextd.service` |
| `sh.evermore.indexd` | — | `evermore-indexd.service` |
| `sh.evermore.inferd` | — | `evermore-inferd.service` |
| `sh.evermore.operationsd` | `sh.evermore.settingsd` | `evermore-operationsd@.service` (per-identity) |
| `sh.evermore.sway-ai-bridge` | `sh.evermore.contextd` | `evermore-sway-ai-bridge.service` |
| `sh.evermore.mcp` | `sh.evermore.operationsd` | *(none -- see below)* |
| `sh.evermore.panel` | — | *(none -- a GUI app, launched manually/via a `.desktop` entry)* |
| `sh.evermore.greeter` | — | *(none -- see caveat below)* |

`sh.evermore.mcp` has no unit on purpose: its default mode speaks MCP
over stdio, spawned per-session by whatever agent client is using it, not
run persistently. Only `--http` mode is daemon-shaped, and nothing in
this repo runs it that way yet.

## `sh.evermore.greeter`'s caveat

`evermore-greeter` is exec'd by `greetd` (see `packaging/greetd/`) as the
`greeter` system user, before any regular user's session -- and therefore
before that user's `~/.local/bin` (where npack would symlink it) exists
or is even meaningful. `greetd`'s config currently expects it on the
system `$PATH`. Until there's a system-scope npack install mode (writing
to e.g. `/usr/local/bin` instead of `$XDG_DATA_HOME`/`~/.local/bin`), this
manifest is here for completeness and for a future system-scope mode, but
updating the *running* greeter still means re-baking/re-copying the
binary to its system path by hand, same as today.

## Building and publishing

```sh
packaging/npack/build-and-publish.sh sh.evermore.settingsd \
  --identity publisher --blossom-url https://blossom.example.org
```

This needs a running `evermore-signer` (with `--identity publisher`'s key
loaded) and a running local relay; see the script's own header comment
for the full prerequisite list, and PROJECT_IDEA.md's Phase 5 for why
there's still no default trust seed (M5.3) -- whoever installs these
packages has to `evermore-npack trust <publisher-pubkey>` once first.

## Updating

Once installed, each package updates independently of the others:

```sh
evermore-npack update sh.evermore.settingsd --identity me --blossom-url https://blossom.example.org
# or, for everything at once:
evermore-npack update-all --identity me --blossom-url https://blossom.example.org
```

`update` only moves a package forward if a newer trusted release exists,
refuses the swap if the package's declared `dependencies` aren't
installed, and restarts its `systemd_unit` (if any) after flipping the
`current` symlink -- so e.g. updating `sh.evermore.contextd` alone does
not touch `sh.evermore.settingsd`'s install directory, version, or
running process.
