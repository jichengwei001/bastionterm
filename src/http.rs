use crate::{service::Service, session::Hub, store};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const MAX_BODY: usize = 1_000_000;

fn headers(stream: &mut TcpStream, status: u16, mime: &str, size: Option<usize>) -> Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status} {}\r\nContent-Type: {mime}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nX-Frame-Options: DENY\r\nContent-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; object-src 'none'; base-uri 'none'\r\n",
        if status == 200 { "OK" } else { "Error" }
    )?;
    if let Some(n) = size {
        write!(stream, "Content-Length: {n}\r\n")?;
    }
    write!(stream, "\r\n")?;
    Ok(())
}
fn respond(stream: &mut TcpStream, status: u16, mime: &str, body: &[u8]) -> Result<()> {
    headers(stream, status, mime, Some(body.len()))?;
    stream.write_all(body)?;
    Ok(())
}
fn json_response(stream: &mut TcpStream, status: u16, value: Value) -> Result<()> {
    respond(
        stream,
        status,
        "application/json",
        &serde_json::to_vec(&value)?,
    )
}
fn forbidden(reader: &mut BufReader<TcpStream>, reason: &str, length: Option<usize>) -> Result<()> {
    json_response(reader.get_mut(), 403, json!({"error":reason}))?;
    // Closing a Windows socket with unread request bytes can reset the
    // connection before the client receives the response.
    reader.get_ref().shutdown(Shutdown::Write)?;
    let Some(mut remaining @ 1..=MAX_BODY) = length else { return Ok(()) };
    let deadline = Instant::now() + Duration::from_millis(250);
    while remaining > 0 {
        let timeout = deadline.saturating_duration_since(Instant::now());
        if timeout.is_zero() {
            break;
        }
        reader.get_ref().set_read_timeout(Some(timeout))?;
        let available = remaining.min(reader.fill_buf()?.len());
        if available == 0 {
            break;
        }
        reader.consume(available);
        remaining -= available;
    }
    Ok(())
}

