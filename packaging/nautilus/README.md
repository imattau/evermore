# Nautilus publish action

Install `Publish to Nostr` in `~/.local/share/nautilus/scripts/` and make it
executable. Nautilus passes the selected file as its first argument. The
action uploads it to the configured Blossom server, then copies a NIP-92
`imeta` tag (`url`, SHA-256, MIME type, and size) to the clipboard. Paste the
tag into the Nostr client or post editor you use.

Set `EVERMORE_BLOSSOM_URL` in the environment inherited by Nautilus. Optional
`EVERMORE_IDENTITY` selects the signer identity (default `personal`). The
selected Blossom server must accept the NIP-98 authorization event signed by
`evermore-signer`.

The CLI equivalent is:

```sh
evermore-blossom-fs --identity personal \
  --blossom-url https://blossom.example \
  --publish-file ./photo.jpg
```
