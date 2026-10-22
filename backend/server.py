#!/usr/bin/env python3
"""
MonarchCAD Backend Service & Heartbeat Server.

This service acts as the companion backend for MonarchCAD.
MonarchCAD pings `/heartbeat` every 2 seconds. If this backend
is stopped or terminated (Ctrl+C), MonarchCAD will automatically
shut down within a few seconds.
"""

import sys
import json
import time
from http.server import HTTPServer, BaseHTTPRequestHandler

PORT = 8000

class HeartbeatHandler(BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        # Clean terminal output
        sys.stdout.write(f"[{time.strftime('%H:%M:%S')}] {args[0]} - {args[1]}\n")
        sys.stdout.flush()

    def handle_one_request(self):
        try:
            super().handle_one_request()
        except (ConnectionResetError, BrokenPipeError):
            pass

    def finish(self):
        try:
            super().finish()
        except (ConnectionResetError, BrokenPipeError):
            pass

    def do_GET(self):
        try:
            if self.path == "/heartbeat":
                response = {
                    "status": "ok",
                    "service": "monarchcad-backend",
                    "timestamp": time.time()
                }
                body = json.dumps(response).encode("utf-8")
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Connection", "close")
                self.end_headers()
                self.wfile.write(body)
                self.wfile.flush()
            elif self.path == "/health":
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", "20")
                self.send_header("Connection", "close")
                self.end_headers()
                self.wfile.write(b'{"status":"healthy"}')
                self.wfile.flush()
            else:
                self.send_response(404)
                self.end_headers()
        except (ConnectionResetError, BrokenPipeError):
            pass

class SilentServer(HTTPServer):
    def handle_error(self, request, client_address):
        # Ignore client disconnects without printing traceback
        ex_type, ex_val, _ = sys.exc_info()
        if ex_type in (ConnectionResetError, BrokenPipeError):
            return
        super().handle_error(request, client_address)

def main():
    port = PORT
    if len(sys.argv) > 1:
        try:
            port = int(sys.argv[1])
        except ValueError:
            print(f"Invalid port: {sys.argv[1]}, using default {PORT}")

    server_address = ("127.0.0.1", port)
    httpd = SilentServer(server_address, HeartbeatHandler)

    print("=" * 60)
    print(f"  MonarchCAD Backend Server running on http://127.0.0.1:{port}")
    print(f"  - Heartbeat endpoint: http://127.0.0.1:{port}/heartbeat")
    print("  - If this server stops, MonarchCAD will automatically exit.")
    print("  - Press Ctrl+C to stop the server.")
    print("=" * 60)

    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        print("\n[Backend] Shutting down backend server...")
    finally:
        httpd.server_close()
        print("[Backend] Server stopped. MonarchCAD will now detect disconnection and terminate.")

if __name__ == "__main__":
    main()
