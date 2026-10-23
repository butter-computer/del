#!/usr/bin/env python3
"""Exercise native WebKit against a local fixture; no external sites or accounts."""
import http.server
from pathlib import Path
import subprocess
import tempfile
import threading
import uuid

class Fixture(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.end_headers()
        self.wfile.write(b'<title>Browser fixture</title><h1>Browser fixture</h1><input id="text"><button>Go</button><a href="?link">Link</a>')

    def log_message(self, *args):
        pass

root = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix="del-browser-test-") as directory:
    binary = str(Path(directory) / "browser-test")
    subprocess.run(["clang", "-fobjc-arc", "-Wno-incompatible-pointer-types", str(root / "scripts/test-browser.m"), "-framework", "AppKit", "-framework", "WebKit", "-o", binary], check=True)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    url = f"http://127.0.0.1:{server.server_port}/"
    try:
        subprocess.run([binary, url], check=True, timeout=100)
        profile = str(uuid.uuid4())
        subprocess.run([binary, url, "write", profile], check=True, timeout=30)
        subprocess.run([binary, url, "read", profile], check=True, timeout=30)
    finally:
        server.shutdown()
