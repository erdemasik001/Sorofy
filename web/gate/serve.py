#!/usr/bin/env python3
"""Serve this directory over http://127.0.0.1:8081.

Nothing clever, and nothing installed: the gate is static files and ES modules, which browsers
will not load from `file://`. That is the whole reason this exists.

    python3 serve.py [port]
"""

import functools
import http.server
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


class Server(http.server.ThreadingHTTPServer):
    # Threaded so that one connection cannot block the others: a browser opens several sockets
    # for the page's thirteen ES modules and does not always send a request on each one right
    # away, and a single-threaded server waits on that idle socket while the modules behind it
    # time out — a blank gate on the first load, fine on reload, which is the worst way to lose
    # a demo.
    daemon_threads = True


if __name__ == "__main__":
    handler = functools.partial(Handler, directory=str(HERE))
    with Server(("127.0.0.1", PORT), handler) as httpd:
        print(f"Sorofy gate → http://127.0.0.1:{PORT}/")
        print("Ctrl-C to stop.")
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            print("\nstopped")
