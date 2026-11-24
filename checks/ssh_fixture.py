"""Two ends of a local encrypted SSH test connection. POSIX test-only fixture."""
import contextlib
import fcntl
import os
import pty
import select
import signal
import socket
import struct
import subprocess
import sys
import termios
import threading

import paramiko


class BashFixture:
    def __init__(self):
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen()
        self.port = self.listener.getsockname()[1]
        self.key = paramiko.RSAKey.generate(2048)
        self.transports = []
        self.interactive = False
        self.password = "fixture-secret"
        threading.Thread(target=self.accept, daemon=True).start()

    def accept(self):
        while True:
            try:
                client, _ = self.listener.accept()
            except OSError:
                return
            transport = paramiko.Transport(client)
            self.transports.append(transport)
            threading.Thread(target=self.serve, args=(transport,), daemon=True).start()

    def serve(self, transport):
        master, slave = pty.openpty()
        shell = threading.Event()

        interactive=self.interactive
        expected_password=self.password
        class Server(paramiko.ServerInterface):
            def check_auth_password(self, username, password):
                return paramiko.AUTH_SUCCESSFUL if not interactive and password == expected_password else paramiko.AUTH_FAILED

            def get_allowed_auths(self, username):
                return "keyboard-interactive" if interactive else "password"

            def check_auth_interactive(self, username, submethods):
                return paramiko.InteractiveQuery("Sign in", "", ("Password: ",False))

            def check_auth_interactive_response(self, responses):
                return paramiko.AUTH_SUCCESSFUL if responses == [expected_password] else paramiko.AUTH_FAILED

            def check_channel_request(self, kind, chanid):
                return paramiko.OPEN_SUCCEEDED if kind == "session" else paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED

            def check_channel_pty_request(self, channel, term, cols, rows, px, py, modes):
                return self.check_channel_window_change_request(channel, cols, rows, px, py)

            def check_channel_window_change_request(self, channel, cols, rows, px, py):
                fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
                return True

            def check_channel_shell_request(self, channel):
                shell.set()
                return True

        proc = None
        try:
            transport.add_server_key(self.key)
            transport.start_server(server=Server())
            channel = transport.accept(8)
            if channel is None or not shell.wait(8):
                return
            child = "import fcntl,termios,os; fcntl.ioctl(0,termios.TIOCSCTTY,0); os.execv('/bin/bash',['bash','--noprofile','--norc','-i'])"
            proc = subprocess.Popen([sys.executable, "-c", child], stdin=slave, stdout=slave, stderr=slave,
                                    cwd="/tmp", start_new_session=True,
                                    env=dict(os.environ, PS1="fixture$ ", BASH_SILENCE_DEPRECATION_WARNING="1",
                                             TERM="xterm-256color", HISTFILE="/dev/null"))
            while not channel.closed and proc.poll() is None:
                readable, _, _ = select.select([master, channel], [], [], .1)
                if channel in readable:
                    data = channel.recv(32768)
                    if not data:
                        break
                    while data:
                        data = data[os.write(master, data):]
                if master in readable:
                    out = os.read(master, 65536)
                    if not out:
                        break
                    channel.sendall(out)
        except (EOFError, OSError, paramiko.SSHException):
            pass
        finally:
            transport.close()
            if proc is not None:
                with contextlib.suppress(ProcessLookupError):
                    os.killpg(proc.pid, signal.SIGKILL)
                proc.wait(timeout=5)
            os.close(master)
            os.close(slave)

    def close(self):
        self.listener.close()
        for transport in self.transports:
            transport.close()
