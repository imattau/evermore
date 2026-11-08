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

`--artifact-url` (https://github.com/imattau/npack/pull/33, merged,
released in v0.8.0) is a feature this project added to `npack` for
exactly this: it points the signed artifact event at any already-hosted
URL instead of requiring a Blossom upload. `build-and-publish.sh`
refuses to run against an older `npack`; point `NPACK_BIN` at a newer
build if the one on `PATH` predates it.

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
`qrencode` is a required base-image dependency of `evermore-greeter` because
the first-user wizard uses it to render NIP-46 pairing URIs as QR codes.

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
  --relay wss://relay.ditto.pub \
  --relay wss://nostr-01.yakihonne.com \
  --relay wss://relay.primal.net
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

In CI (`.github/workflows/release-npack.yml`), nothing persists a
registered key between runs, so the workflow passes `--secret-key`
explicitly from the `EVERMORE_NPACK_SECRET_KEY` repository secret
instead of relying on `npack register`.

## Running it from GitHub Actions

`.github/workflows/release-npack.yml` is `workflow_dispatch`-only --
deliberately never triggered by a tag push or anything else automatic,
since a run signs real events under Evermore's publisher identity and
writes them to real relays, and there's no settled relay/trust story to
automate against yet. Run it from the Actions tab (or `gh workflow run
release-npack.yml -f tag=v0.1.0`), with:

- `tag` (required): the GitHub release tag, e.g. `v0.1.0`; created if
  it doesn't already exist.
- `packages` (optional): which of the table above to build, space
  separated; defaults to all of them.
- `relays` (optional): where to publish the release events; defaults
  to `relay.ditto.pub`, `nostr-01.yakihonne.com` and `relay.primal.net`.
- `dry_run` (optional): build and pack every selected package without
  uploading or publishing anything, and upload the result as a workflow
  artifact per package for inspection instead.

One-time setup: `gh secret set EVERMORE_NPACK_SECRET_KEY --repo
imattau/evermore < packaging/npack/publisher.secret` (or paste just the
`NSEC=`/`HEX=` value by hand) -- required before any non-dry-run run.
The workflow fails fast with a clear message if it's missing. It
downloads a specific, hash-pinned release of `npack` itself (currently
v0.8.1) rather than "latest", so a future `npack` release never changes
this workflow's behavior without that pin being bumped on purpose.

## Updating

Bump the version in the relevant crate's `Cargo.toml`, tag a release,
and re-run the script for every package that changed with the new
`--tag`. `npack update` on the installing side picks up the new version
the same way it would from any other publisher.
