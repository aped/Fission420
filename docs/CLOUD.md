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
name, username, bearer token and the server's download size limit, plus which account is
active. The file is written atomically, with mode 0600 on Unix. Signing out revokes the token
on the server (best effort) and removes that account from the file.

The file can hold several accounts (several servers, or several users on one server). The app
and every `rcad-cli cloud` command use the **active** one:

- signing in (again) makes that account active;
- `rcad-cli cloud use SERVER-URL [USERNAME]` makes an existing account active;
- signing out of the active account makes the first remaining account active;
- `rcad-cli cloud --account SERVER-URL[#USERNAME] COMMAND` uses another account for that one
  command without changing the file.

`rcad-cli cloud accounts` lists the accounts and marks the active one with `*`. Each account
has its own cache directory, links and offline queue (see below), so `queue`, `sync` and
`save` act on the account in use.

Cache: `$XDG_CACHE_HOME/r-cad/cloud` (else `~/.cache/r-cad/cloud`), one directory per account
on each server:

```text
<host>_<port>-<user>-<hash>/         host, port and path sanitized, the username, and 16 hex
                                     digits of BLAKE3 over the URL (scheme included) and the
                                     username: http:// and https:// on one host, and two
                                     accounts on one server, never share a directory
  .lock                              advisory lock (see below)
  links.json                         local working file -> document, base version, base hash
  pending.json                       versions saved while offline, in order
  blobs/<aa>/<hash>                  content-addressed copies of downloaded/uploaded versions
  work/<document-id>/<name>.rcad.json            editable working copies
  versions/<document-id>/v<N>-<name>.rcad.json   read-only copies of older versions
```

- Every file is replaced atomically (temporary file in the same directory, then rename).
- The app and `rcad-cli cloud` may run at the same time. Every read-modify-write of
  `links.json` and `pending.json` holds an exclusive advisory lock on the directory's `.lock`
  (`flock` on Unix, `LockFileEx` on Windows, through `std::fs::File::lock`), and every change
  of `cloud.json` holds one on `.cloud.json.lock` next to it. The lock is held only for the
  local read and write, never during a network request. A sync re-reads the queue before each
  entry and removes sent entries by value, so a version queued by the other process while a
  long upload runs is kept (and sent too) instead of being overwritten by a stale copy.
- Links record their server URL, and only links of the cache's own server are used.
- Caches written by earlier builds were keyed by host only. On first use, the old directory
  is taken over by the first account opened on that server, provided every link in it names
  this exact server URL (scheme included). Otherwise it is left alone and its documents are
  downloaded again; its queued versions are not sent from there.
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
  The login request is `POST /api/v1/auth/login` with the JSON body
  `{"username": "...", "password": "...", "device_name": "R-CAD on <hostname>"}`; all three
  fields are required (a body without `device_name` is refused with `missing field
  device_name`), and the device name labels the token in the server's token list. Plain
  `http://` needs consent unless the host is on a local network (see Security notes).
- **Revoked token.** When the token stops working (revoked by an administrator, or every
  session signed out by a password change), requests fail with `invalid or revoked token`.
  Sign in again (`rcad-cli cloud login SERVER-URL USERNAME`): it replaces that account's token
  and keeps its cache, links and queue.
- **Download limits.** A version is downloaded only if its declared size is within the
  server's advertised `max_blob_bytes` (recorded at sign-in; 1 GiB for accounts added by
  earlier builds). The download stops as soon as the server sends more bytes than the
  version's declared size, and the partial file is deleted. In-memory downloads (share link
  content) are capped at 1 GiB.
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
  - A version can carry a thumbnail: a PNG of at most 1 MiB (the app renders a clean isometric view of the model;
    `rcad-cli cloud save|push --thumbnail FILE.png` takes one). It is checked (PNG
    signature, size) before anything is sent, stored in the blob cache next to the
    document snapshot (so a queued save keeps it), uploaded with the same missing/upload
    steps and named in the version. A queued save whose thumbnail snapshot has gone from
    the cache is saved without one.
  - 422 `hash_mismatch` when creating the version means the server no longer has the blob
    (for example, its garbage collection removed it after the upload check). The blob is
    uploaded again from the blob cache and the save retried once; a second refusal is final.
- **Thumbnails.** `LoadThumbnail` reads the document detail for the thumbnail's hash and
  answers from the blob cache when it has that hash; otherwise it downloads
  `/versions/{n}/thumbnail`, capped at 1 MiB, and accepts it only with the PNG signature
  and the right hash. A version without a thumbnail answers with none (no error).