pub fn handle(stream: TcpStream, service: &Service, token: &str, port: u16) -> Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        let n = (&mut reader)
            .take((16384 - buf.len()) as u64)
            .read_until(b'\n', &mut buf)?;
        ensure!(n > 0 && buf.len() < 16384, "请求头过长");
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let mut slots = [httparse::EMPTY_HEADER; 64];
    let mut req = httparse::Request::new(&mut slots);
    ensure!(req.parse(&buf)?.is_complete(), "请求不完整");
    let header = |name: &str| {
        req.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .and_then(|h| std::str::from_utf8(h.value).ok())
            .unwrap_or("")
    };
    let host = header("host");
    let post = req.method == Some("POST");
    let rejection = if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        Some("Invalid Host")
    } else if post
        && (header("Origin") != format!("http://{host}") || header("X-Terminal-Token") != token)
    {
        Some("Invalid origin or token")
    } else {
        None
    };
    if let Some(reason) = rejection {
        return forbidden(&mut reader, reason, header("Content-Length").parse().ok());
    }
    let path = req.path.context("请求缺少路径")?;
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    if post {
        let result = (|| {
            let n = header("Content-Length").parse::<usize>()?;
            ensure!((1..=MAX_BODY).contains(&n), "请求大小无效");
            let mut data = vec![0; n];
            reader.read_exact(&mut data)?;
            service.action(
                path.strip_prefix("/api/").context("未知地址")?,
                &serde_json::from_slice(&data)?,
            )
        })();
        return match result {
            Ok(v) => json_response(reader.get_mut(), 200, v),
            Err(e) => json_response(reader.get_mut(), 400, json!({"error":e.to_string()})),
        };
    }
    let stream = reader.get_mut();
    if req.method != Some("GET") {
        return json_response(stream, 405, json!({"error":"Method not allowed"}));
    }
    macro_rules! asset {
        ($file:literal,$mime:literal) => {
            respond(
                stream,
                200,
                $mime,
                include_bytes!(concat!("../assets/browser/", $file)),
            )
        };
    }
    match path {
        "/health" => json_response(
            stream,
            200,
            json!({"service":"bastion-bridge-gpui","pid":std::process::id(),"workspace":service.hub.dir}),
        ),
        "/" => respond(
            stream,
            200,
            "text/html; charset=utf-8",
            include_str!("../assets/browser/index.html")
                .replace("__SESSION_TOKEN__", token)
                .replace("__DESKTOP_MODE__", "true")
                .as_bytes(),
        ),
        "/style.css" => asset!("style.css", "text/css"),
        "/skins.js" => asset!("skins.js", "application/javascript"),
        "/app.js" => asset!("app.js", "application/javascript"),
        "/vendor/xterm.js" => asset!("xterm.js", "application/javascript"),
        "/vendor/fit.js" => asset!("addon-fit.js", "application/javascript"),
        "/vendor/xterm.css" => asset!("xterm.css", "text/css"),
        "/fonts/JetBrainsMono-Regular.woff2" => asset!("JetBrainsMono-Regular.woff2", "font/woff2"),
        "/fonts/JetBrainsMono-Bold.woff2" => asset!("JetBrainsMono-Bold.woff2", "font/woff2"),
        "/api/events" => {
            let query = url::form_urlencoded::parse(query.as_bytes())
                .collect::<std::collections::HashMap<_, _>>();
            if query.get("token").is_none_or(|v| v != token) {
                return json_response(stream, 403, json!({"error":"Invalid token"}));
            }
            let mut args = json!({"session_id":query.get("session_id")});
            let mut last = Value::Null;
            let mut beat = Instant::now();
            headers(stream, 200, "text/event-stream", None)?;
            while !service.stop.load(Ordering::Acquire) {
                let mut v = service.action("snapshot", &args)?;
                args["offset"] = v["offset"].clone();
                args["connection_id"] = v["connection_id"].clone();
                let data = v.as_object_mut().unwrap().remove("data").unwrap();
                if last != v || data != "" || beat.elapsed() > Duration::from_secs(5) {
                    last = v.clone();
                    v["data"] = data;
                    write!(stream, "data: {v}\n\n")?;
                    stream.flush()?;
                    beat = Instant::now();
                }
                std::thread::sleep(Duration::from_millis(80));
            }
            Ok(())
        }
        _ => json_response(stream, 404, json!({"error":"Not found"})),
    }
}

pub fn serve(dir: &Path) -> Result<()> {
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("service.lock"))?;
    if let Err(e) = fs2::FileExt::try_lock_exclusive(&lock) {
        if e.kind() == std::io::ErrorKind::WouldBlock
            || e.raw_os_error() == fs2::lock_contended_error().raw_os_error()
        {
            return Ok(());
        }
        return Err(e.into());
    }
    let listener = (8767..8800)
        .find_map(|p| TcpListener::bind(("127.0.0.1", p)).ok())
        .context("8767–8799 端口已占用")?;
    let port = listener.local_addr()?.port();
    let token = uuid::Uuid::new_v4().simple().to_string();
    let service = Arc::new(Service {
        hub: Hub::new(dir.into())?,
        url: format!("http://127.0.0.1:{port}"),
        stop: AtomicBool::new(false),
    });
    store::save(
        &dir.join("service.json"),
        &json!({"token":token,"port":port,"pid":std::process::id()}),
    )?;
    listener.set_nonblocking(true)?;
    while !service.stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                let service = service.clone();
                let token = token.clone();
                std::thread::spawn(move || {
                    let _ = handle(stream, &service, &token, port);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(8))
            }
            Err(e) => bail!(e),
        }
    }
    for s in service.hub.sessions.lock().unwrap().values() {
        s.lock().unwrap().disconnect(false)?;
    }
    std::fs::remove_file(dir.join("service.json"))?;
    Ok(())
}
