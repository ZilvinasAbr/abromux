#!/usr/bin/env python3
"""A GitLab far away: the dev GitLab behind a proxy that costs what a remote
server does. Each new connection waits CONNECT ms (TCP and TLS handshakes),
each request REQUEST ms (the round trip and the server's time). Connections
stay open between requests (keep-alive), as a real server's do, so a client
that reuses them pays the handshake once.

  dev/gitlab/slow-proxy.py [--port 8930] [--connect-ms 150] [--request-ms 120]

Then point the app at http://localhost:8930 (the same token works).
"""
import argparse, http.client, http.server, socket, socketserver, time

P = argparse.ArgumentParser()
P.add_argument("--port", type=int, default=8930)
P.add_argument("--upstream", default="localhost:8929")
P.add_argument("--connect-ms", type=int, default=150)
P.add_argument("--request-ms", type=int, default=120)
A = P.parse_args()

HOP = {"connection", "keep-alive", "transfer-encoding", "te", "trailer", "upgrade", "proxy-connection"}


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def setup(self):
        super().setup()
        time.sleep(A.connect_ms / 1000)
        self.upstream = http.client.HTTPConnection(A.upstream, timeout=120)

    def log_message(self, *_):
        pass

    def forward(self):
        time.sleep(A.request_ms / 1000)
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n) if n else None
        headers = {k: v for k, v in self.headers.items() if k.lower() not in HOP and k.lower() != "host"}
        headers["Host"] = A.upstream
        try:
            self.upstream.request(self.command, self.path, body=body, headers=headers)
            r = self.upstream.getresponse()
        except (http.client.HTTPException, OSError):
            self.upstream = http.client.HTTPConnection(A.upstream, timeout=120)
            self.upstream.request(self.command, self.path, body=body, headers=headers)
            r = self.upstream.getresponse()
        data = r.read()
        self.send_response(r.status)
        for k, v in r.getheaders():
            if k.lower() not in HOP and k.lower() != "content-length":
                self.send_header(k, v)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    do_GET = do_POST = do_PUT = do_DELETE = forward


class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    # IPv6 and IPv4 both, as a real server: `localhost` is ::1 first, and a
    # refused connection would make clients look slower than they are.
    address_family = socket.AF_INET6
    daemon_threads = True
    allow_reuse_address = True
    request_queue_size = 128

    def server_bind(self):
        self.socket.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 0)
        super().server_bind()


print(f"slow GitLab on http://localhost:{A.port} -> {A.upstream} "
      f"({A.connect_ms} ms a connection, {A.request_ms} ms a request)", flush=True)
Server(("::", A.port), Handler).serve_forever()
