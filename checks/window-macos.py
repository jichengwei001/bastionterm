"""Exercise the extracted macOS bundle through its normal desktop entry point.
Requires System Events accessibility permission for the window close check.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

exe=Path(sys.argv[1]).resolve()
with tempfile.TemporaryDirectory(prefix="垒桥终端 GUI ") as directory:
    data=Path(directory)
    env=dict(os.environ, BASTION_GPUI_HOME=directory)
    proc=None
    def script(body):
        return subprocess.check_output(["osascript","-e",body],text=True,timeout=8).strip()
    try:
        # First start just MCP; no GUI process has been launched.
        cfg=json.loads(subprocess.check_output([exe,"--ensure"],env=env,text=True))
        service=json.loads((data/"service.json").read_text())
        assert cfg["url"].endswith(":"+str(service["port"]))
        proc=subprocess.Popen([exe],env=env)
        prefix='tell application "System Events" to tell first application process whose unix id is '+str(proc.pid)+' to '
        for _ in range(60):
            if proc.poll() is not None:raise RuntimeError("window process exited")
            try:
                if "BastionTerm" in script(prefix+'get name of window 1'):break
            except subprocess.CalledProcessError:pass
            time.sleep(.1)
        else:raise RuntimeError("window title missing")
        script(prefix+'set frontmost to true')

        def key(code):
            if script(prefix+'get frontmost') != 'true':
                raise RuntimeError("Test window lost keyboard focus")
            script(prefix+'key code '+str(code))
            time.sleep(.12)

        # Enter the skin picker, apply Paper, dismiss it, and re-open the same
        # picker from the restored button. Both changes must reach the service home.
        key(48)
        key(36)
        for _ in range(4):
            key(48)
        key(36)
        assert json.loads((data/"appearance.json").read_text())["skin"]=="paper"
        key(53)
        key(36)
        key(48)
        key(36)
        assert json.loads((data/"appearance.json").read_text())["skin"]=="tundra"
        key(53)
        print("PASS toolbar keyboard navigation, modal focus restoration and persisted skin")
        # From the skin button, open connection settings in a short window.
        # The reverse tab from the dismiss button selects the off-screen Connect action.
        script(prefix+'set size of window 1 to {580, 400}')
        for _ in range(30):
            if script(prefix+'get size of window 1') == '640, 480':
                break
            time.sleep(.1)
        else:
            raise AssertionError("decorated window did not reach the original 640 × 480 minimum")
        script(prefix+'set size of window 1 to {660, 530}')
        key(48); key(48); key(36)
        script(prefix+'key code 48 using shift down'); time.sleep(.2)
        key(36)  # Empty required fields bring focus back to Host.
        script(prefix+'keystroke "127.0.0.1"')
        key(48); key(48)
        # Numeric fixture values are committed immediately by the macOS Pinyin IME.
        script(prefix+'keystroke "12345"')
        key(48)
        script(prefix+'keystroke "67890"')
        key(36)  # Enter in a field saves the profile.
        profiles=json.loads((data/"profiles.json").read_text())["profiles"]
        assert [(p["host"],p["user"]) for p in profiles]==[("127.0.0.1","12345")]
        assert not (data/"sessions.json").exists(), "form validation or save opened a connection"
        key(53)
        print("PASS small-window keyboard navigation, required-field focus and profile save")
        # From the restored connection settings button, tab through Add, Copy
        # and Clear to the terminal fullscreen button, even with no SSH session.
        original = script(prefix+'get size of window 1')
        for _ in range(4):
            key(48)
        key(36)
        fullscreen = 'get value of attribute "AXFullScreen" of window 1'
        def await_fullscreen(active, expected_size=None):
            deadline=time.monotonic()+6
            while time.monotonic()<deadline:
                if proc.poll() is not None:
                    raise RuntimeError("App exited during fullscreen transition")
                try:
                    if script(prefix+fullscreen)==active and (expected_size is None or
                            script(prefix+'get size of window 1')==expected_size):
                        return
                except subprocess.CalledProcessError:
                    # AppKit can temporarily remove the window from the AX tree during animation.
                    pass
                time.sleep(.1)
            raise AssertionError(f"window did not reach fullscreen={active}, size={expected_size}")
        await_fullscreen('true')
        key(53)
        await_fullscreen('false',original)
        print("PASS keyboard fullscreen in observer mode and Escape restores window size")
        script(prefix+'click (first button of window 1 whose subrole is "AXCloseButton")')
        time.sleep(.5)
        assert script(prefix+'get value of attribute "AXMinimized" of window 1')=='true'
        assert json.loads((data/"service.json").read_text())["pid"]==service["pid"]
        subprocess.run([exe],env=env,check=True,timeout=6)
        for _ in range(30):
            if script(prefix+'get value of attribute "AXMinimized" of window 1')=='false':break
            time.sleep(.12)
        else:raise RuntimeError("second launch did not restore window")
        script(prefix+'set frontmost to true')
        script(prefix+'keystroke "q" using command down')
        proc.wait(8)
        assert not (data/"service.json").exists(), "GUI quit left the headless service running"
        print("PASS MCP-only start, GUI attachment, close minimizes, relaunch restores the same window, explicit Quit stops SSH daemon")
    finally:
        if proc and proc.poll() is None:proc.terminate();proc.wait(5)
        subprocess.run([exe,"--stop"],env=env,capture_output=True,timeout=8)