- **Newer version available.** `CheckHead` reads the document's head version; the app polls
  it for the open cloud document (see docs/UI.md) and offers to update an unmodified
  working copy.
- **Sync.** Queued versions are sent in order.
  - The first conflict or lock stops the queue. That entry stays queued; resolve it by saving
    that file with force, which forces its queued versions too.
  - An entry that can never succeed is dropped with an error: the document is gone, you lack
    permission, it is too large, or the file is no longer linked. Its bytes stay in the blob
    cache and the file is untouched.
  - A save of a document that already has queued versions goes behind them.
  - Any other refusal (for example `quota_exceeded`) stops the queue with an error and keeps
    the entry, since it may succeed later. The queue is listed in the Data panel and by
    `rcad-cli cloud queue`. **Discard** in the app drops one entry after a confirmation
    dialog. `rcad-cli cloud discard INDEX` asks `[y/N]` when stdin is a terminal; otherwise it
    refuses (exit 64) unless `--yes` is given. It prints `discarded INDEX: DOCUMENT queued
    TIME DESCRIPTION`. Discarding is local: the file and the blob cache are not touched, and
    the request names the entry's document and blob, so a queue that changed since it was
    listed is not edited by position alone.
- **Share links.** A share link (`rcad-cli cloud share`) shows its URL once; the server keeps
  only its id. `rcad-cli cloud shares DOCUMENT-ID` lists a document's live links (id, the
  version it pins or `latest`, expiry, whether downloading is allowed, who created it), and
  `rcad-cli cloud unshare SHARE-ID` revokes one: its page, metadata and content answer 410
  from then on. Listing and revoking need the project Editor role, like creating.
- **Rename, move, trash.** Renaming a document also renames it in the local links (the chip
  and the panel follow). Moving to the trash keeps the document's versions on the server;
  editors restore it from the project's Trash. A trashed document keeps its folder, so a
  folder whose documents are in the trash is still not empty and cannot be deleted. A local
  working copy of a trashed document stays on disk and linked; restore the document before
  saving versions of it.

## Roles and permissions

The server decides every permission (403 `forbidden` for an action the role does not allow,
404 `not_found` for anything the caller cannot see). The client uses the roles it is told
(`OrgSummary::my_role`, `Project::my_role`, `DocumentDetail::my_role`) only to hide or
disable controls, and shows the server's refusals.

| action | who (as the client offers it) |
|---|---|
| see a project, open and download its documents, list its members | project Viewer and up |
| comment | project Commenter and up |
| upload, save versions, rename, move, trash and restore documents; create, rename and delete folders; share links; promote; milestones | project Editor and up |
| add, change and remove project members (the server also lets them break locks) | project Admin (organization Admins and Owners are project admins everywhere without being listed) |
| create projects | organization Member and up (Guests see only projects they are invited to) |
| organization members and storage usage | organization Admin and Owner; Admins manage Guests and Members, only Owners manage Admins and Owners |

- A project member must already be a member of the project's organization; adding anyone else
  is refused with 404, and the Members window explains that the user must join the
  organization first (an organization administrator adds them under **Organization…**).
- The Members list shows explicit members only. Organization administrators do not appear in
  it but administer the project anyway.
- Names are unique (ignoring case) per scope: projects in an organization, documents in a
  project. A clash is 409 `already_exists`.

## Command line

`rcad-cli cloud` uses the same account file and cache as the app, so signing in from the
command line also signs in the app. Commands act on the active account (see Client files).

