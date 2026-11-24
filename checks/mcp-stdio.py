"""Check a packaged MCP process through stdin/stdout in a fresh data directory."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def check(executable):
    frames = [
        "{",
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"id": 1},
        {"id": 2, "method": "no-such-method"},
        {"id": 3, "method": "tools/call", "params": []},
        {"id": 4, "method": "tools/call", "params": {"name": "no-such-tool"}},
        {"id": 5, "method": "tools/call", "params": {"name": "ssh_start_web", "arguments": None}},
        {"id": 6, "method": "tools/call", "params": {"name": "ssh_status", "arguments": {"session_id": ""}}},
        {"id": 7, "method": "initialize", "params": {"protocolVersion": "2024-11-05"}},
        {"id": 8, "method": "tools/list"},
        {"id": 9, "method": "ping"},
    ]
    with tempfile.TemporaryDirectory() as tmp:
        env = dict(os.environ, BASTION_GPUI_HOME=tmp, SHARED_TERMINAL_HOME=tmp)
        def run(lines):
            data = "".join((x if isinstance(x, str) else json.dumps({"jsonrpc": "2.0", **x})) + "\n" for x in lines)
            output = subprocess.check_output([executable, "--mcp"], input=data, env=env,
                                             encoding="utf-8", timeout=30)
            return [json.loads(line) for line in output.splitlines()]
        try:
            replies = run(frames)
            assert [r["id"] for r in replies] == [None, *range(1, 10)]
            assert [r["error"]["code"] for r in replies[:5]] == [-32600, -32600, -32601, -32600, -32600]
            assert all(r["result"]["isError"] for r in replies[5:7])
            assert "ssh_list_profiles" in replies[7]["result"]["instructions"]
            assert len(replies[8]["result"]["tools"]) == 9 and replies[9]["result"] == {}
            assert not (Path(tmp) / "service.json").exists()
            response, = run([{"id": 10, "method": "tools/call", "params": {"name": "ssh_list_sessions"}}])
            assert json.loads(response["result"]["content"][0]["text"])["sessions"] == []
            assert (Path(tmp) / "service.json").is_file()
            def call(ident, name, **args):
                return {"id": ident, "method": "tools/call", "params": {"name": name, "arguments": args}}
            web, profile, session, listing = [r["result"] for r in run([
                call(11, "ssh_start_web"), call(12, "ssh_connect", profile_id="deleted"),
                call(13, "ssh_status", session_id="closed"), call(14, "ssh_list_sessions"),
            ])]
            url = json.loads(web["content"][0]["text"])["url"]
            assert profile["isError"] and "ssh_list_profiles" in profile["content"][0]["text"]
            assert url in profile["content"][0]["text"]
            assert session["isError"] and "ssh_list_sessions" in session["content"][0]["text"]
            assert json.loads(listing["content"][0]["text"])["sessions"] == []
        finally:
            if (Path(tmp) / "service.json").is_file():
                subprocess.run([executable, "--stop"], env=env, check=True, capture_output=True, timeout=15)


if __name__ == "__main__":
    check(sys.argv[1])
    print("PASS MCP stream recovery, error routing, notifications, schema validation and lazy service startup")
