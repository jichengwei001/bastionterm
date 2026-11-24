"""End-to-end parity check on macOS/Linux with two real encrypted Bash PTYs.
Run from the development checkout with the desktop test dependencies.
"""
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from urllib.error import HTTPError
from urllib.request import Request, ProxyHandler, build_opener

ROOT = Path(__file__).resolve().parents[1]
from ssh_fixture import BashFixture

EXE = Path(os.environ.get("BASTION_TEST_EXE", ROOT / "target/release/bastion-bridge-gpui")).resolve()


class NativeParity(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.dir = Path(tmp.name)
        self.env = dict(os.environ, BASTION_GPUI_HOME=tmp.name)
        self.servers = [BashFixture(), BashFixture()]
        for server in self.servers:
            self.addCleanup(server.close)
        self.addCleanup(self.stop)
        self.opener = build_opener(ProxyHandler({}))
        self.start()

    def start(self):
        self.url = self.tool("ssh_start_web")["url"]
        self.token = json.loads((self.dir / "service.json").read_text())["token"]

    def stop(self):
        subprocess.run([EXE, "--stop"], env=self.env, check=True, capture_output=True, timeout=10)
        for _ in range(100):
            if not (self.dir / "service.json").exists():
                return
            time.sleep(.03)
        self.fail("test daemon did not stop")

    def rpc(self, method, params):
        req = dict(jsonrpc="2.0", id=1, method=method, params=params)
        raw = subprocess.check_output([EXE, "--mcp"], env=self.env, text=True, input=json.dumps(req)+"\n", timeout=65)
        return json.loads(raw)["result"]

    def tool(self, name, **args):
        result = self.rpc("tools/call", dict(name=name, arguments=args))
        if result.get("isError"):
            raise ValueError(result["content"][0]["text"])
        return json.loads(result["content"][0]["text"])

    def action(self, method, origin=None, **args):
        req = Request(self.url+"/api/"+method, data=json.dumps(args).encode(), headers={
            "Content-Type":"application/json", "Origin":origin or self.url, "X-Terminal-Token":self.token})
        with self.opener.open(req, timeout=60) as res:
            return json.load(res)

    def profile(self, i, name="Test"):
        return self.action("profile-save", name=name, host="127.0.0.1", port=self.servers[i].port,
                           user="fixture", password="fixture-secret", remember_password=True)

    def test_routing_control_history_and_recovery(self):
        defs=self.rpc("tools/list", {})["tools"]
        self.assertEqual(len(defs), 9)
        a,b=self.profile(0,"Host A"),self.profile(1,"Host B")
        alias=self.profile(0,"Alias")
        self.assertNotIn("fixture-secret",(self.dir/"profiles.json").read_text())
        with concurrent.futures.ThreadPoolExecutor(6) as pool:
            connections=list(pool.map(lambda p:self.tool("ssh_connect",profile_id=p["id"]),[a,b,alias,a,b,alias]))
        aid,bid=(r["session_id"] for r in connections[:2])
        self.assertNotEqual(aid,bid)
        self.assertEqual(connections[0]["connection_id"],connections[2]["connection_id"])
        self.assertEqual([len(s.transports) for s in self.servers],[1,1])
        execute=lambda id,cmd,wait=5:self.tool("ssh_execute",session_id=id,command=cmd,wait=wait)
        run=execute(aid,'export PARITY=一; cd /tmp; printf start; sleep 30',0)
        self.assertEqual(run["status"],"running")
        for _ in range(40):
            seen=self.tool("ssh_poll",id=run["id"],session_id=aid)
            if seen["output"]=="start":break
            time.sleep(.03)
        self.assertEqual(seen["output"],"start")
        with self.assertRaises(ValueError):
            execute(aid,"false",0)
        self.assertEqual(execute(bid,'printf %s "${PARITY:-different}"')["output"],"different")
        taken=self.action("takeover",session_id=aid)
        self.assertEqual(taken["state"],"manual")
        self.assertEqual(self.tool("ssh_poll",session_id=aid,id=run["id"])["exit_code"],130)
        self.assertEqual(self.tool("ssh_connect",profile_id=a["id"])["owner"],"human")
        for name,args in [("ssh_disconnect",{}),("ssh_execute",{"command":"false"}),("ssh_cancel",{"id":run["id"]})]:
            with self.assertRaises(ValueError):self.tool(name,session_id=aid,**args)
        self.action("input",session_id=aid,data="export MANUAL=human\r")
        time.sleep(.3)
        self.action("release",session_id=aid)
        self.assertEqual(execute(aid,'printf "%s:%s:%s" "$PWD" "$PARITY" "$MANUAL"')["output"],"/tmp:一:human")
        run=execute(aid,"sleep 20",0)
        self.assertEqual(self.tool("ssh_cancel",session_id=aid,id=run["id"])["exit_code"],130)
        bigcmd="printf 好; false\n#"+"x"*50000
        result=execute(aid,bigcmd)
        self.assertEqual((result["exit_code"],result["output"],result["offset"]),(1,"好",1))
        run=execute(aid,"yes a | head -c 2100000 | tr -d '\\n' | tr a b; printf END")
        self.assertTrue(run["output"].endswith("END"))
        large=execute(aid,"head -c 2050000 /dev/zero | tr '\\0' x",25)
        self.assertEqual((len(large["output"]),large["offset"],large["truncated"]),(2_000_000,2_050_000,True))
        self.assertEqual(self.tool("ssh_poll",session_id=aid,id=large["id"],offset=2049999)["output"],"x")
        with self.assertRaises(ValueError):self.tool("ssh_poll",session_id=bid,id=run["id"])
        self.action("resize",session_id=aid,cols=99,rows=37)
        self.assertEqual(execute(aid,"stty size")["output"].strip(),"37 99")
        first=execute(aid,"echo once")
        self.tool("ssh_disconnect",session_id=aid)
        reconnected=self.action("reconnect",session_id=aid)
        self.assertNotEqual(reconnected["connection_id"],connections[0]["connection_id"])
        self.assertEqual(self.tool("ssh_poll",session_id=aid,id=first["id"])["output"],"once\r\n")
        self.action("profile-delete",id=a["id"])
        self.assertEqual(self.tool("ssh_status",session_id=aid)["state"],"ready")
        self.action("close-session",session_id=aid)
        self.assertEqual([s["session_id"] for s in self.tool("ssh_list_sessions")["sessions"]],[bid])
        self.stop();self.start()
        self.assertEqual(self.tool("ssh_status",session_id=bid)["state"],"disconnected")
        self.assertEqual(len(self.servers[1].transports),1)

    def test_old_terminal_offset_after_closing_and_reopening_host(self):
        profile = self.profile(0)
        first = self.tool("ssh_connect", profile_id=profile["id"])
        sid = first["session_id"]
        identity = json.dumps(["127.0.0.1", self.servers[0].port, "fixture"]).encode()
        self.assertEqual(sid, "s_" + hashlib.sha256(identity).hexdigest()[:20])
        self.tool("ssh_execute", session_id=sid, command="printf '%2048s' OLD")
        old_offset = self.action("snapshot", session_id=sid)["offset"]
        self.action("close-session", session_id=sid)
        second = self.tool("ssh_connect", profile_id=profile["id"])
        self.assertEqual(second["session_id"], sid)
        self.assertNotEqual(second["connection_id"], first["connection_id"])
        job = self.tool("ssh_execute", session_id=sid, command="printf '新😀'")
        current = self.action("snapshot", session_id=sid)
        self.assertLess(current["offset"], old_offset)
        self.assertIn("新😀", current["data"])
        self.assertNotIn("OLD", current["data"])
        recovered = self.action("snapshot", session_id=sid, offset=old_offset)
        self.assertEqual(recovered["data"], current["data"])
        self.assertFalse(recovered["truncated"])
        self.assertEqual(self.action("snapshot", session_id=sid, offset=recovered["offset"])["data"], "")
        self.assertEqual(self.tool("ssh_poll", session_id=sid, id=job["id"], offset=old_offset)["output"], "")

    def test_upgrade_keeps_existing_session_id(self):
        profile = self.profile(0)
        current = self.tool("ssh_connect", profile_id=profile["id"])
        self.stop()
        path = self.dir / "sessions.json"
        entries = json.loads(path.read_text())
        legacy_id = "s_91cb90aea3b74f56ada6fb6f655d698b"
        self.assertEqual(entries[0]["session_id"], current["session_id"])
        entries[0]["session_id"] = legacy_id
        path.write_text(json.dumps(entries))
        self.start()
        self.assertEqual(self.tool("ssh_status", session_id=legacy_id)["state"], "disconnected")
        reopened = self.tool("ssh_connect", profile_id=profile["id"])
        self.assertEqual(reopened["session_id"], legacy_id)
        self.assertEqual(len(self.tool("ssh_list_sessions")["sessions"]), 1)

    def test_foreground_program_keeps_output_and_control_after_failed_handoff(self):
        import shlex
        session = self.tool("ssh_connect", profile_id=self.profile(0)["id"])
        sid = session["session_id"]

        def transcript():
            return self.action("snapshot", session_id=sid)["data"]

        def wait_for(text):
            for _ in range(100):
                if text in transcript():
                    return
                time.sleep(.04)
            self.fail(f"Missing foreground output {text!r}")

        initial = transcript()
        self.assertIn("--- SSH connected ---", initial)
        self.action("takeover", session_id=sid)
        program = (
            'trap "" INT; stty -echo; printf "APP_%s\\n" READY; '
            'while IFS= read -r line; do '
            '[ -z "$line" ] && continue; [ "$line" = leave ] && break; '
            'printf "DROP_%s" PRELUDE; head -c 140000 /dev/zero | tr "\\000" "."; '
            'printf "HANDOFF_%s\\n" PENDING; done; stty echo; printf "APP_%s\\n" CLOSED'
        )
        self.action("input", session_id=sid, data="bash -c " + shlex.quote(program) + "\r")
        wait_for("APP_READY")
        with self.assertRaises(HTTPError):
            self.action("release", session_id=sid)
        self.assertEqual(self.tool("ssh_status", session_id=sid)["owner"], "human")
        retained = transcript()
        self.assertEqual(retained.count("HANDOFF_PENDING"), 1)
        self.assertNotIn("DROP_PRELUDE", retained)
        self.assertIn("." * 65000 + "HANDOFF_PENDING", retained)
        self.assertNotIn("--- Agent control ---", initial)
        self.assertNotIn("--- Agent control ---", transcript())
        self.action("input", session_id=sid, data="leave\r")
        wait_for("APP_CLOSED")
        self.action("release", session_id=sid)
        self.assertEqual(transcript().count("--- Agent control ---"), 1)
        self.assertEqual(self.tool("ssh_execute", session_id=sid, command="printf resumed")["output"], "resumed")

    def test_outstanding_polls_follow_the_job_across_reconnect(self):
        profile = self.profile(0)
        first = self.tool("ssh_connect", profile_id=profile["id"])
        sid = first["session_id"]
        job = self.tool("ssh_execute", session_id=sid, command="printf early; sleep 15", wait=0)["id"]
        for _ in range(200):
            seen = self.action("mcp-call", name="ssh_poll", arguments={"session_id": sid, "id": job})
            if seen["output"] == "early":
                break
            time.sleep(.01)
        self.assertEqual(seen["output"], "early")
        start = threading.Barrier(13)

        def poll():
            start.wait(timeout=5)
            return self.action("mcp-call", name="ssh_poll", arguments={
                "session_id": sid, "id": job, "wait": 5, "offset": 1})

        with concurrent.futures.ThreadPoolExecutor(12) as pool:
            pending = [pool.submit(poll) for _ in range(12)]
            start.wait(timeout=5)
            time.sleep(.12)
            self.action("mcp-call", name="ssh_disconnect", arguments={"session_id": sid})
            second = self.action("mcp-call", name="ssh_connect", arguments={"profile_id": profile["id"]})
            self.assertEqual(second["session_id"], sid)
            self.assertNotEqual(second["connection_id"], first["connection_id"])
            for future in pending:
                result = future.result(timeout=8)
                self.assertEqual((result["status"], result["output"], result["offset"]),
                                 ("disconnected", "arly", 5))
                self.assertIn("error", result)
        retained = self.tool("ssh_poll", session_id=sid, id=job, offset=-1)
        self.assertEqual((retained["output"], retained["truncated"]), ("early", True))

    def test_form_switches_reconnect_profile_without_replacing_shell(self):
        first = self.profile(0, "First")
        second = self.profile(0, "Second")
        session = self.tool("ssh_connect", profile_id=first["id"])
        sid = session["session_id"]
        self.action("takeover", session_id=sid)
        probe = self.tool("ssh_connect", profile_id=second["id"])
        self.assertEqual((probe["next_action"], probe["profile_id"]), ("wait_for_user", first["id"]))
        selected = self.action("connect", profile_id=second["id"])
        self.assertEqual((selected["name"], selected["profile_id"]), ("Second", second["id"]))
        self.assertEqual((selected["session_id"], selected["connection_id"], selected["owner"]),
                         (sid, session["connection_id"], "human"))
        self.assertEqual(len(self.servers[0].transports), 1)
        self.action("profile-delete", id=first["id"])
        self.servers[0].transports[0].close()
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            offline = self.tool("ssh_status", session_id=sid)
            if offline["state"] == "disconnected":
                break
        self.assertEqual((offline["state"], offline["owner"]), ("disconnected", "human"))
        with self.assertRaises(ValueError):
            self.tool("ssh_disconnect", session_id=sid)
        with self.assertRaises(HTTPError):
            self.action("takeover", session_id=sid)
        self.stop()
        self.start()
        self.assertEqual(self.tool("ssh_status", session_id=sid)["profile_id"], second["id"])
        resumed = self.action("reconnect", session_id=sid)
        self.assertEqual((resumed["state"], resumed["owner"], resumed["session_id"]), ("ready", "agent", sid))
        self.assertNotEqual(resumed["connection_id"], session["connection_id"])
        self.assertEqual(self.tool("ssh_execute", session_id=sid, command="printf resumed")["output"], "resumed")

    def test_keyboard_interactive_and_changed_host_key(self):
        import paramiko
        self.servers[0].interactive=True
        profile=self.profile(0)
        session=self.tool("ssh_connect",profile_id=profile["id"])
        self.tool("ssh_disconnect",session_id=session["session_id"])
        self.servers[0].key=paramiko.RSAKey.generate(2048)
        with self.assertRaises(ValueError):
            self.tool("ssh_connect",profile_id=profile["id"])
        self.assertEqual(self.tool("ssh_status",session_id=session["session_id"])["state"],"disconnected")

    def test_six_hosts_and_twelve_mcp_clients(self):
        for _ in range(4):
            server=BashFixture()
            self.servers.append(server)
            self.addCleanup(server.close)
        profiles=[self.profile(i,"Node "+str(i)) for i in range(6)]
        with concurrent.futures.ThreadPoolExecutor(12) as pool:
            connections=list(pool.map(lambda p:self.tool("ssh_connect", profile_id=p["id"]), profiles*2))
            ids=[s["session_id"] for s in connections]
            self.assertEqual(ids[:6],ids[6:])
            self.assertEqual(len(set(ids)),6)
            def start(i):
                try:
                    r=self.tool("ssh_execute",session_id=ids[i],command=f"export PER_HOST={i}; sleep .8; printf %s $PER_HOST",wait=0)
                    return i,r["id"]
                except ValueError as e:
                    self.assertIn("正在执行",str(e))
                    return None
            started=[job for job in pool.map(start,range(12)) if job]
            self.assertEqual({ids[i] for i,_ in started},set(ids))
            self.assertEqual(len(started),6)
            for i,job in started:
                self.assertEqual(self.tool("ssh_poll",session_id=ids[i],id=job,wait=5)["output"],str(i))
        self.assertEqual([len(s.transports) for s in self.servers],[1]*6)

    def test_profile_target_binding_and_local_web(self):
        p=self.profile(0)
        for invalid in [None, False, 22, [], {}, "密"*1366]:
            with self.subTest(password_type=type(invalid).__name__):
                with self.assertRaises(HTTPError) as caught:
                    self.action("profile-save",**dict(p,password=invalid,remember_password=False))
                self.assertEqual(json.load(caught.exception)["error"],"密码格式无效或长度超过限制")
                self.assertTrue(self.action("profiles")["profiles"][0]["has_password"])
        with self.assertRaises(HTTPError) as denied:
            self.action("profiles", origin="https://example.invalid")
        self.assertEqual(denied.exception.code, 403)
        self.assertEqual(json.load(denied.exception)["error"], "Invalid origin or token")
        self.assertEqual(self.action("profile-save",**dict(p,password="",remember_password=True))["has_password"],True)
        with self.assertRaises(HTTPError):self.action("profile-save",**dict(p,port=self.servers[1].port,password="",remember_password=True))
        self.action("profile-save",**dict(p,password="",remember_password=False))
        self.assertFalse(self.tool("ssh_list_profiles")["profiles"][0]["has_password"])
        self.assertNotIn("fixture-secret",(self.dir/"credentials.json").read_text())
        with self.assertRaises(HTTPError) as missing:
            self.action("connect",profile_id=p["id"])
        self.assertIn("已保存密码",json.load(missing.exception)["error"])
        self.assertEqual(self.servers[0].transports,[])
        self.servers[0].password=""
        direct=self.action("connect",name="临时",host="127.0.0.1",port=p["port"],user="fixture",password="")
        self.assertEqual(self.tool("ssh_connect",profile_id=p["id"])["connection_id"],direct["connection_id"])
        self.assertEqual(self.tool("ssh_execute",session_id=direct["session_id"],command="printf direct")["output"],"direct")
        deleted=self.action("profile-delete",id=p["id"])
        self.assertEqual((deleted["profiles"],deleted["default_id"]),([],None))
        self.assertEqual(self.tool("ssh_status",session_id=direct["session_id"])["state"],"ready")
        html=self.opener.open(self.url).read().decode()
        self.assertIn(self.token,html)
        self.assertIn('id="history-sidebar"',html)
        for asset in ["style.css","skins.js","vendor/xterm.js","vendor/fit.js","fonts/JetBrainsMono-Regular.woff2"]:
            with self.opener.open(self.url+"/"+asset) as r:self.assertEqual(r.status,200)
        with self.assertRaises(ValueError):self.tool("ssh_execute",command="false")
        with self.assertRaises(ValueError):self.tool("ssh_cancel",session_id="s",id="i",source="human")


if __name__=="__main__":unittest.main()
