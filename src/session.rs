use crate::{store::Profile, transcript::Transcript};
use anyhow::{Context, Result, anyhow, bail, ensure};
use base64::Engine;
use indexmap::IndexMap;
use russh::{
    ChannelMsg, client,
    keys::{
        known_hosts::{check_known_hosts_path, known_host_keys_path, learn_known_hosts_path},
        ssh_key::PublicKey,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{runtime::Runtime, sync::mpsc};
use uuid::Uuid;

const HANDOFF_BUFFER: usize = 65_536;

fn target_id(host: &str, port: u16, user: &str) -> String {
    // Match the original desktop broker's JSON array including its separator spaces.
    // Host and user are validated ASCII identifiers that do not require JSON escaping.
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let identity = format!(r#"["{host}", {port}, "{user}"]"#);
    let digest = format!("{:x}", Sha256::digest(identity.as_bytes()));
    format!("s_{}", &digest[..20])
}

pub type Shared = Arc<Mutex<Session>>;

pub enum ConnectOrigin<'a> {
    Agent,
    Form,
    Reconnect(&'a Shared),
}
pub struct Hub {
    pub sessions: Mutex<IndexMap<String, Shared>>,
    pub dir: PathBuf,
    rt: Runtime,
}
struct HostKey {
    host: String,
    port: u16,
    file: PathBuf,
}
static KEY_LOCK: Mutex<()> = Mutex::new(());
impl client::Handler for HostKey {
    type Error = anyhow::Error;
    async fn check_server_key(&mut self, key: &PublicKey) -> Result<bool> {
        let _guard = KEY_LOCK.lock().unwrap();
        let system = dirs::home_dir().map(|home| home.join(".ssh/known_hosts"));
        let host = self.host.trim_end_matches('.').to_ascii_lowercase();
        let mut known = false;
        for file in system.iter().chain(std::iter::once(&self.file)) {
            for candidate in [&host, &self.host] {
                if !known_host_keys_path(candidate, self.port, file)?.is_empty() {
                    ensure!(
                        check_known_hosts_path(candidate, self.port, key, file)?,
                        "SSH 主机密钥已改变，请核对主机身份"
                    );
                    known = true;
                }
                if *candidate == self.host && host == self.host {
                    break;
                }
            }
        }
        if !known {
            learn_known_hosts_path(&host, self.port, key, &self.file)?;
        }
        Ok(true)
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub session_id: String,
    pub profile_id: Option<String>,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
}

enum Wire {
    Data(Vec<u8>),
    Resize(u16, u16),
    Close,
}

pub struct Session {
    pub entry: Entry,
    pub phase: String,
    pub manual: bool,
    pub revision: u64,
    geometry: (u16, u16),
    terminal: Transcript,
    screen: Option<vt100::Parser>,
    skin: crate::theme::Theme,
    tx: mpsc::UnboundedSender<Wire>,
    jobs: VecDeque<Arc<Job>>,
    upload: VecDeque<Vec<u8>>,
    sync: Option<String>,
    pending: Vec<u8>,
    pub connection_id: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone)]
struct Job {
    id: String,
    command: String,
    started: f64,
    ended: Option<f64>,
    begun: bool,
    output: Transcript,
    status: &'static str,
    code: Option<i32>,
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
fn pos(data: &[u8], needle: &[u8]) -> Option<usize> {
    data.windows(needle.len()).position(|w| w == needle)
}
fn safe_prefix(data: &[u8], marker: &[u8]) -> usize {
    let keep = (1..marker.len().min(data.len() + 1))
        .rev()
        .find(|n| data.ends_with(&marker[..*n]))
        .unwrap_or(0);
    data.len() - keep
}
impl Session {
    fn new(entry: Entry, tx: mpsc::UnboundedSender<Wire>) -> Self {
        Self {
            entry,
            phase: "disconnected".into(),
            manual: false,
            revision: 0,
            geometry: (160, 40),
            terminal: Transcript::default(),
            screen: None,
            skin: Default::default(),
            tx,
            jobs: VecDeque::new(),
            upload: VecDeque::new(),
            sync: None,
            pending: vec![],
            connection_id: None,
            error: None,
        }
    }
    fn send(&self, data: Vec<u8>) -> Result<()> {
        self.tx
            .send(Wire::Data(data))
            .map_err(|_| anyhow!("SSH 已断开"))
    }
    pub fn set_skin(&mut self, skin: crate::theme::Theme, reset: bool) {
        self.skin = skin;
        if let Some(screen) = &mut self.screen {
            screen.set_palette(skin.colors(), reset)
        }
    }
    fn append(&mut self, data: &[u8]) {
        let (cols, rows) = self.geometry;
        // Retain enough history to reflow the largest supported viewport on resize.
        let screen = self.screen.get_or_insert_with(|| {
            let mut parser = vt100::Parser::new(rows, cols, 200);
            parser.set_palette(self.skin.colors(), true);
            // On reconnect the viewers replay the retained transcript for this tab.
            // Restore its cursor and modes without sending any old query back to SSH.
            let (history, _, _) = self.terminal.read(0);
            parser.process(history.as_bytes());
            parser
        });
        if self.manual && self.phase == "manual" {
            let reply = screen.process_with_replies(data);
            if !reply.is_empty() && self.send(reply).is_err() {
                self.offline();
            }
        } else {
            screen.process(data);
        }
        self.terminal.append(data, false);
        self.revision += 1;
    }
    fn active(&self) -> Option<&Job> {
        self.jobs
            .back()
            .map(Arc::as_ref)
            .filter(|j| j.status == "running")
    }
    pub fn status(&self) -> Value {
        let e = &self.entry;
        json!({"state":self.phase,"owner":if self.manual{"human"}else{"agent"},"active_id":self.active().map(|j|&j.id),"connection_id":self.connection_id,"ssh_pid":null,"target":{"host":e.host,"port":e.port,"user":e.user},"name":e.name,"profile_id":e.profile_id,"session_id":e.session_id})
    }
    pub fn read(&self, offset: usize) -> Value {
        let (data, end, truncated) = self.terminal.read_terminal(offset);
        let mut value = self.status();
        value["data"] = json!(data);
        value["offset"] = json!(end);
        value["truncated"] = json!(truncated);
        value["jobs"]=json!(self.jobs.iter().rev().map(|j|json!({"command":j.command.chars().take(2000).collect::<String>(),"id":j.id,"status":j.status,"started_at":j.started,"ended_at":j.ended,"exit_code":j.code})).collect::<Vec<_>>());
        value
    }
    pub fn input(&self, data: Vec<u8>) -> Result<()> {
        ensure!(self.manual && self.phase == "manual", "请先接管终端");
        ensure!(data.len() <= 16384, "输入过长");
        self.send(data)
    }
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        let (cols, rows) = (cols.clamp(20, 500), rows.clamp(5, 200));
        if self.phase != "disconnected"
            && self.phase != "connecting"
            && self.geometry != (cols, rows)
        {
            self.tx
                .send(Wire::Resize(cols, rows))
                .map_err(|_| anyhow!("SSH 已断开"))?;
            self.geometry = (cols, rows);
            if let Some(screen) = &mut self.screen {
                screen.set_size(rows, cols);
            }
            self.revision += 1;
        }
        Ok(())
    }
    fn begin_sync(&mut self) {
        self.sync = Some(Uuid::new_v4().simple().to_string());
        self.pending.clear();
        self.revision += 1;
    }
    fn send_sync(&self, initial: bool) -> Result<()> {
        let id = self.sync.as_deref().context("SSH 同步已中断")?;
        let setup = if initial {
            "stty -echo; exec env TERM=dumb PS1='' PS2='' PROMPT_COMMAND='' bash --noprofile --norc --noediting -i\n"
        } else {
            "stty -echo; PS1=''; PS2=''; PROMPT_COMMAND=''; set +o emacs; set +o vi; "
        };
        self.send(format!("{setup}builtin printf '\\036{id}S\\037\\n'\n").into_bytes())
    }
    pub fn execute(&mut self, command: &str) -> Result<String> {
        ensure!(
            !self.manual && self.phase == "ready",
            "主机未就绪、已接管或正在执行"
        );
        ensure!(
            !command.trim().is_empty() && command.len() <= 128000,
            "命令不能为空或超过 128 KB"
        );
        let id = Uuid::new_v4().simple().to_string();
        let encoded = base64::engine::general_purpose::STANDARD.encode(command);
        let run = format!(
            "builtin printf '\\036{id}B\\037'; builtin eval -- \"$(builtin printf %s \"$__agent_payload\" | base64 --decode)\"; builtin printf '\\036{id}E%d\\037' \"$?\"\n"
        );
        // A canonical PTY can silently discard input past its line limit. Wait for
        // the shell to accept each fragment before sending the next one.
        let mut lines = VecDeque::new();
        for (index, chunk) in encoded.as_bytes().chunks(512).enumerate() {
            let op = if index == 0 { "=" } else { "+=" };
            let chunk = std::str::from_utf8(chunk)?;
            lines.push_back(
                format!("__agent_payload{op}'{chunk}'; builtin printf '\\036{id}A\\037'\n")
                    .into_bytes(),
            );
        }
        lines.push_back(run.into_bytes());
        self.send(lines.pop_front().unwrap())?;
        self.upload = lines;
        self.append(
            format!(
                "\r\n\x1b[36magent ❯ \x1b[0m{}\r\n",
                command.replace('\n', "\r\n")
            )
            .as_bytes(),
        );
        self.jobs.push_back(Arc::new(Job {
            id: id.clone(),
            command: command.into(),
            started: now(),
            ended: None,
            begun: false,
            output: Transcript::default(),
            status: "running",
            code: None,
        }));
        if self.jobs.len() > 20 {
            self.jobs.pop_front();
        }
        self.pending.clear();
        self.phase = "busy".into();
        Ok(id)
    }
    pub fn poll(&self, id: &str, offset: usize) -> Result<Value> {
        let j = self
            .jobs
            .iter()
            .find(|j| j.id == id)
            .context("该任务不属于此会话或已过期")?;
        let (out, end, truncated) = j.output.read(offset);
        let mut result = json!({"id":id,"status":j.status,"output":out,"offset":end,"truncated":truncated,"exit_code":j.code,"session_id":self.entry.session_id});
        if j.status == "disconnected" {
            result["error"] =
                json!("SSH disconnected; command outcome unknown. Never automatically replay.");
        }
        Ok(result)
    }
    fn consume(&mut self, data: &[u8]) {
        if let Some(id) = &self.sync {
            let marker = format!("\x1e{id}S\x1f");
            let start = self.pending.len().saturating_sub(marker.len() - 1);
            self.pending.extend_from_slice(data);
            if pos(&self.pending[start..], marker.as_bytes()).is_some() {
                self.sync = None;
                self.pending.clear();
                if self.phase == "connecting" {
                    self.phase = "ready".into();
                }
            } else if self.pending.len() > 2 * HANDOFF_BUFFER {
                let cut = self.pending.len() - HANDOFF_BUFFER;
                self.pending.drain(..cut);
            }
            return;
        }
        if self.active().is_none() {
            self.append(data);
            return;
        }
        self.pending.extend_from_slice(data);
        if !self.upload.is_empty() {
            let ack = format!("\x1e{}A\x1f", self.active().unwrap().id);
            if pos(&self.pending, ack.as_bytes()).is_some() {
                self.pending.clear();
                if let Some(line) = self.upload.pop_front() {
                    if self.send(line).is_err() {
                        self.offline();
                    }
                }
            } else {
                let n = safe_prefix(&self.pending, ack.as_bytes());
                self.pending.drain(..n);
            }
            return;
        }
        let j = Arc::make_mut(self.jobs.back_mut().unwrap());
        if !j.begun {
            let start = format!("\x1e{}B\x1f", j.id);
            if let Some(n) = pos(&self.pending, start.as_bytes()) {
                self.pending.drain(..n + start.len());
                j.begun = true;
            } else {
                let n = safe_prefix(&self.pending, start.as_bytes());
                self.pending.drain(..n);
                return;
            }
        }
        let end = format!("\x1e{}E", j.id);
        let mut search = 0;
        let (cut, code) = loop {
            let Some(n) = pos(&self.pending[search..], end.as_bytes()).map(|n| search + n) else {
                break (safe_prefix(&self.pending, end.as_bytes()), None);
            };
            let rest = &self.pending[n + end.len()..];
            let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
            if digits <= 10 && digits == rest.len() {
                break (n, None);
            }
            if digits > 0 && rest.get(digits) == Some(&31) {
                if let Ok(code) = std::str::from_utf8(&rest[..digits]).unwrap().parse::<i32>() {
                    break (n, Some(code));
                }
            }
            search = n + 1;
        };
        let bytes: Vec<u8> = self.pending.drain(..cut).collect();
        j.output.append(&bytes, code.is_some());
        if let Some(code) = code {
            j.status = "completed";
            j.code = Some(code);
            j.ended = Some(now());
            self.phase = "ready".into();
            self.pending.clear();
        }
        self.append(&bytes);
        if let Some(code) = code {
            self.append(format!("\r\n\x1b[90m[exit {code}]\x1b[0m\r\n").as_bytes());
        }
    }
    fn agent_ready(&mut self) {
        self.phase = "ready".into();
        self.manual = false;
        self.append(b"\r\n\x1b[36m--- Agent control ---\x1b[0m\r\n");
    }
    fn offline(&mut self) {
        if self.active().is_some() {
            let j = Arc::make_mut(self.jobs.back_mut().unwrap());
            j.output.append(&[], true);
            j.status = "disconnected";
            j.ended = Some(now());
        }
        self.screen = None;
        self.phase = "disconnected".into();
        self.sync = None;
        self.revision += 1;
    }
    pub fn disconnect(&mut self, from_agent: bool) -> Result<Value> {
        ensure!(!from_agent || !self.manual, "用户正在操作，请等待交还");
        let _ = self.tx.send(Wire::Close);
        self.offline();
        Ok(self.status())
    }
}

pub fn wait_job(s: &Shared, id: &str, offset: usize, wait: f64) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs_f64(wait.clamp(0., 25.));
    loop {
        {
            let state = s.lock().unwrap();
            if state.active().is_none_or(|j| j.id != id) || Instant::now() >= deadline {
                return state.poll(id, offset);
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
pub fn cancel(s: &Shared, id: &str, from_agent: bool) -> Result<Value> {
    {
        let mut g = s.lock().unwrap();
        ensure!(!from_agent || !g.manual, "用户正在操作，请等待交还");
        if g.active().is_none_or(|j| j.id != id) {
            return g.poll(id, 0);
        }
        if !g.jobs.back().unwrap().begun {
            Arc::make_mut(g.jobs.back_mut().unwrap()).begun = true;
            g.pending.clear();
        }
        g.upload.clear();
        g.send(vec![3])?;
    }
    std::thread::sleep(Duration::from_millis(250));
    {
        let g = s.lock().unwrap();
        if g.active().is_some_and(|j| j.id == id) {
            g.send(format!("\nbuiltin printf '\\036{id}E130\\037'\n").into_bytes())?;
        }
    }
    let result = wait_job(s, id, 0, 5.)?;
    if result["status"] == "running" {
        let mut g = s.lock().unwrap();
        if g.active().is_some_and(|j| j.id == id) {
            g.disconnect(false)?;
        }
        return g.poll(id, 0);
    }
    Ok(result)
}
pub fn takeover(s: &Shared, skin: crate::theme::Theme) -> Result<Value> {
    let active = {
        let mut g = s.lock().unwrap();
        ensure!(g.phase != "disconnected", "SSH 已断开，请先重连");
        if g.manual {
            return Ok(g.status());
        }
        ensure!(g.phase == "ready" || g.phase == "busy", "会话尚未就绪");
        g.set_skin(skin, false);
        g.manual = true;
        g.active().map(|j| j.id.clone())
    };
    if let Some(id) = active {
        cancel(s, &id, false)?;
    }
    let mut g = s.lock().unwrap();
    ensure!(g.phase == "ready" && g.manual, "无法恢复 shell");
    g.send(b"stty echo; export TERM=xterm-256color; PS1='\\[\\e[32m\\]\\u@\\h\\[\\e[0m\\]:\\w\\$ '; PS2='> '; set -o emacs; builtin printf '\\n'\n".to_vec())?;
    g.phase = "manual".into();
    g.append(b"\r\n\x1b[33m--- Manual control ---\x1b[0m\r\n");
    Ok(g.status())
}
pub fn release(s: &Shared) -> Result<Value> {
    {
        let mut g = s.lock().unwrap();
        if !g.manual {
            return Ok(g.status());
        }
        ensure!(g.phase == "manual", "终端正在同步");
        g.send(vec![3])?;
        g.begin_sync();
        g.phase = "synchronizing".into();
    }
    std::thread::sleep(Duration::from_millis(200));
    {
        let g = s.lock().unwrap();
        ensure!(g.phase == "synchronizing", "SSH 已断开");
        g.send_sync(false)?;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        {
            let mut g = s.lock().unwrap();
            ensure!(g.phase == "synchronizing", "SSH 已断开");
            if g.sync.is_none() {
                // Publish agent ownership and take the response snapshot atomically.
                // A concurrent execute can then start without delaying this handoff.
                g.agent_ready();
                return Ok(g.status());
            }
            if Instant::now() >= deadline {
                g.sync = None;
                let output = std::mem::take(&mut g.pending);
                let start = output.len().saturating_sub(HANDOFF_BUFFER);
                g.append(&output[start..]);
                g.phase = "manual".into();
                bail!("请退出当前全屏程序后再交还");
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

impl Hub {
    pub fn new(dir: PathBuf) -> Result<Self> {
        let mut sessions = IndexMap::new();
        let path = dir.join("sessions.json");
        if path.exists() {
            let entries: Vec<Entry> =
                serde_json::from_slice(&std::fs::read(path).context("无法读取 sessions.json")?)
                    .context("sessions.json 中的会话列表格式无效")?;
            for entry in entries {
                let (tx, _) = mpsc::unbounded_channel();
                sessions.insert(
                    entry.session_id.clone(),
                    Arc::new(Mutex::new(Session::new(entry, tx))),
                );
            }
        }
        Ok(Self {
            dir,
            rt: Runtime::new()?,
            sessions: Mutex::new(sessions),
        })
    }
    fn persist(&self, map: &IndexMap<String, Shared>, replacement: Option<&Entry>) -> Result<()> {
        crate::store::save(
            &self.dir.join("sessions.json"),
            &map.iter()
                .map(|(id, session)| match replacement {
                    Some(entry) if entry.session_id == *id => entry.clone(),
                    _ => session.lock().unwrap().entry.clone(),
                })
                .collect::<Vec<_>>(),
        )
    }
    pub fn list(&self) -> Vec<Value> {
        self.sessions
            .lock()
            .unwrap()
            .values()
            .map(|s| s.lock().unwrap().status())
            .collect()
    }
    pub fn get(&self, id: &str) -> Result<Shared> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .context("未知会话，请刷新标签")
    }
    pub fn connect(&self, p: Profile, origin: ConnectOrigin<'_>) -> Result<Value> {
        let mut map = self.sessions.lock().unwrap();
        let existing = map
            .values()
            .find(|s| {
                let g = s.lock().unwrap();
                let e = &g.entry;
                e.host
                    .trim_end_matches('.')
                    .eq_ignore_ascii_case(p.host.trim_end_matches('.'))
                    && e.port == p.port
                    && e.user == p.user
            })
            .cloned();
        let reused = existing
            .as_ref()
            .is_some_and(|s| s.lock().unwrap().phase != "disconnected");
        if let ConnectOrigin::Reconnect(original) = origin {
            ensure!(
                existing
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, original))
                    && !reused,
                "会话已关闭或已重连，请刷新后操作"
            );
        }
        let entry = Entry {
            session_id: existing
                .as_ref()
                .map(|s| s.lock().unwrap().entry.session_id.clone())
                .unwrap_or_else(|| target_id(&p.host, p.port, &p.user)),
            profile_id: (!p.id.is_empty()).then(|| p.id.clone()),
            name: p.name.clone(),
            host: p.host.clone(),
            port: p.port,
            user: p.user.clone(),
        };
        let s = if reused {
            let session = existing.unwrap();
            if matches!(origin, ConnectOrigin::Form) && session.lock().unwrap().entry != entry {
                // Publish the operator's selected reconnect profile only after it is durable.
                self.persist(&map, Some(&entry))?;
                session.lock().unwrap().entry = entry;
            }
            session
        } else {
            ensure!(p.id.is_empty() || !p.password.is_empty(), "请填写密码；此连接没有可用的已保存密码");
            let id = entry.session_id.clone();
            let (tx, rx) = mpsc::unbounded_channel();
            let mut g = Session::new(entry, tx);
            g.phase = "connecting".into();
            let s = Arc::new(Mutex::new(g));
            let previous = map.insert(id.clone(), s.clone());
            if let Err(e) = self.persist(&map, None) {
                map.shift_remove(&id);
                if let Some(old) = previous {
                    map.insert(id, old);
                }
                return Err(e);
            }
            if let Some(old) = previous {
                let mut old = old.lock().unwrap();
                let mut new = s.lock().unwrap();
                // A poll already in flight still holds the old session. All of its
                // jobs are final at disconnect, so both transports can share them.
                new.jobs = old.jobs.clone();
                new.terminal = std::mem::take(&mut old.terminal);
            }
            let file = self.dir.join("known_hosts");
            let task = s.clone();
            self.rt.spawn(async move {
                let r = run(p, file, task.clone(), rx).await;
                let mut g = task.lock().unwrap();
                g.error = r.err().map(|e| e.to_string());
                g.offline();
            });
            s
        };
        drop(map);
        let deadline = Instant::now() + Duration::from_secs(50);
        loop {
            {
                let g = s.lock().unwrap();
                if g.phase != "connecting" {
                    ensure!(
                        g.phase != "disconnected",
                        "{}",
                        g.error.as_deref().unwrap_or("SSH 已断开")
                    );
                    let mut v = g.status();
                    v["reused"] = json!(reused);
                    return Ok(v);
                }
            }
            ensure!(Instant::now() < deadline, "连接超时");
            std::thread::sleep(Duration::from_millis(40));
        }
    }
    pub fn close(&self, id: &str) -> Result<()> {
        let mut map = self.sessions.lock().unwrap();
        let s = map.shift_remove(id).context("未知会话")?;
        if let Err(e) = self.persist(&map, None) {
            map.insert(id.into(), s);
            return Err(e);
        }
        s.lock().unwrap().disconnect(false)?;
        Ok(())
    }
}
async fn run(
    p: Profile,
    file: PathBuf,
    s: Shared,
    mut rx: mpsc::UnboundedReceiver<Wire>,
) -> Result<()> {
    let setup = async {
        let config = client::Config {
            keepalive_interval: Some(Duration::from_secs(15)),
            keepalive_max: 3,
            ..Default::default()
        };
        let mut handle = client::connect(
            Arc::new(config),
            (p.host.as_str(), p.port),
            HostKey {
                host: p.host.clone(),
                port: p.port,
                file,
            },
        )
        .await?;
        let auth = handle.authenticate_password(&p.user, &p.password).await?;
        if let client::AuthResult::Failure {
            remaining_methods,
            partial_success,
        } = auth
        {
            use client::KeyboardInteractiveAuthResponse as Interactive;
            use russh::MethodKind;
            ensure!(
                !partial_success
                    && !remaining_methods.contains(&MethodKind::Password)
                    && remaining_methods.contains(&MethodKind::KeyboardInteractive),
                "SSH 登录失败"
            );
            let mut challenge = handle
                .authenticate_keyboard_interactive_start(&p.user, None)
                .await?;
            let mut sent = false;
            loop {
                match challenge {
                    Interactive::Success => break,
                    Interactive::InfoRequest { prompts, .. } if prompts.is_empty() => {
                        challenge = handle
                            .authenticate_keyboard_interactive_respond(vec![])
                            .await?;
                    }
                    Interactive::InfoRequest { prompts, .. }
                        if !sent && prompts.len() == 1 && !prompts[0].echo =>
                    {
                        sent = true;
                        challenge = handle
                            .authenticate_keyboard_interactive_respond(vec![p.password.clone()])
                            .await?;
                    }
                    _ => bail!("SSH 登录失败或需要额外的认证步骤"),
                }
            }
        }
        let mut ch = handle.channel_open_session().await?;
        ch.request_pty(true, "xterm-256color", 160, 40, 0, 0, &[])
            .await?;
        ch.request_shell(true).await?;
        let mut initial = Vec::new();
        loop {
            match ch.wait().await {
                Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                    initial.extend_from_slice(&data);
                    let cut = initial.len().saturating_sub(16000);
                    initial.drain(..cut);
                    let mut parser = vt100::Parser::new(40, 160, 0);
                    parser.process(&initial);
                    if parser
                        .screen()
                        .contents()
                        .lines()
                        .any(|line| line.trim_end().ends_with(['$', '#', '>']))
                    {
                        break;
                    }
                }
                Some(ChannelMsg::Failure | ChannelMsg::Close | ChannelMsg::Eof) | None => {
                    bail!("服务器未提供交互式 shell")
                }
                _ => {}
            }
        }
        anyhow::Ok((handle, ch, initial))
    };
    let (_handle, mut ch, initial) = tokio::select! {r=tokio::time::timeout(Duration::from_secs(25),setup)=>r??,_=rx.recv()=>return Ok(())};
    drop(p);
    {
        let mut g = s.lock().unwrap();
        ensure!(g.phase == "connecting", "连接已关闭");
        g.connection_id = Some(Uuid::new_v4().simple().to_string());
        g.append(b"\r\n\x1b[36m--- SSH connected ---\x1b[0m\r\n");
        g.append(&initial);
        g.begin_sync();
        g.send_sync(true)?;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        tokio::select! {
            msg=ch.wait()=>match msg{
                Some(ChannelMsg::Data{data}|ChannelMsg::ExtendedData{data,..})=>s.lock().unwrap().consume(&data),
                Some(ChannelMsg::Eof|ChannelMsg::Close)|None=>break,
                Some(ChannelMsg::Failure)=>bail!("服务器拒绝 PTY 或 shell 请求"),_=>{}
            },
            data=rx.recv()=>match data{
                Some(Wire::Data(bytes))=>ch.data(&bytes[..]).await?,
                Some(Wire::Resize(cols,rows))=>ch.window_change(cols.into(),rows.into(),0,0).await?,
                Some(Wire::Close)|None=>{ch.close().await?;break}
            },
            _=tokio::time::sleep_until(deadline),if s.lock().unwrap().phase=="connecting"=>bail!("Bash 初始化超时")
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session() -> (Session, mpsc::UnboundedReceiver<Wire>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut s = Session::new(
            Entry {
                session_id: "s".into(),
                profile_id: None,
                name: "demo".into(),
                host: "localhost".into(),
                port: 22,
                user: "test".into(),
            },
            tx,
        );
        s.phase = "ready".into();
        (s, rx)
    }
    #[test]
    fn release_publishes_ownership_only_after_acknowledgment() {
        let (mut state, mut rx) = session();
        state.manual = true;
        state.phase = "manual".into();
        let shared = Arc::new(Mutex::new(state));
        let control = shared.clone();
        let handing_off = std::thread::spawn(move || release(&control));
        assert!(matches!(rx.blocking_recv(), Some(Wire::Data(bytes)) if bytes == [3]));
        assert!(matches!(rx.blocking_recv(), Some(Wire::Data(_))));
        {
            let mut state = shared.lock().unwrap();
            let ack = format!("\x1e{}S\x1f", state.sync.as_ref().unwrap());
            state.consume(ack.as_bytes());
            assert!(state.execute("echo too-early").is_err());
        }
        let status = handing_off.join().unwrap().unwrap();
        assert_eq!(status["state"], "ready");
        assert_eq!(status["owner"], "agent");
        assert!(shared.lock().unwrap().execute("echo ready").is_ok());
    }

    #[test]
    fn live_terminal_queries_are_ordered_and_scoped_to_manual_ownership() {
        let (mut s, mut rx) = session();
        s.consume(b"\x1b[4;5H\x1b[6n");
        assert!(rx.try_recv().is_err());
        s.manual = true;
        s.phase = "manual".into();
        for piece in [
            b"\x1b[".as_slice(),
            b"6n\x1b[?6",
            b"n\x1b[5n\x1b[c\x1b[>c",
            b"\x1b[2;4f\x1b[6n\x1bc\x1b[6n",
        ] {
            s.consume(piece);
        }
        let mut bytes = Vec::new();
        while let Ok(Wire::Data(reply)) = rx.try_recv() {
            bytes.extend(reply);
        }
        assert_eq!(
            bytes,
            b"\x1b[4;5R\x1b[?4;5R\x1b[0n\x1b[?1;2c\x1b[>0;276;0c\x1b[2;4R\x1b[1;1R"
        );
        s.read(0);
        s.read(0);
        s.consume(b"\x1b[8n\x1b[2c\x1b[?15n");
        assert!(rx.try_recv().is_err());
        s.phase = "synchronizing".into();
        s.consume(b"\x1b[5n");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn parser_streams_immediately_and_retains_twenty_jobs() {
        let (mut s, _rx) = session();
        let mut ids = vec![];
        for _ in 0..21 {
            let id = s.execute("echo demo").unwrap();
            ids.push(id.clone());
            s.consume(format!("\x1e{id}A\x1f").as_bytes());
            for b in format!("echo hidden\x1e{id}B\x1f测🔴").bytes() {
                s.consume(&[b]);
            }
            assert_eq!(s.poll(&id, 0).unwrap()["output"], "测🔴");
            let fake = format!("\x1e{id}Enot-an-end");
            s.consume(fake.as_bytes());
            for b in format!("\x1e{id}E17\x1f").bytes() {
                s.consume(&[b]);
            }
            let r = s.poll(&id, 2).unwrap();
            assert_eq!(r["output"], fake);
            assert_eq!(r["exit_code"], 17);
        }
        assert!(s.poll(&ids[0], 0).is_err());
        assert_eq!(s.read(0)["jobs"].as_array().unwrap().len(), 20);
        assert_eq!(s.poll(&ids[1], 0).unwrap()["exit_code"], 17);
        assert!(!s.read(0)["data"].as_str().unwrap().contains("echo hidden"));
    }
}