```text
rcad-cli cloud login https://cad.example.net ada      # password: $RCAD_PASSWORD, prompt or stdin
rcad-cli cloud login http://cad.example.net ada --allow-insecure-http
rcad-cli cloud whoami
rcad-cli cloud accounts                                # * marks the active account
rcad-cli cloud use https://cad.example.net [ada]       # make that account active
rcad-cli cloud --account https://lab.local:8640#bob tree <project-id>   # one command only
rcad-cli cloud orgs
rcad-cli cloud projects <org-id>
rcad-cli cloud tree <project-id>
rcad-cli cloud push Bracket.rcad.json --project <id> [--folder <id>] [--name "Bracket"] [--thumbnail t.png]
rcad-cli cloud save Bracket.rcad.json -m "thicker web" [--milestone "Prototype A"] [--force] [--thumbnail t.png]
rcad-cli cloud pull <document-id> [--version N] [--out copy.rcad.json]
rcad-cli cloud versions <document-id>
rcad-cli cloud share <document-id> [--version N] [--days 7] [--download]
rcad-cli cloud shares <document-id>                    # id, version|latest, expiry, download, creator
rcad-cli cloud unshare <share-id>                      # revoke a share link
rcad-cli cloud sync
rcad-cli cloud queue                                   # versions queued while offline, INDEX first
rcad-cli cloud discard <index> [--yes]                 # drop one (the local file is not changed)
rcad-cli cloud project create <org-id> <name> [--description TEXT]
rcad-cli cloud members <project-id>
rcad-cli cloud member add|set <project-id> <username> viewer|commenter|editor|admin
rcad-cli cloud member remove <project-id> <username>
rcad-cli cloud mkdir <project-id> <name> [--parent <folder-id>]
rcad-cli cloud trash <document-id>
rcad-cli cloud restore <document-id>
rcad-cli cloud logout
```

- `login` takes the password from `$RCAD_PASSWORD`, else from a prompt with echo off when
  stdin is a terminal, else from one line of stdin. Plain `http://` to a host that is not on
  a local network is refused before the password is read unless `--allow-insecure-http` is
  given (a warning is printed then).
- Text output escapes control characters, escape sequences and bidirectional-override
  characters in everything it prints (names, descriptions, comments, usernames, lock holders,
  server error messages), as `\u{1b}` and so on, so a hostile string cannot drive the
  terminal (colour, cursor, title or clipboard sequences) or forge output lines. `--json`
  output carries the exact strings, escaped by the JSON encoder.
- `push` prints the new document's id on a `document <id>` line (with `--json`, the
  `version_saved` event's `document` field). `versions`, `pull`, `share` and `shares` take it;
  `tree` lists it too.
- `pull` without `--out` prints the path of the linked working copy: edit it and `save` it.
- `pull --out` writes an unlinked copy.
- `--json` prints one JSON object per line, one per event, with an `"event"` field naming
  it. `--json` and `--account` may appear anywhere after `cloud` (`rcad-cli cloud versions
  <id> --json`) or before it (`rcad-cli --json cloud versions <id>`).
- `--account SERVER-URL[#USERNAME]` (also `--account=...`) picks the account for one command.
  The URL is matched after normalization (a bare host means `https://`, a trailing slash is
  ignored); the username is needed only when that server has several accounts. A missing or
  ambiguous account is an error (exit 1). It is refused (exit 64) with `login`, `accounts`
  and `use`, which name their account themselves. `logout --account ...` signs out of that
  account.
- `discard` asks for confirmation on a terminal; in scripts pass `--yes` (`-y`). Without
  either, when stdin is not a terminal, nothing is discarded and the exit code is 64.
- When standard output is closed early (`rcad-cli cloud versions <id> | head -1`), the
  command stops printing and exits with status 141 (128 + SIGPIPE, as a shell reports for a
  tool killed by the signal). The request itself has already completed by then.
- Roles are case-insensitive. `member add` and `member set` send the same request (add, or
  change the role).
- Exit codes:

  | code | meaning |
  |---|---|
  | 0 | ok |
  | 1 | error |
  | 3 | version conflict |
  | 4 | locked by another user |
  | 5 | server unreachable, version queued |
  | 64 | usage |
  | 141 | standard output closed before everything was printed |

## Security notes

- Use `https://` server URLs. A home lab behind `tailscale serve` or a reverse proxy with a
  real certificate works as is: certificates are checked by the operating system's verifier
  (rustls with `rustls-platform-verifier`), so a locally trusted CA works too.
- Plain `http://` is meant for trusted networks only: the password at sign-in, the bearer
  token on every request and the documents travel unencrypted. Signing in over `http://`
  works without asking only when the host is a loopback name or address, a private
  (10/8, 172.16/12, 192.168/16), link-local (169.254/16) or carrier-grade NAT (100.64/10,
  used by Tailscale) IPv4 address, a unique-local or link-local IPv6 address, or a name under
  `.local`, `.lan`, `.internal`, `.home.arpa`, `.localhost` or `.ts.net`. Any other host
  (single-label names included, since a DNS search domain can resolve them publicly) needs
  explicit consent: `--allow-insecure-http` on the command line, or ticking **Allow
  unencrypted connection** in the sign-in dialog, which shows a warning. The check happens at
  sign-in; accounts added earlier keep working.
- The client honours the standard proxy variables (`HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`,
  `NO_PROXY`).
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
