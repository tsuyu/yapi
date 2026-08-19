# Testing api-req

## Automated

```
cargo test                      # all 91
cargo test curl                 # one module
cargo test -- --nocapture       # keep println output
cargo clippy --all-targets      # lints
```

The network tests are real: they bind a loopback port, stand up an HTTP server,
send through the actual client, and assert on the bytes the server received. No
mocks, nothing hits the internet, nothing needs a fixture running.

Where the coverage sits:

| module | checks |
|---|---|
| `model` | url scheme rules, query/path params, encoding, base64, variables |
| `net` | bodies on the wire, multipart boundaries, content-type precedence, shape detection, cookies |
| `curl` | import of every supported flag, export, generate-then-parse round trip, shell tokenizing |
| `jsonpath` | keys, indices, wildcards, slices, recursive descent, bad paths |
| `jwt` | decode + HS256 signing against the jwt.io reference token |
| `oauth` | token endpoint exchange, both client-auth styles, PKCE vs RFC 7636 |
| `extract` | jsonpath/header/cookie/regex/status rules, per-rule failures |
| `chain` | the login → /users/me → /users/{{user_id}} flow, stop-on-failure |
| `pretty` | xml indenting, tag stripping, sizes, hex dump |
| `store` | collection and session round trips, corrupt files |
| `app` | search matching, view/shape pairing, cookie parsing |

## By hand

A throwaway API is included so you can exercise everything locally:

```
python dev-server.py            # http://localhost:8000
cargo run                       # in another terminal
```

| endpoint | for testing |
|---|---|
| `/echo` | echoes method, path, query, headers, cookies, body |
| `/login` | POST `{"user":"dev"}` → `{"access_token":...}` |
| `/users/me` | needs `Authorization: Bearer <token>` → `{"id":42,...}` |
| `/users/<id>` | one user |
| `/xml`, `/html` | the xml and html viewers |
| `/image`, `/binary` | image preview and hex dump |
| `/big` | 200 items, for the json tree and jsonpath |
| `/cookies` | two `Set-Cookie` headers |
| `/redirect` | 302 → `/echo`, for the follow-redirects option |
| `/slow?ms=2000` | timeouts |
| `/status/<code>` | any status code |
| `/upload` | reports whether it got multipart, urlencoded or raw |
| `/basic-auth` | wants Basic `dev:dev` |

### A pass over each feature

**Send.** Type `localhost:8000/echo` — no scheme, no `http://`. Add query rows
`a=1`, `b=two words`; the preview line under the url should read
`http://localhost:8000/echo?a=1&b=two+words`. Send with ctrl+enter. The echo
shows what arrived.

**Path params.** Url `localhost:8000/users/:id`, open the path tab, `scan url`,
set `id` to `7`. The tab header stops saying `1 missing`.

**Body.** POST to `/echo`. Try each kind: json (`format` should re-indent),
form-data with a file part, binary with any file. `/upload` reports which one it
received.

**Auth.** Basic `dev` / `dev` against `/basic-auth` → 200; wrong password → 403.
Bearer, api key, and jwt all show the header they will send at the bottom of the
auth tab.

**Response viewer.** `/xml` and `/html` for the formatters, `/image` for the
preview, `/binary` for hex, `/big` for the tree. In `/big`, search `item-19`,
then try jsonpath `$.items[0].name`, `$..price`, `$.items[3:6]`.

**Cookies.** `/cookies`, response cookies tab, `send these back with the
request`, then `/echo` — they come back in the request.

**Options.** `/redirect` with follow off returns 302; with `-L` on it follows to
`/echo` and the final url in the response headers tab shows it.

**Chaining.** Save three requests:

1. `login` — POST `localhost:8000/login`, json body `{"user":"dev"}`,
   extract `access_token` from `$.access_token`
2. `me` — GET `localhost:8000/users/me`, header
   `Authorization: Bearer {{access_token}}`, extract `user_id` from `$.id`
3. `by id` — GET `localhost:8000/users/{{user_id}}`

Switch the sidebar to chains, add the three steps in order, `run chain`. The log
should show three greens with `-> access_token` and `-> user_id`, and the
variables panel fills in. Delete `access_token` from the panel and re-run step 2
alone: it refuses before sending, saying the variable is unset.

**cURL.** `import curl` and paste:

```bash
curl -X POST "http://localhost:8000/echo" \
  -H "Authorization: Bearer abc123" \
  -H "Content-Type: application/json" \
  -b "session=s3cr3t" \
  -d '{"name":"John"}'
```

The auth tab should show a bearer token (not a header row), the cookie should be
in the cookies tab, and the body should be json. Send it and check the echo.
Then `copy as curl` and paste it back into `import curl` — same request.

**Persistence.** Type something into the url, do not save, close the window,
reopen. The draft is back, variables included.

### Testing against a real API

`https://httpbin.org` mirrors requests the same way `/echo` does, if you want
something over TLS. Nothing in the client phones home on its own.
