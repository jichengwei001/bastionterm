use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
#[derive(Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    #[serde(default, skip_serializing)]
    pub password: String,
}
pub fn directory() -> Result<PathBuf> {
    let dir = std::env::var_os("BASTION_GPUI_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::data_local_dir().map(|d| d.join("BastionBridgeGPUI")))
        .context("找不到用户目录")?;
    fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}
pub fn save(path: &Path, data: &impl Serialize) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().context("配置目录无效")?)?;
    serde_json::to_writer_pretty(&mut file, data)?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}
use fs2::FileExt;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Default)]
struct Profiles {
    profiles: Vec<Profile>,
    default_id: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct Secret {
    host: String,
    port: u16,
    user: String,
    password: String,
}
struct Store {
    profiles: Profiles,
    secrets: BTreeMap<String, Secret>,
}
impl Store {
    fn listing(&self) -> Value {
        json!({
            "default_id": self.profiles.default_id,
            "profiles": self.profiles.profiles.iter().map(|p| self.public(p)).collect::<Vec<_>>()
        })
    }
    fn secret(&self, p: &Profile) -> Option<&str> {
        self.secrets
            .get(&p.id)
            .filter(|s| s.host == p.host && s.port == p.port && s.user == p.user)
            .map(|s| s.password.as_str())
            .filter(|s| !s.is_empty())
    }
    fn public(&self, p: &Profile) -> Value {
        let mut v = serde_json::to_value(p).unwrap();
        v["has_password"] = json!(self.secret(p).is_some());
        v
    }
    fn commit(&self, dir: &Path) -> Result<()> {
        save(&dir.join("credentials.json"), &self.secrets)?;
        save(&dir.join("profiles.json"), &self.profiles)
    }
}
fn locked<T>(dir: &Path, f: impl FnOnce(&mut Store) -> Result<T>) -> Result<T> {
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join("profiles.lock"))?;
    FileExt::lock_exclusive(&lock)?;
    let path = dir.join("profiles.json");
    let credentials = dir.join("credentials.json");
    let mut store = Store {
        profiles: Profiles::default(),
        secrets: if credentials.exists() {
            serde_json::from_slice(&fs::read(credentials)?)?
        } else {
            BTreeMap::new()
        },
    };
    if path.exists() {
        let value: Value = serde_json::from_slice(&fs::read(path)?)?;
        if value.is_array() {
            // Upgrade earlier GPUI installations under the same file lock.
            for mut p in serde_json::from_value::<Vec<Profile>>(value)? {
                if !p.password.is_empty() {
                    store.secrets.insert(
                        p.id.clone(),
                        Secret {
                            host: p.host.clone(),
                            port: p.port,
                            user: p.user.clone(),
                            password: std::mem::take(&mut p.password),
                        },
                    );
                }
                if store.profiles.default_id.is_none() {
                    store.profiles.default_id = Some(p.id.clone());
                }
                store.profiles.profiles.push(p);
            }
            store.commit(dir)?;
        } else {
            store.profiles = serde_json::from_value(value)?;
        }
    }
    f(&mut store)
}
pub fn list(dir: &Path) -> Result<Value> {
    locked(dir, |s| Ok(s.listing()))
}
pub fn find_profile(dir: &Path, id: &str, password: &str) -> Result<Option<Profile>> {
    locked(dir, |s| {
        let Some(mut p) = s.profiles.profiles.iter().find(|p| p.id == id).cloned() else {
            return Ok(None);
        };
        p.password = if password.is_empty() {
            s.secret(&p).unwrap_or("").into()
        } else {
            password.into()
        };
        Ok(Some(p))
    })
}
pub fn parse(v: &Value) -> Result<Profile> {
    let text = |k: &str| v[k].as_str().unwrap_or("").trim().to_string();
    let host = text("host");
    let user = text("user");
    let mut name = text("name");
    let port = match &v["port"] {
        Value::Null => 22,
        Value::String(s) => {
            let port = s.trim().parse::<i64>().context("请输入 1 到 65535 之间的整数端口")?;
            u16::try_from(port).context("端口必须在 1–65535 之间")?
        }
        n => u16::try_from(n.as_u64().context("无效端口")?).context("端口必须在 1–65535 之间")?,
    };
    let valid = |s: &str, max| {
        !s.is_empty()
            && s.len() <= max
            && !s.starts_with('-')
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    };
    ensure!(valid(&host, 253), "请输入有效的主机域名或 IPv4 地址");
    ensure!(valid(&user, 128), "请输入有效的 SSH 账号");
    ensure!(port > 0, "端口必须在 1–65535 之间");
    if name.is_empty() {
        name = format!("{user}@{host}");
    }
    ensure!(name.chars().count() <= 120, "连接名称不能超过 120 个字符");
    let password = match v.get("password") {
        Some(value) => value.as_str().context("密码格式无效或长度超过限制")?,
        None => "",
    };
    ensure!(password.len() <= 4096, "密码格式无效或长度超过限制");
    Ok(Profile {
        id: text("id"),
        name,
        host,
        port,
        user,
        password: password.to_owned(),
    })
}
pub fn upsert(dir: &Path, v: &Value) -> Result<Value> {
    let mut p = parse(v)?;
    locked(dir, |s| {
        let remember = v["remember_password"] == true;
        if !p.id.is_empty() {
            let old = s
                .profiles
                .profiles
                .iter()
                .find(|old| old.id == p.id)
                .context("该连接配置已被删除，请新建")?;
            ensure!(
                !remember
                    || !p.password.is_empty()
                    || s.secret(old).is_none()
                    || s.secret(&p).is_some(),
                "主机、端口或账号已改变，请重新输入密码，或取消保存密码"
            );
        } else {
            p.id = uuid::Uuid::new_v4().simple().to_string();
        }
        if !remember {
            s.secrets.remove(&p.id);
        } else if !p.password.is_empty() {
            s.secrets.insert(
                p.id.clone(),
                Secret {
                    host: p.host.clone(),
                    port: p.port,
                    user: p.user.clone(),
                    password: std::mem::take(&mut p.password),
                },
            );
        }
        s.profiles.profiles.retain(|old| old.id != p.id);
        s.profiles.default_id = Some(p.id.clone());
        let public = s.public(&p);
        s.profiles.profiles.push(p);
        s.commit(dir)?;
        Ok(public)
    })
}
pub fn delete(dir: &Path, id: &str) -> Result<Value> {
    locked(dir, |s| {
        ensure!(
            s.profiles.profiles.iter().any(|p| p.id == id),
            "连接配置不存在"
        );
        s.profiles.profiles.retain(|p| p.id != id);
        s.secrets.remove(id);
        if s.profiles.default_id.as_deref() == Some(id) {
            s.profiles.default_id = s.profiles.profiles.first().map(|p| p.id.clone());
        }
        s.commit(dir)?;
        let mut result = s.listing();
        result["deleted"] = json!(true);
        Ok(result)
    })
}
