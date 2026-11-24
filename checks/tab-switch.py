"""Run against an extracted executable in a desktop session.

Hold an incremental snapshot while the user switches away and back. The new
terminal must read the entire selected transcript. macOS uses System Events
and cliclick for the two clicks. Windows posts them to the test window.
"""
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
from queue import Empty, Queue
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def click_tab(pid, x):
    if sys.platform == "darwin":
        proc = f'tell application "System Events" to tell first application process whose unix id is {pid} to '
        subprocess.run(["osascript", "-e", proc + "set frontmost to true"], check=True)
        point = subprocess.check_output(["osascript", "-e", proc + "get position of window 1"], text=True)
        left, top = map(int, point.split(","))
        subprocess.run(["cliclick", f"c:{left+x},{top+88}"], check=True)
        return
    user = ctypes.windll.user32
    user.SetThreadDpiAwarenessContext(ctypes.c_void_p(-4))
    handles = []
    callback = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

    @callback
    def collect(hwnd, _):
        owner = wintypes.DWORD()
        user.GetWindowThreadProcessId(wintypes.HWND(hwnd), ctypes.byref(owner))
        if owner.value == pid and user.IsWindowVisible(wintypes.HWND(hwnd)):
            handles.append(wintypes.HWND(hwnd))
        return True

    user.EnumWindows(collect, 0)
    hwnd, = handles
    scale = user.GetDpiForWindow(hwnd) / 96
    position = round(x * scale) | (round(60 * scale) << 16)
    for message, buttons in [(0x200, 0), (0x201, 1), (0x202, 0)]:
        assert user.PostMessageW(hwnd, message, buttons, position)
        time.sleep(.1)


def run(executable, home):
    blocked, release = threading.Event(), threading.Event()
    resizes, subsequent = Queue(), Queue()
    sessions = [dict(session_id=id, name=id, state=state, owner=owner, connection_id=id,
                     target=dict(host="localhost", port=22, user="fixture"))
                for id, state, owner in [("A", "ready", "agent"), ("B", "manual", "human")]]

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *_):
            pass

        def reply(self, value):
            body = json.dumps(value).encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            self.reply(dict(service="bastion-bridge-gpui", pid=os.getpid(), workspace=str(home)))

        def do_POST(self):
            args = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            id = args.get("session_id", "")
            if self.path == "/api/resize":
                resizes.put(id)
            if self.path != "/api/snapshot":
                self.reply({})
                return
            offset = args["offset"]
            if id == "A" and offset > 0 and not blocked.is_set():
                blocked.set()
                release.wait(20)
            elif release.is_set():
                subsequent.put((id, offset))
            state = next((item for item in sessions if item["session_id"] == id), {})
            transcript = f"Host {id} transcript\r\n" if id else ""
            self.reply(dict(state, sessions=sessions, jobs=[], offset=len(transcript),
                            data=transcript[offset:], truncated=False))

    with ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
        threading.Thread(target=server.serve_forever, daemon=True).start()
        (home / "service.json").write_text(json.dumps(dict(port=server.server_port,
                                              token="isolated-test", pid=os.getpid())))
        gui = subprocess.Popen([executable], env=dict(os.environ, BASTION_GPUI_HOME=str(home)))
        try:
            assert blocked.wait(12), "GUI did not read the initial transcript"
            assert resizes.get(timeout=3) == "A"
            time.sleep(.4)  # Allow the first window frame to be presented.
            for x, id in [(140, "B"), (18, "A")]:
                click_tab(gui.pid, x)
                deadline = time.monotonic() + 4
                try:
                    while resizes.get(timeout=max(.01, deadline - time.monotonic())) != id:
                        pass
                except Empty as error:
                    raise AssertionError(f"Could not select tab {id}") from error
            release.set()
            actual = subsequent.get(timeout=5)
            assert actual == ("A", 0), f"new terminal resumed from stale snapshot {actual}"
            assert subsequent.get(timeout=5) == ("A", len("Host A transcript\r\n"))
            print("PASS delayed A response discarded after A -> B -> A; complete A transcript reread")
        finally:
            release.set()
            gui.terminate()
            gui.wait(timeout=8)
            server.shutdown()


if __name__ == "__main__":
    with tempfile.TemporaryDirectory() as directory:
        run(Path(sys.argv[1]).resolve(), Path(directory))
