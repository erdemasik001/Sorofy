#!/usr/bin/env python3
"""Serve this directory over http://127.0.0.1:8081.

Nothing clever, and nothing installed: the gate is static files and ES modules, which browsers
will not load from `file://`. That is the whole reason this exists.

    python3 serve.py [port]
"""

import functools
import http.server
import socketserver
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8081


class Handler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        # No caching: during a demo the cost of a stale file is an inexplicable bug on stage,
        # and the cost of re-reading a few kilobytes from localhost is nothing.
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def log_message(self, fmt, *args):  # quieter than the default
        sys.stderr.write("  %s\n" % (fmt % args))


if __name__ == "__main__":
    socketserver.TCPServer.allow_reuse_address = True
    handler = functools.partial(Handler, directory=str(HERE))
    with socketserver.TCPServer(("127.0.0.1", PORT), handler) as httpd:
        print(f"Sorofy gate → http://127.0.0.1:{PORT}/")
        print("Ctrl-C to stop.")
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            print("\nstopped")
