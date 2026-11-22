"""Serve the package's video fixture over loopback during verification."""

from contextlib import contextmanager
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from threading import Thread
from urllib.parse import quote


@contextmanager
def serve_sample(sample):
    handler = partial(SimpleHTTPRequestHandler, directory=sample.parent)
    with ThreadingHTTPServer(("127.0.0.1", 0), handler) as server:
        worker = Thread(target=server.serve_forever, daemon=True)
        worker.start()
        try:
            yield f"http://127.0.0.1:{server.server_port}/{quote(sample.name)}"
        finally:
            server.shutdown()
            worker.join()
