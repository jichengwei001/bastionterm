use crate::service::{specs, tool_schema, validate};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{BufRead, Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpStream},
    path::Path,
    time::{Duration, Instant},
};

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Endpoint {
    pub port: u16,
    token: String,
    pid: u32,
}
impl Endpoint {
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
    fn request(&self, action: Option<&str>, data: &Value) -> Result<Value> {
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.port);
        let mut stream = TcpStream::connect_timeout(&addr.into(), Duration::from_millis(700))?;
        stream.set_read_timeout(Some(Duration::from_secs(if action.is_some() {
            65
        } else {
            2
        })))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        if let Some(action) = action {
            let bytes = serde_json::to_vec(data)?;
            write!(
                stream,
                "POST /api/{action} HTTP/1.1\r\nHost: {addr}\r\nOrigin: http://{addr}\r\nX-Terminal-Token: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                self.token,
                bytes.len()
            )?;
            stream
                .write_all(&bytes)
                .context("请求传输中断，执行结果未知。请检查任务状态。")?;
        } else {
            write!(
                stream,
                "GET /health HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
            )?;
        }
        let read_response = || -> Result<(bool, Value)> {
            let mut response = Vec::new();
            stream.take(32_000_000).read_to_end(&mut response)?;
            let cut = response
                .windows(4)
                .position(|s| s == b"\r\n\r\n")
                .context("无效的后台响应")?;
            let value = serde_json::from_slice(&response[cut + 4..])?;
            Ok((response.starts_with(b"HTTP/1.1 200 "), value))
        };
        let (ok, value) = read_response()
            .context("后台响应丢失，执行结果未知。请先检查会话和任务状态，勿自动重放命令。")?;
        if !ok {
            bail!("{}", value["error"].as_str().unwrap_or("后台请求失败"));
        }
        Ok(value)
    }
    pub fn action(&self, name: &str, args: &Value) -> Result<Value> {
        self.request(Some(name), args)
    }
    pub fn find(dir: &Path) -> Option<Self> {
        let e: Self =
            serde_json::from_slice(&std::fs::read(dir.join("service.json")).ok()?).ok()?;
        let health = e.request(None, &Value::Null).ok()?;
        (health["service"] == "bastion-bridge-gpui"
            && health["pid"] == e.pid
            && health["workspace"] == dir.to_str()?)
        .then_some(e)
    }
    pub fn stop(dir: &Path) -> Result<()> {
        let Some(service) = Self::find(dir) else {
            return Ok(());
        };
        service.action("shutdown", &json!({}))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while std::fs::read(dir.join("service.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Self>(&b).ok())
            .is_some_and(|e| e == service)
        {
            ensure!(Instant::now() < deadline, "后台未在限时内停止");
            std::thread::sleep(Duration::from_millis(30));
        }
        Ok(())
    }
    pub fn ensure(dir: &Path) -> Result<Self> {
        Self::ensure_until_quit(dir, || false)
    }
    pub fn ensure_until_quit(dir: &Path, quitting: impl Fn() -> bool) -> Result<Self> {
        ensure!(!quitting(), "应用正在退出");
        if let Some(e) = Self::find(dir) {
            return Ok(e);
        }
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("startup.lock"))?;
        loop {
            ensure!(!quitting(), "应用正在退出");
            match fs2::FileExt::try_lock_exclusive(&lock) {
                Ok(()) => break,
                Err(e) if e.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
                    std::thread::sleep(Duration::from_millis(60));
                }
                Err(e) => return Err(e.into()),
            }
        }
        if let Some(e) = Self::find(dir) {
            return Ok(e);
        }
        let mut child = crate::background::spawn_service()?;
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            if let Some(e) = Self::find(dir) {
                #[cfg(unix)]
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return Ok(e);
            }
            if child.try_wait()?.is_some() {
                bail!("后台启动失败，请查看 {}", dir.join("service.log").display());
            }
            if Instant::now() >= deadline {
                child.kill().context("后台启动超时，未能停止启动进程")?;
                #[cfg(unix)]
                child.wait()?;
                bail!("后台启动超时，请重新尝试");
            }
            std::thread::sleep(Duration::from_millis(60));
        }
    }
}
const INSTRUCTIONS: &str = concat!(
    "Use ssh_list_profiles, choose the requested profile, then ssh_connect(profile_id). ",
    "This starts or reuses the local service. Return web_url to the user. ",
    "If credentials or a profile are missing, use ssh_start_web and ask the user to configure the host there. ",
    "Use ssh_list_sessions to find existing sessions. Pass the target session_id to status, execute, poll, cancel and disconnect. ",
    "Browser tab selection does not select an MCP target. If owner is human, wait for the user to return control. ",
    "Never open another SSH connection to bypass ownership. Poll running jobs with their id and offset. ",
    "A lost connection leaves command outcomes uncertain. Never automatically replay commands. ",
    "Reconnecting creates a new shell. Verify the working directory, environment and remote state."
);

fn rpc_error(id: &Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

fn rpc_response(dir: &Path, line: &str) -> Option<Value> {
    let q: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(_) => return Some(rpc_error(&Value::Null, -32600, "Invalid JSON request")),
    };
    let id = q.get("id")?;
    let result = match q["method"].as_str() {
        Some("initialize") => json!({
            "protocolVersion":q["params"]["protocolVersion"].as_str().unwrap_or("2024-11-05"),
            "capabilities":{"tools":{}},
            "serverInfo":{"name":"baoleiji-pty","version":env!("CARGO_PKG_VERSION")},
            "instructions":INSTRUCTIONS
        }),
        Some("ping") => json!({}),
        Some("tools/list") => specs().clone(),
        Some("tools/call") => {
            let Some(name) = q["params"]["name"].as_str() else {
                return Some(rpc_error(id, -32600, "Missing tool name"));
            };
            let Ok(schema) = tool_schema(name) else {
                return Some(rpc_error(id, -32600, "Unknown tool"));
            };
            let empty = json!({});
            let args = q["params"].get("arguments").unwrap_or(&empty);
            match validate(schema, args).and_then(|()| {
                Endpoint::ensure(dir)?.action("mcp-call", &json!({"name":name,"arguments":args}))
            }) {
                Ok(v) => json!({"content":[{"type":"text","text":v.to_string()}]}),
                Err(e) => json!({"isError":true,"content":[{"type":"text","text":e.to_string()}]}),
            }
        }
        None => return Some(rpc_error(id, -32600, "Missing method")),
        Some(_) => return Some(rpc_error(id, -32601, "Method not found")),
    };
    Some(json!({"jsonrpc":"2.0","id":id,"result":result}))
}

pub fn stdio(dir: &Path) -> Result<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        if let Some(reply) = rpc_response(dir, &line?) {
            writeln!(out, "{reply}")?;
            out.flush()?;
        }
    }
    Ok(())
}
