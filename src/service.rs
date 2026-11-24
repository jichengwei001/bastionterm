use crate::{
    session::{self, Hub},
    store,
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};

pub struct Service {
    pub hub: Hub,
    pub url: String,
    pub stop: AtomicBool,
}

pub fn specs() -> &'static Value {
    static SCHEMA: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    SCHEMA.get_or_init(|| serde_json::from_str(include_str!("tools.json")).unwrap())
}
pub(crate) fn tool_schema(name: &str) -> Result<&'static Value> {
    specs()["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == name)
        .map(|spec| &spec["inputSchema"])
        .context("未知 MCP 工具")
}
pub(crate) fn validate(schema: &Value, args: &Value) -> Result<()> {
    let obj = args.as_object().context("无效的工具参数")?;
    for required in schema["required"].as_array().unwrap() {
        ensure!(
            obj.contains_key(required.as_str().unwrap()),
            "缺少参数 {required}"
        );
    }
    for (k, v) in obj {
        let field = &schema["properties"][k];
        let valid = match field["type"].as_str() {
            Some("string") => v.as_str().is_some_and(|s| {
                field["minLength"]
                    .as_u64()
                    .is_none_or(|min| s.chars().count() as u64 >= min)
            }),
            Some("integer") => v.is_i64() || v.is_u64(),
            Some("number") => v.as_f64().is_some_and(|n| (0.0..=25.).contains(&n)),
            _ => false,
        };
        ensure!(valid, "无效的工具参数 {k}");
    }
    Ok(())
}

