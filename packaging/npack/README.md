# Publishing Evermore's own components as npack packages

Evermore's package manager is the real `npack`
(https://github.com/imattau/npack -- not a component of this repo; see
`apps/evermore-panel/src/npack_client.rs`, `crates/evermore-operationsd/src/npackd_client.rs`
and `crates/evermore-inferd/src/npackd_client.rs` for how Evermore's own
daemons talk to it). This directory is the tooling to publish Evermore's
own components *through* it.

## Why `--artifact-url`, not a Blossom server

npack normally uploads a package's `.npk` to a Blossom server and
points the signed release at that URL. Evermore doesn't want to stand
up or depend on one just to distribute its own packages, when this
repo's own GitHub Releases are a perfectly good place to host a `.npk`
built from a tagged release.

`--artifact-url` (https://github.com/imattau/npack/pull/33) is a
feature this project added to `npack` for exactly this: it points the
signed artifact event at any already-hosted URL instead of requiring a
Blossom upload. Until that PR merges, build `npack` from
`feat/artifact-url-storage` on that branch yourself and point
`NPACK_BIN` at it.

## What's packaged

| Package | Depends on | systemd unit |
| --- | --- | --- |
| `settingsd` | — | `evermore-settingsd@.service` (per-identity) |
| `notifyd` | `settingsd` | `evermore-notifyd@.service` (per-identity) |
| `blossom-fs` | — | `evermore-blossom-fs@.service` (per-identity) |
| `contextd` | — | `evermore-contextd.service` |
| `indexd` | — | `evermore-indexd.service` |
| `inferd` | — | `evermore-inferd.service` |
| `operationsd` | `settingsd` | `evermore-operationsd@.service` (per-identity) |
| `sway-ai-bridge` | `contextd` | `evermore-sway-ai-bridge.service` |
| `mcp` | `operationsd` | *(none -- spawned per-session over stdio by an MCP client, not run as a daemon)* |
| `panel` | — | *(none -- a GUI app, launched manually/via a `.desktop` entry)* |

Not packaged: `evermore-signer`, the local relay, `evermore-enroll`, and
`evermore-pkcs11` stay baked into the ISO -- npack's own `install`/
`update` need a signer and a relay already running, so those two (plus
the two that run before any session exists) can never be npack's own
first install. `evermore-greeter` is exec'd by `greetd` as the
`greeter` system user before any regular user's `$HOME` exists, so a
per-user npack install can't place it where `greetd` looks; it isn't
packaged here either, for the same reason.

## The publisher identity

`publisher.json` carries the public half (npub + hex) of Evermore's own
npack publisher key -- safe to commit and reference, and the value
every `--publisher` argument below should use. The secret half lives
only in `publisher.secret` next to it, generated locally, gitignored,
never committed; back it up somewhere durable (a password manager, not
another repo) and treat it like any other private key. For GitHub
Actions, store its `NSEC` (or `HEX`) value as a repository secret by
hand (`gh secret set`) when the workflow is set up -- nothing here does
that automatically.

## Building and publishing

```sh
packaging/npack/build-and-publish.sh settingsd \
  --publisher npub1g3vdg46zq7xpkmtdnta7l8u9684zd2p4e9wrp3xd7wv4xk7vq03svh653x \
  --tag v0.1.0 \
  --relay wss://relay.example
```

This builds the release binary, runs `npack init`/`pack` to produce a
real `.npk` (with `dependencies` and a `register-service` post-install
action filled in from the table above), uploads it as a `v0.1.0` asset
on this repo's GitHub Releases (`gh release upload --clobber`, so
re-running after a fix replaces it), and runs `npack publish
--artifact-url` against that asset -- no `--server` needed at all.

Pass `--dry-run` to build and pack without uploading or publishing; the
script then copies its working directory (the `.npk` and its
hash-filled manifest) next to this script, named `<tmpdir>.dry-run`,
for inspection with `npack verify`/`npack inspect`.

Needs, once: `npack register` (or `generate-key`) with Evermore's
publisher key, and `gh auth login` for this repo. Whoever installs a
package built this way needs to `npack trust add <publisher>` once, and
pass `--allow-capability service-manager` on `install`/`update` for any
package with a systemd unit (npack gates `register-service` behind that
capability on purpose -- it never enables or starts the unit itself).

## Updating

Bump the version in the relevant crate's `Cargo.toml`, tag a release,
and re-run the script for every package that changed with the new
`--tag`. `npack update` on the installing side picks up the new version
the same way it would from any other publisher.
