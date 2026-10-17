# Optional sync server

R-CAD is complete without a server. A sync server adds, for an organization: shared projects
and folders, whole-document versions with milestones, edit locks, share links and comments.
The wire contract is the `rcad-cloud-proto` crate (JSON over HTTPS, REST routes under
`/api/v1`, protocol version 1). It is MIT OR Apache-2.0, so anyone can implement a server.

The client side is the `rcad-cloud` crate (used by the app and by `rcad-cli cloud`).

## Guarantees

- **No account, no network.** Until the user signs in, R-CAD opens no socket, writes no cloud
  file and reads nothing but the (absent) account file. Local documents never depend on the
  server.
- **Cloud documents are ordinary files.** A cloud document is edited as a local `.rcad.json`
  working copy; a link records which document and base version it belongs to. You can always
  copy the file out, and every version can be downloaded as a self-contained file.
- **Offline is normal.** Opening a document you have opened before works offline (the cached
  working copy). Saving a version offline queues it; the queue is sent later, in order.
- **No silent overwrite, in either direction.**
  - Saving a version names the version the edit started from. If someone saved in the
    meantime, the server answers with a conflict, and nothing is saved until you choose to
    save anyway (`force`) or to save as a new document.
  - Opening a document never replaces a working copy that has local modifications. You get
    the modified copy, plus a conflict notice when the server has moved on.
- **Integrity.** Version content is addressed by its BLAKE3 hash. Downloads whose bytes do
  not match the hash are rejected, and so are cached blobs that fail the same check.

## Client files

Accounts: `$XDG_CONFIG_HOME/r-cad/cloud.json` (else `~/.config/r-cad/cloud.json`), next to
`prefs.json`. Versioned JSON (`"version": 1`) holding each account's server URL, server
name, username and bearer token. The file is written atomically, with mode 0600 on Unix.
Signing out revokes the token on the server (best effort) and removes it from the file.

Cache: `$XDG_CACHE_HOME/r-cad/cloud` (else `~/.cache/r-cad/cloud`), one directory per server:

```text
<host>_<port>/                       the server URL's host, port and path, sanitized
  links.json                         local working file -> document, base version, base hash
  pending.json                       versions saved while offline, in order
  blobs/<aa>/<hash>                  content-addressed copies of downloaded/uploaded versions
  work/<document-id>/<name>.rcad.json            editable working copies
  versions/<document-id>/v<N>-<name>.rcad.json   read-only copies of older versions
```

- Every file is replaced atomically (temporary file in the same directory, then rename).
- Document names and ids come from the server. They are reduced to a single safe file-name
  component: no separators, drive prefixes, control characters, Windows-forbidden characters
  or device names, no leading dots, at most 96 bytes. A hostile name cannot leave the cache
  directory.
- A file uploaded with **push** / **Upload** stays where it is and becomes the linked working
  copy. It is not copied.
- Deleting the cache loses nothing that is on the server. It does lose queued (unsent)
  versions and the links, so sync first.

## Behaviour details

- **Sign in.** The client first reads `GET /api/v1/info`. A server that is not an R-CAD
  server, or that speaks another protocol version, is refused before the password is sent.
  The token's device name is `R-CAD on <hostname>`.
- **Open.** The head version is taken from the blob cache if present, otherwise downloaded and
  verified. It is then written to the working copy, and the link moves to the head.
  - Working copy modified (its hash differs from the link's base hash): it is kept and
    reported, with a conflict if the head moved.
  - Server unreachable: the cached working copy opens, marked as from the cache.
- **Save version.** The file's bytes are hashed and stored in the blob cache. They are
  uploaded only if the organization lacks that blob, then the version is created with the
  link's base version.
  - 409 `version_conflict`: a conflict is reported and nothing is saved.
  - 423 `locked`: the lock holder is reported.
  - Server unreachable: the snapshot, already in the blob cache, is queued. Later edits of
    the file do not change what is sent.
  - If a save's answer was lost (connection dropped after the server stored it), the replay
    finds the head already holding the same bytes and counts it as saved.
- **Sync.** Queued versions are sent in order.
  - The first conflict or lock stops the queue. That entry stays queued; resolve it by saving
    that file with force, which forces its queued versions too.
  - An entry that can never succeed is dropped with an error: the document is gone, you lack
    permission, it is too large, or the file is no longer linked. Its bytes stay in the blob
    cache and the file is untouched.
  - A save of a document that already has queued versions goes behind them.

## Command line

`rcad-cli cloud` uses the same account file and cache as the app, so signing in from the
command line also signs in the app.

```text
rcad-cli cloud login https://cad.example.net ada      # password: $RCAD_PASSWORD or one stdin line
rcad-cli cloud whoami
rcad-cli cloud orgs
rcad-cli cloud projects <org-id>
rcad-cli cloud tree <project-id>
rcad-cli cloud push Bracket.rcad.json --project <id> [--folder <id>] [--name "Bracket"]
rcad-cli cloud save Bracket.rcad.json -m "thicker web" [--milestone "Prototype A"] [--force]
rcad-cli cloud pull <document-id> [--version N] [--out copy.rcad.json]
rcad-cli cloud versions <document-id>
rcad-cli cloud share <document-id> [--version N] [--days 7] [--download]
rcad-cli cloud sync
rcad-cli cloud logout
```

- `pull` without `--out` prints the path of the linked working copy: edit it and `save` it.
- `pull --out` writes an unlinked copy.
- `--json` prints one JSON object per event.
- Exit codes:

  | code | meaning |
  |---|---|
  | 0 | ok |
  | 1 | error |
  | 3 | version conflict |
  | 4 | locked by another user |
  | 5 | server unreachable, version queued |
  | 64 | usage |

## Security notes

- Use `https://` server URLs. A home lab behind `tailscale serve` or a reverse proxy with a
  real certificate works as is: certificates are checked by the operating system's verifier
  (rustls with `rustls-platform-verifier`), so a locally trusted CA works too.
- Plain `http://` is accepted for trusted networks only. The password at sign-in and the
  bearer token on every request then travel unencrypted.
- The client follows no redirects. A server that redirects, for example from http to https,
  must be added with its final URL.
- The token is a secret: mode 0600 file, redacted from `Debug` output, never printed by the
  CLI or included in error messages. Passwords are not stored.
- Timeouts: 5 s to connect, 60 s per request, 30 min for uploads and downloads. Errors map as
  follows:

  | what happened | reported as |
  |---|---|
  | DNS failure, refused connection, TLS failure, timeout | offline |
  | 502/503/504 from a proxy, without an R-CAD error body | offline |
  | any other non-2xx answer | the server's structured error |
- Ids from the server are percent-encoded into URL paths and sanitized before use as file
  names.