impl Service {
    pub fn tool(&self, name: &str, a: &Value) -> Result<Value> {
        validate(tool_schema(name)?, a)?;
        let id = a["id"].as_str().unwrap_or_default();
        match name {
            "ssh_start_web" => Ok(json!({"url":self.url,"pid":std::process::id(),"started":false})),
            "ssh_list_profiles" => store::list(&self.hub.dir),
            "ssh_list_sessions" => Ok(json!({"sessions":self.hub.list()})),
            "ssh_connect" => {
                let id = a["profile_id"].as_str().context("缺少配置 ID")?;
                let p = store::find_profile(&self.hub.dir, id, "")?.with_context(|| {
                    format!(
                        "Unknown profile_id. Use ssh_list_profiles or configure a host at {}",
                        self.url
                    )
                })?;
                let mut r = self.hub.connect(p, session::ConnectOrigin::Agent)?;
                r["web_url"] = json!(format!(
                    "{}/?session_id={}",
                    self.url,
                    r["session_id"].as_str().unwrap()
                ));
                r["next_action"] = json!(if r["owner"] == "human" {
                    "wait_for_user"
                } else if !r["active_id"].is_null() {
                    "poll_active_command"
                } else {
                    "execute"
                });
                Ok(r)
            }
            name => {
                let session_id = a["session_id"].as_str().context("请指定会话")?;
                let s = self.hub.get(session_id).context("Unknown session_id. Use ssh_list_sessions.")?;
                match name {
                    "ssh_status" => Ok(s.lock().unwrap().status()),
                    "ssh_disconnect" => s.lock().unwrap().disconnect(true),
                    "ssh_cancel" => session::cancel(&s, id, true),
                    "ssh_execute" => {
                        let id = s.lock().unwrap().execute(a["command"].as_str().unwrap())?;
                        session::wait_job(&s, &id, 0, a["wait"].as_f64().unwrap_or(10.))
                    }
                    "ssh_poll" => {
                        let offset = &a["offset"];
                        let mut result = session::wait_job(
                            &s,
                            id,
                            offset.as_u64().unwrap_or(0) as usize,
                            a["wait"].as_f64().unwrap_or(0.),
                        )?;
                        if offset.as_i64().is_some_and(|n| n < 0) {
                            result["truncated"] = json!(true);
                        }
                        Ok(result)
                    }
                    _ => bail!("未知工具"),
                }
            }
        }
    }
    pub fn action(&self, name: &str, a: &Value) -> Result<Value> {
        let dir = &self.hub.dir;
        let id = a["session_id"].as_str().unwrap_or("");
        match name {
            "desktop-info" => {
                let exe = std::env::current_exe()?;
                Ok(
                    json!({"version":env!("CARGO_PKG_VERSION"),"web_url":self.url,"data_dir":dir,"mcp_config":{"mcpServers":{"baoleiji-pty":{"command":exe,"args":["--mcp"]}}},"pi_config":{"mcpServers":{"baoleiji-pty":{"command":exe,"args":["--mcp"],"exposure":"direct"}}}}),
                )
            }
            "shutdown" => {
                self.stop.store(true, Ordering::Release);
                Ok(json!({"stopping":true}))
            }
            "profiles" => store::list(dir),
            "profile-save" => store::upsert(dir, a),
            "profile-delete" => store::delete(dir, a["id"].as_str().context("缺少配置 ID")?),
            "connect" => {
                let p = if let Some(id) = a["profile_id"].as_str().filter(|s| !s.is_empty()) {
                    let p = store::find_profile(dir, id, a["password"].as_str().unwrap_or(""))?
                        .context("连接配置不存在")?;
                    ensure!(!p.password.is_empty(), "请填写密码；此连接没有可用的已保存密码");
                    p
                } else {
                    let mut p = store::parse(a)?;
                    p.id.clear();
                    p
                };
                self.hub.connect(p, session::ConnectOrigin::Form)
            }
            "reconnect" => {
                let s = self.hub.get(id)?;
                let status = s.lock().unwrap().status();
                ensure!(status["state"] == "disconnected", "仅断开的会话可重连");
                let mut profile_id = Value::Null;
                if let Some(pid) = status["profile_id"].as_str() {
                    if let Ok(Some(profile)) = store::find_profile(dir, pid, "") {
                        let t = &status["target"];
                        if profile
                            .host
                            .trim_end_matches('.')
                            .eq_ignore_ascii_case(t["host"].as_str().unwrap().trim_end_matches('.'))
                            && profile.port as u64 == t["port"]
                            && profile.user == t["user"]
                        {
                            if !profile.password.is_empty() {
                                return self.hub.connect(profile, session::ConnectOrigin::Reconnect(&s));
                            }
                            profile_id = json!(pid);
                        }
                    }
                }
                Ok(
                    json!({"needs_credentials":true,"target":status["target"],"session_id":id,"name":status["name"],"profile_id":profile_id}),
                )
            }
            "snapshot" => {
                let mut v = match self.hub.get(id) {
                    Ok(s) => {
                        let g = s.lock().unwrap();
                        let offset = a["offset"].as_u64().unwrap_or(0);
                        g.read(offset as usize)
                    }
                    Err(_) => {
                        json!({"state":"disconnected","owner":"agent","data":"","offset":0,"jobs":[]})
                    }
                };
                v["sessions"] = json!(self.hub.list());
                Ok(v)
            }
            "close-session" => {
                self.hub.close(id)?;
                Ok(json!({"closed":true,"session_id":id}))
            }
            "mcp-call" => self.tool(a["name"].as_str().context("缺少工具名")?, &a["arguments"]),
            _ => {
                let s = self.hub.get(id)?;
                match name {
                    "takeover" => {
                        let theme = match a["skin"].as_str() {
                            Some(id) => crate::theme::Theme::named(id).context("未知皮肤")?,
                            None => crate::theme::Theme::saved(dir),
                        };
                        session::takeover(&s, theme)
                    }
                    "terminal-theme" => {
                        let skin = a["skin"]
                            .as_str()
                            .and_then(crate::theme::Theme::named)
                            .context("未知皮肤")?;
                        s.lock().unwrap().set_skin(skin, true);
                        Ok(json!({"ok":true}))
                    }
                    "release" => session::release(&s),
                    "input-bytes" => {
                        use base64::Engine;
                        let text = a["base64"].as_str().context("缺少输入")?;
                        ensure!(text.len() <= 21848, "输入过长");
                        s.lock()
                            .unwrap()
                            .input(base64::engine::general_purpose::STANDARD.decode(text)?)?;
                        Ok(json!({"accepted":true}))
                    }
                    "input" => {
                        s.lock()
                            .unwrap()
                            .input(a["data"].as_str().context("缺少输入")?.as_bytes().into())?;
                        Ok(json!({"accepted":true}))
                    }
                    "resize" => {
                        let size = |k: &str| {
                            a[k].as_u64()
                                .map(|v| v.min(65535) as u16)
                                .context("尺寸无效")
                        };
                        s.lock().unwrap().resize(size("cols")?, size("rows")?)?;
                        Ok(json!({"ok":true}))
                    }
                    _ => bail!("未知操作"),
                }
            }
        }
    }
}
