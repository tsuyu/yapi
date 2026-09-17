#!/usr/bin/env python3
"""Throwaway API for exercising yAPI by hand.

    python dev-server.py            # http://localhost:8000

Every endpoint is deliberately boring; the point is to have something local
that covers each feature of the client.

    GET  /                 what this server offers, as json
    ANY  /echo             echoes method, path, query, headers, cookies, body
    POST /login            {"user":..,"pass":..} -> {"access_token":..}
    GET  /users/me         needs Authorization: Bearer <token> -> {"id":42,..}
    GET  /users/<id>       one user
    GET  /xml              an xml document
    GET  /html             an html page
    GET  /image            a small png
    GET  /binary           16 bytes of non-text
    GET  /big              200 json items, for the tree view and jsonpath
    GET  /cookies          sets two cookies
    GET  /redirect         302 to /echo, for -L
    GET  /slow?ms=2000     sleeps, for timeouts
    GET  /status/<code>    returns that status
    POST /upload           multipart or binary; reports what it received
    GET  /basic-auth       needs Basic dev:dev
"""

import base64
import json
import time
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

PORT = 8000
TOKENS = {}

# a 1x1 red png, built here so the file stays dependency free
PNG = (
    b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x02"
    b"\x00\x00\x00\x90wS\xde\x00\x00\x00\x0cIDATx\x9cc\xf8\xcf\xc0\x00\x00\x03\x01"
    b"\x01\x00\x18\xdd\x8d\xb0\x00\x00\x00\x00IEND\xaeB`\x82"
)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "yAPI-dev/1.0"

    # ---------------------------------------------------------------- utils

    def log_message(self, fmt, *args):
        print(f"  {self.command:7} {self.path}")

    def body_bytes(self):
        length = int(self.headers.get("Content-Length") or 0)
        return self.rfile.read(length) if length else b""

    def reply(self, code, body, content_type="application/json", extra=None):
        if isinstance(body, (dict, list)):
            body = json.dumps(body, indent=2).encode()
        elif isinstance(body, str):
            body = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        for key, value in (extra or []):
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(body)

    def described(self):
        url = urlparse(self.path)
        raw = self.body_bytes()
        try:
            body = raw.decode()
        except UnicodeDecodeError:
            body = f"<{len(raw)} binary bytes>"
        cookies = {}
        for pair in (self.headers.get("Cookie") or "").split(";"):
            if "=" in pair:
                k, v = pair.split("=", 1)
                cookies[k.strip()] = v.strip()
        return {
            "method": self.command,
            "path": url.path,
            "query": {k: v[0] if len(v) == 1 else v
                      for k, v in parse_qs(url.query).items()},
            "headers": dict(self.headers),
            "cookies": cookies,
            "body": body,
            "body_bytes": len(raw),
        }

    # --------------------------------------------------------------- routes

    def route(self):
        path = urlparse(self.path).path.rstrip("/") or "/"
        query = parse_qs(urlparse(self.path).query)

        if path == "/":
            return self.reply(200, {
                "server": "yAPI dev server",
                "endpoints": [
                    "/echo", "/login", "/users/me", "/users/<id>", "/xml",
                    "/html", "/image", "/binary", "/big", "/cookies",
                    "/redirect", "/slow?ms=2000", "/status/<code>", "/upload",
                    "/basic-auth",
                ],
            })

        if path == "/echo":
            return self.reply(200, self.described())

        if path == "/login":
            data = {}
            try:
                data = json.loads(self.body_bytes() or b"{}")
            except json.JSONDecodeError:
                pass
            user = data.get("user", "dev")
            token = "tok-" + base64.urlsafe_b64encode(
                f"{user}:{time.time()}".encode()).decode().rstrip("=")[:24]
            TOKENS[token] = {"id": 42, "name": user}
            return self.reply(200, {
                "access_token": token,
                "token_type": "Bearer",
                "expires_in": 3600,
                "user": {"id": 42, "name": user},
            })

        if path == "/users/me":
            auth = self.headers.get("Authorization", "")
            if not auth.startswith("Bearer "):
                return self.reply(401, {"error": "no bearer token"})
            token = auth[7:].strip()
            if token not in TOKENS:
                return self.reply(401, {"error": "unknown token", "got": token})
            user = TOKENS[token]
            return self.reply(200, {"id": user["id"], "name": user["name"],
                                    "email": f"{user['name']}@example.com"})

        if path.startswith("/users/"):
            uid = path.rsplit("/", 1)[-1]
            return self.reply(200, {
                "id": uid,
                "name": "dev",
                "email": "dev@example.com",
                "roles": ["admin", "user"],
            })

        if path == "/xml":
            return self.reply(200,
                              '<?xml version="1.0"?><catalog><book id="1">'
                              "<title>Rust</title><price>30</price></book>"
                              "<book id=\"2\"><title>Go</title><price>25</price>"
                              "</book></catalog>",
                              "application/xml")

        if path == "/html":
            return self.reply(200,
                              "<!doctype html><html><head><title>hi</title>"
                              "<style>p{color:red}</style></head><body>"
                              "<h1>Heading</h1><p>Some <b>text</b>.</p>"
                              "<script>console.log(1 < 2)</script>"
                              "</body></html>",
                              "text/html; charset=utf-8")

        if path == "/image":
            return self.reply(200, PNG, "image/png")

        if path == "/binary":
            return self.reply(200, bytes(range(16)), "application/octet-stream")

        if path == "/big":
            return self.reply(200, {
                "total": 200,
                "items": [
                    {"id": i, "name": f"item-{i}", "price": i * 3,
                     "tags": ["a", "b"] if i % 2 else ["c"]}
                    for i in range(200)
                ],
            })

        if path == "/cookies":
            return self.reply(200, {"set": ["session", "theme"]}, extra=[
                ("Set-Cookie", "session=s3cr3t; Path=/; HttpOnly; SameSite=Lax"),
                ("Set-Cookie", "theme=dark; Path=/"),
            ])

        if path == "/redirect":
            self.send_response(302)
            self.send_header("Location", "/echo")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return

        if path == "/slow":
            ms = int(query.get("ms", ["2000"])[0])
            time.sleep(ms / 1000)
            return self.reply(200, {"slept_ms": ms})

        if path.startswith("/status/"):
            code = int(path.rsplit("/", 1)[-1] or 200)
            return self.reply(code, {"status": code})

        if path == "/upload":
            described = self.described()
            ct = self.headers.get("Content-Type", "")
            described["received"] = (
                "multipart" if "multipart/form-data" in ct else
                "urlencoded" if "x-www-form-urlencoded" in ct else
                "raw"
            )
            return self.reply(200, described)

        if path == "/basic-auth":
            auth = self.headers.get("Authorization", "")
            if not auth.startswith("Basic "):
                return self.reply(401, {"error": "no basic auth"}, extra=[
                    ("WWW-Authenticate", 'Basic realm="dev"')])
            user, _, pw = base64.b64decode(auth[6:]).decode().partition(":")
            if (user, pw) != ("dev", "dev"):
                return self.reply(403, {"error": "wrong credentials",
                                        "got": {"user": user}})
            return self.reply(200, {"authenticated": True, "user": user})

        return self.reply(404, {"error": "no such endpoint", "path": path})

    do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = do_HEAD = do_OPTIONS = route


if __name__ == "__main__":
    print(f"yAPI dev server on http://localhost:{PORT}  (ctrl+c to stop)")
    print("try:  localhost:8000/echo?a=1")
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
