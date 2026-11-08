# evermore-indexd

Local, read-only semantic search over individually selected text files. The
daemon listens on `$XDG_RUNTIME_DIR/evermore/ai-indexd.sock` using newline-
delimited JSON with the `evermore_common::ai_index` protocol.

Set `EVERMORE_INDEX_FILES` to a JSON array of exact file paths to make them
available as scopes. When the variable is unset, selections are read from
`$XDG_CONFIG_HOME/evermore/index-files.json` or `~/.config/evermore/index-files.json`;
the panel manages this file. There is no directory recursion or implicit
home-folder indexing. Only regular `.md`, `.txt`, `.rst`, and `.org` files up
to 2 MiB are accepted. Requests can list configured scopes, reindex/remove a
scope, or search the local index. Removing a scope purges its stored chunks.
Search results are limited to scopes still present in the configuration. No
message, DM, or cloud source is read.

Place a sibling `<filename>.no-ai` marker (for example, `journal.md.no-ai`) to
exclude a selected file from embedding and search. Marked files disappear from
the active scope list immediately, so existing chunks no longer appear in
search. Removing the marker makes a file eligible again; reindex it to update
its stored content.

Embeddings use `evermore-inferd`. By default `evermore-indexd` asks the
inference service for an installed model whose manifest declares the
`embeddings` capability; set `EVERMORE_EMBED_MODEL` to select one explicitly.
The inference service rejects cloud model tags and launches the embedding model
on its own supervised `llama-server` child. Secret-shaped values are redacted
from both document chunks and search queries before embedding; redacted excerpts
are stored in `$EVERMORE_INDEX_STATE_DIR/semantic-index.json`, or under
`$XDG_STATE_HOME/evermore/semantic-index.json` when that override is unset, with
user-only permissions.

Context-source selection is managed by the panel's private configuration.
Model installation is handled through the signed NPK model-import operation;
model setup still requires a publisher-signed package and owner approval.
Context subscriptions remain future work; this daemon currently serves
allowlisted sources on request.

Install the daemon as a user service with
`cargo install --path crates/evermore-indexd`, copy
`packaging/systemd/evermore-indexd.service` to
`~/.config/systemd/user/`, then run `systemctl --user daemon-reload` and
`systemctl --user enable --now evermore-indexd.service`. It starts without an
embedding model; list scopes and health remain available, while reindex and
semantic search report a local inference error until `evermore-inferd` has a
signed embedding-capable model installed.
