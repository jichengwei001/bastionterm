#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
mod config_text;
mod cursor_motion;
mod display;
mod field;
mod field_history;
mod form_validation;
mod field_words;
mod keyboard;
mod history_text;
#[cfg(target_os = "macos")]
mod mac_window;
mod menu;
mod scrollbar;
mod selection;
mod startup;
mod terminal_ime;
use bastion_bridge_gpui::theme;
mod view;
use anyhow::Result;
use bastion_bridge_gpui::{
    http,
    mcp::{self, Endpoint},
    store,
};
use gpui::*;
use serde_json::json;
use std::{path::PathBuf, sync::Arc};

actions!(bridge, [ShowTerminal, QuitBridge]);

fn show(cx: &mut App) {
    cx.activate(true);
    for w in cx.windows() {
        let _ = w.update(cx, |_, w, _| w.activate_window());
    }
}
fn main() -> Result<()> {
    let dir = store::directory()?;
    match std::env::args().nth(1).as_deref() {
        Some("--mcp") => return mcp::stdio(&dir),
        Some("--serve") => {
            if let Err(error) = http::serve(&dir) {
                use std::io::Write;
                let mut log = std::fs::OpenOptions::new();
                log.create(true).append(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    log.mode(0o600);
                }
                if let Ok(mut file) = log.open(dir.join("service.log")) {
                    let _ = writeln!(file, "{} {error}", chrono::Local::now().to_rfc3339());
                }
                return Err(error);
            }
            return Ok(());
        }
        Some("--ensure") => {
            println!(
                "{}",
                json!({"url":Endpoint::ensure(&dir)?.url(),"data_dir":dir})
            );
            return Ok(());
        }
        Some("--stop") => {
            Endpoint::stop(&dir)?;
            println!("{}", json!({"stopped":true}));
            return Ok(());
        }
        Some(other) => anyhow::bail!("未知启动参数 {other}"),
        None => {}
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .read(true)
        .open(dir.join("native-window.lock"))?;
    if let Err(e) = fs2::FileExt::try_lock_exclusive(&lock) {
        if e.raw_os_error() == fs2::lock_contended_error().raw_os_error() {
            std::fs::write(dir.join("show-window"), [])?;
            return Ok(());
        }
        return Err(e.into());
    }
    let app = Application::new();
    app.on_reopen(show);
    app.run(move |cx| {
        field::bind(cx);
        #[cfg(windows)]
        cx.text_system()
            .add_fonts(vec![
                std::borrow::Cow::Borrowed(include_bytes!("../assets/JetBrainsMono-Regular.ttf")),
                std::borrow::Cow::Borrowed(include_bytes!("../assets/JetBrainsMono-Bold.ttf")),
            ])
            .expect("JetBrains Mono");
        #[cfg(not(any(windows, target_os = "macos")))]
        cx.set_menus(vec![Menu {
            name: "垒桥终端".into(),
            items: vec![
                MenuItem::action("显示终端", ShowTerminal),
                MenuItem::separator(),
                MenuItem::action("退出垒桥终端与后台", QuitBridge),
            ],
        }]);
        cx.on_action(|_: &ShowTerminal, cx| show(cx));
        cx.on_action(|_: &QuitBridge, cx| cx.quit());
        cx.bind_keys([KeyBinding::new(
            if cfg!(windows) {
                "ctrl-shift-q"
            } else {
                "cmd-q"
            },
            QuitBridge,
            None,
        )]);
        let launch = Arc::new(startup::Launch::default());
        let shutdown = launch.clone();
        let data = dir.clone();
        cx.on_app_quit(move |_| {
            shutdown.stop(&data);
            async {}
        })
        .detach();
        startup::open(dir, launch, cx).expect("无法打开启动窗口");
        cx.activate(true);
    });
    Ok(())
}

fn open_terminal(dir: PathBuf, endpoint: Endpoint, minimized: bool, cx: &mut App) -> Result<()> {
    let bounds = Bounds::centered(None, size(px(1280.), px(860.)), cx);
    let terminal = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            focus: !minimized,
            window_min_size: Some(size(px(640.), px(480.))),
            titlebar: Some(TitlebarOptions {
                title: Some("垒桥终端 · BastionTerm".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        |w, cx| cx.new(|cx| view::View::new(dir, endpoint, w, cx)),
    )?;
    #[cfg(target_os = "macos")]
    menu::install(terminal, cx)?;
    #[cfg(target_os = "macos")]
    terminal.update(cx, |v, w, _| mac_window::prepare(w, v.theme()))??;

    if minimized {
        terminal.update(cx, |_, w, _| w.minimize_window())?;
    }
    Ok(())
}
