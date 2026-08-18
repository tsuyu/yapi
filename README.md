# api-req

Desktop GUI for poking HTTP APIs. Rust + egui/eframe, blocking reqwest on a
worker thread. Requests are saved to a JSON collection on disk.

## Run

```
cargo run --release
```

## Request builder

**Methods** — GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS.

**URL** — a url with no scheme gets `http://` when the host is loopback
(`localhost`, `*.localhost`, `127.x.x.x`, `::1`, `0.0.0.0`,
`host.docker.internal`) and `https://` otherwise, so `localhost:3000/api` just
works. A bare `:3000/api` means localhost too, and the port buttons
(3000/5000/8000/8080) re-point the current path at a local server. The line
under the url bar previews exactly what will be requested, path params and
query merged in.

**Query params** — table with per-row on/off. `pull params out of url` moves an
existing `?a=b&c=d` into the table.

**Path params** — write `/users/:id/posts/{postId}` in the url; `scan url` adds
a row per placeholder. `:8080` is read as a port, not a placeholder. A
placeholder with no value is left in the url rather than silently dropped, and
the tab header flags how many are missing.

**Headers** — table, plus one-click `Content-Type: application/json` and
`Accept: application/json`. An explicit header always beats anything a body
kind or auth mode would have set.

**Cookies** — table, sent as a single `Cookie` header. One click copies
`Set-Cookie` values from the last response into the table.

**Body**

| kind | what it sends |
|---|---|
| none | no body |
| json | text editor, `format` pretty-prints, `application/json` |
| xml | text editor, `application/xml` |
| raw | text editor, `text/plain; charset=utf-8` |
| x-www-form-urlencoded | one `key=value` per line, percent-encoded |
| form-data | multipart: text parts and file parts, per-part content type |
| binary | one file as the whole body |

File parts and binary bodies guess their content type from the extension unless
you set one.

## Authentication

| mode | what it sends |
|---|---|
| none | nothing |
| basic | `Authorization: Basic <base64(user:pass)>` |
| bearer | `Authorization: Bearer <token>`; a value already starting with `Bearer ` is not double-prefixed |
| api key | your header name (default `X-API-Key`), or a query param |
| oauth 2.0 | `Authorization: <token_type> <access_token>` from a real token flow |
| jwt | your header (default `Authorization`) and prefix (default `Bearer`) |

**OAuth 2.0** runs the flow itself and stores the result with the request:

- *client credentials* and *password* grants POST the token endpoint directly.
- *authorization code* binds `http://127.0.0.1:<port>/callback`, opens your
  browser at the authorization endpoint, catches the redirect, checks `state`,
  and exchanges the code. PKCE (S256) is on by default; turn it off for servers
  that reject it.
- Client credentials go as HTTP Basic or as body fields, your choice. A public
  client (no secret) sends `client_id` in the body.
- The token panel shows time to expiry, has `refresh` and `clear`, and keeps the
  raw token endpoint response for when a server disagrees with you.

**JWT** takes a pasted token or signs one: paste, and the claims are decoded and
shown with the algorithm, `sub`, `iss` and expiry; or write claims, set a secret
(raw or base64), and `sign` mints an HS256 token. `+ exp 1h` stamps an
expiry. Signatures on pasted tokens are *not* verified — this shows you what you
are sending, it is not a validator.

The top bar warns before you send with a missing or expired credential.

## Response viewer

Status (colour coded), elapsed ms, size in human units, detected shape, and the
server's content type across the top. Three tabs: **body**, **headers**,
**cookies**.

The body shape comes from the content type, with a sniff as backup for servers
that say `application/octet-stream` or nothing at all. The views on offer follow
from it:

| shape | views |
|---|---|
| json | pretty (key order preserved), tree, raw |
| xml | pretty (re-indented), raw |
| html | pretty, text (tags and script/style stripped), raw |
| text | raw |
| image | image preview, hex |
| binary | hex dump |

- **Tree** — collapsible json, values coloured by type, `path` on any leaf copies
  its JSONPath.
- **Image** — png, jpeg, gif, bmp, webp and ico decode and display scaled to fit,
  with the pixel dimensions. Anything that fails to decode falls back to hex.
- **Search** — `find` highlights every hit in place and reports
  `N matches in M lines`; `case` for case sensitivity, `matching lines` filters
  down to hits with their line numbers.
- **JSONPath** — `$.data[0].id`, `$..name`, `$.items[*].price`,
  `$.books[0:2]`, `$["key with spaces"]`, negative indices. One match renders
  bare, several as an array; `copy result` takes it. Filter expressions
  (`?(@.x > 1)`) are not supported.
- **Cookies** — `Set-Cookie` split into name, value and attributes, with one
  click to copy them into the request cookies tab.
- **save body...** writes the raw bytes to a file, guessing a name from the url.

Failures come back with the reqwest error chain plus the hint that usually
applies: connection refused on loopback asks whether the dev server is up, a
cert failure points at `insecure tls`, a dns failure suggests `127.0.0.1` over
`localhost`, a timeout points at the timeout box.

## Local development

Requests to loopback bypass any system or corporate proxy. `insecure tls` in the
top bar skips certificate checks for a local https server with a self-signed
cert — it disables validation entirely, so keep it for servers you run.

## Saving

- `save` / `ctrl+s` writes the open request into the sidebar collection: url,
  method, query, path params, headers, cookies, body (including file paths and
  multipart parts), and the full auth config.
- Draft autosave: whatever is in the editor right now, plus the timeout and
  which entry is open, is written ~0.8s after you stop typing and again on exit,
  then restored next launch. Closing the window never loses typed input. The top
  bar flags `unsaved edits` when the draft differs from its sidebar entry.
- `import` appends another collection file; `export` writes the whole collection
  anywhere.

Files, side by side:

- Windows: `%APPDATA%\api-req\config\collection.json` + `session.json`
- Linux: `~/.config/api-req/`
- macOS: `~/Library/Application Support/api-req/`

A missing or corrupt file of either kind falls back to defaults rather than
failing to start.

**Secrets are stored in plain text**: bearer tokens, basic passwords, api keys,
oauth client secrets, access and refresh tokens, and jwt signing secrets all go
into `collection.json` / `session.json` as written. It is a local dev tool, not
a secrets manager — don't point it at production credentials or commit exports.
Secrets are masked in the UI until you tick `reveal`.

## Keys

- `ctrl+enter` send
- `ctrl+s` save current request

## Layout

- [src/model.rs](src/model.rs) — request/collection types, url, path params,
  query and base64 encoding
- [src/net.rs](src/net.rs) — request execution on a worker thread, bodies,
  multipart, error hints
- [src/oauth.rs](src/oauth.rs) — oauth2 grants, PKCE, loopback redirect capture
- [src/jwt.rs](src/jwt.rs) — jwt decode and HS256 signing
- [src/pretty.rs](src/pretty.rs) — xml/html formatting, tag stripping, hex dump
- [src/jsonpath.rs](src/jsonpath.rs) — jsonpath parser and evaluator
- [src/store.rs](src/store.rs) — collection and session persistence
- [src/app.rs](src/app.rs) — egui UI

## Tests

```
cargo test
```

62 tests. The network ones stand up a real HTTP server on a loopback port and
assert on the bytes it receives: path substitution, cookie header, multipart
boundaries and file parts, binary bodies, content-type precedence, and an oauth2
token endpoint exchange; responses are round-tripped to check shape detection,
xml indenting, image bytes surviving intact, and json key order. JWT signing is
checked against the jwt.io reference token and PKCE against RFC 7636 appendix B.
