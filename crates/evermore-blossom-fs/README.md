# evermore-blossom-fs

This binary offers three ways to use a Blossom server. Upload and list
requests use short-lived NIP-98 events; BUD-04 mirror requests use a scoped
BUD-11 token. The signer creates each token; no API key or secret key is
stored by this crate.

## Sync directory

The systemd user unit `evermore-blossom-fs@.service` watches `~/Nostr/`.
Configure `BLOSSOM_URL` for the unit and enable it for an identity:

```sh
systemctl --user edit evermore-blossom-fs@personal
systemctl --user enable --now evermore-blossom-fs@personal
```

Add this to the unit override:

```ini
[Service]
Environment=BLOSSOM_URL=https://blossom.example
```

The service connects to the local control relay at `ws://127.0.0.1:4869` by
default. It waits for the initial subscription's `EOSE` before reconciling
local files, then uploads changed files and publishes the directory index.
The `--relay`, `--dir`, and `--label` options can override those defaults.
Deleting a local file removes it from the shared index; content-addressed
blobs already on the server are left alone.

## FUSE mount

Mount a read/write view backed by a local cache, with directory contents
resolved from the same relay index:

```sh
evermore-blossom-fs --identity personal \
  --blossom-url https://blossom.example \
  --label personal --mount ~/Blossom
```

The default cache is `$XDG_CACHE_HOME/evermore/blossom/<label>` (or
`~/.cache/evermore/blossom/<label>`). Override it with `--cache-dir`. The
mount requires Linux FUSE support, permission to access `/dev/fuse`, and the
`fuse3` package (which provides the unprivileged `fusermount3` helper). It does
not require a separately installed libfuse library.

## Publish a file

Upload one file and copy a NIP-92 `imeta` tag to the desktop clipboard:

```sh
evermore-blossom-fs --identity personal \
  --blossom-url https://blossom.example --publish-file ./photo.jpg
```

The command uses `wl-copy` or `xclip` if available and prints the tag either
way. A Nautilus script is provided in `packaging/nautilus/`.

## Mirror a blob

Ask the configured server to mirror a public URL using BUD-04. If the source
URL path includes the 64-character blob hash, it is inferred; otherwise pass
the expected digest explicitly. The request uses a short-lived, server-scoped
BUD-11 upload authorization token:

```sh
evermore-blossom-fs --identity personal \
  --blossom-url https://blossom.example \
  --mirror-url https://source.example/<sha256>.png
```

If the source URL does not include the hash in its path, add
`--mirror-sha256 <64-character-hex-digest>`.

The target server must implement the optional BUD-04 mirror endpoint.
