"""Exercise startup failure and retry using an isolated home on a Mac or Windows desktop."""
import ctypes
from ctypes import wintypes
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time


def wait_for(process, predicate):
    for _ in range(100):
        if process.poll() is not None:
            raise AssertionError(f"GUI exited during startup ({process.returncode})")
        if predicate():
            return
        time.sleep(.1)
    raise AssertionError("Timed out waiting for the startup or terminal window")


def mac_window(process, title):
    prefix = f'tell application "System Events" to tell first application process whose unix id is {process.pid} to '
    result = subprocess.run(["osascript", "-e", prefix + "get name of windows"], capture_output=True, text=True)
    return result.returncode == 0 and title in result.stdout.strip().split(", ")


def windows_window(process, title):
    user = ctypes.windll.user32
    handles = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def collect(hwnd, _):
        pid = wintypes.DWORD()
        hwnd = wintypes.HWND(hwnd)
        user.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        if pid.value == process.pid and user.IsWindowVisible(hwnd):
            caption = ctypes.create_unicode_buffer(128)
            user.GetWindowTextW(hwnd, caption, len(caption))
            if caption.value == title:
                handles.append(hwnd)
        return True

    user.EnumWindows(collect, 0)
    return handles[0] if handles else None


def retry(process):
    if sys.platform == "darwin":
        prefix = f'tell application "System Events" to tell first application process whose unix id is {process.pid} to '
        for action in ["set frontmost to true", "key code 36"]:
            subprocess.run(["osascript", "-e", prefix + action], check=True)
    else:
        hwnd = windows_window(process, "垒桥终端")
        for message, bits in [(0x100, 1), (0x101, 0xc0000001)]:
            assert ctypes.windll.user32.PostMessageW(hwnd, message, 13, bits)


def main(executable):
    if sys.platform not in ("win32", "darwin"):
        raise SystemExit("Run on a Mac or Windows desktop")
    window = mac_window if sys.platform == "darwin" else windows_window
    with tempfile.TemporaryDirectory() as directory:
        home = Path(directory)
        env = dict(os.environ, BASTION_GPUI_HOME=directory)
        sessions = home / "sessions.json"
        sessions.write_text("invalid fixture", encoding="utf8")
        process = subprocess.Popen([executable], env=env)
        try:
            wait_for(process, lambda: window(process, "垒桥终端") and (home / "service.log").exists())
            assert "sessions.json" in (home / "service.log").read_text(encoding="utf8")
            assert not (home / "service.json").exists()
            sessions.write_text("[]", encoding="utf8")
            # Give the asynchronous error response time to focus the retry button.
            for _ in range(25):
                if window(process, "垒桥终端 · BastionTerm"):
                    break
                if window(process, "垒桥终端"):
                    retry(process)
                time.sleep(.2)
            wait_for(process, lambda: window(process, "垒桥终端 · BastionTerm") and not window(process, "垒桥终端"))
            assert (home / "service.json").exists()
            print("PASS failed service launch leaves a retryable window, retry opens the terminal in the same GUI process")
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(8)
            subprocess.run([executable, "--stop"], env=env, check=True, capture_output=True, timeout=10)


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
