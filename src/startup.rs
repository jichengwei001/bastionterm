use crate::{mcp::Endpoint, open_terminal};
use anyhow::Result;
use gpui::{prelude::*, *};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Default)]
pub struct Launch {
    // Quit can arrive between spawning the service and receiving its ready response.
    attached: Mutex<bool>,
    quitting: AtomicBool,
}

impl Launch {
    fn connect(&self, dir: &Path) -> Result<Endpoint> {
        let mut attached = self.attached.lock().unwrap();
        let endpoint = Endpoint::ensure_until_quit(dir, || self.quitting.load(Ordering::Acquire))?;
        *attached = true;
        Ok(endpoint)
    }

    pub fn stop(&self, dir: &Path) {
        self.quitting.store(true, Ordering::Release);
        if *self.attached.lock().unwrap() {
            let _ = Endpoint::stop(dir);
        }
    }
}

struct Startup {
    dir: PathBuf,
    launch: Arc<Launch>,
    error: Option<String>,
    icon: Arc<Image>,
    retry: FocusHandle,
}

pub fn open(dir: PathBuf, launch: Arc<Launch>, cx: &mut App) -> Result<()> {
    let bounds = Bounds::centered(None, size(px(600.), px(360.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("垒桥终端".into()),
                ..Default::default()
            }),
            is_resizable: false,
            ..Default::default()
        },
        |w, cx| {
            cx.new(|cx: &mut Context<Startup>| {
                let mut view = Startup {
                    dir,
                    launch,
                    error: None,
                    retry: cx.focus_handle().tab_index(0).tab_stop(true),
                    icon: Arc::new(Image::from_bytes(
                        ImageFormat::Png,
                        include_bytes!("../assets/startup-icon.png").to_vec(),
                    )),
                };
                w.on_window_should_close(cx, |w, _| {
                    w.minimize_window();
                    false
                });
                cx.spawn_in(w, async move |this, cx| {
                    loop {
                        Timer::after(Duration::from_millis(150)).await;
                        if this
                            .update(cx, |v, cx| {
                                if std::fs::remove_file(v.dir.join("show-window")).is_ok() {
                                    crate::show(cx);
                                }
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .detach();
                view.start(w, cx);
                view
            })
        },
    )?;
    Ok(())
}

impl Startup {
    fn start(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.error = None;
        cx.notify();
        let dir = self.dir.clone();
        let launch = self.launch.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { launch.connect(&dir) })
                .await;
            let _ = this.update_in(cx, |v, w, cx| {
                match result
                    .and_then(|endpoint| open_terminal(v.dir.clone(), endpoint, minimized(w), cx))
                {
                    Ok(()) => w.remove_window(),
                    Err(error) => {
                        v.error = Some(format!("{error:#}"));
                        w.focus(&v.retry);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
}

fn minimized(window: &Window) -> bool {
    #[cfg(target_os = "macos")]
    return crate::mac_window::minimized(window);
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = HasWindowHandle::window_handle(window)
            && let RawWindowHandle::Win32(handle) = handle.as_raw()
        {
            return unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::IsIconic(handle.hwnd.get() as _) != 0
            };
        }
        false
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = window;
        false
    }
}

impl Render for Startup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .bg(rgb(0x101311))
            .text_color(rgb(0xe1e7e0))
            .child(img(self.icon.clone()).size(px(64.)))
            .child(
                div()
                    .mt(px(18.))
                    .text_size(px(25.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("垒桥终端"),
            )
            .child(
                div()
                    .id("startup-status")
                    .mt(px(18.))
                    .max_w(px(500.))
                    .max_h(px(110.))
                    .overflow_y_scroll()
                    .text_size(px(13.))
                    .text_color(rgb(0xa4b1a6))
                    .child(
                        self.error
                            .clone()
                            .unwrap_or_else(|| "正在连接本地后台…".into()),
                    ),
            )
            .when(self.error.is_some(), |root| {
                root.child(
                    div()
                        .id("retry-service")
                        .track_focus(&self.retry)
                        .mt(px(12.))
                        .px(px(18.))
                        .py(px(8.))
                        .rounded(px(5.))
                        .text_size(px(13.))
                        .bg(rgb(0xb5e68b))
                        .text_color(rgb(0x152010))
                        .cursor_pointer()
                        .child("重新尝试")
                        .on_click(cx.listener(|v, _, w, cx| {
                            if v.error.is_some() {
                                v.start(w, cx);
                            }
                        })),
                )
            })
            .child(
                div()
                    .mt(px(24.))
                    .text_size(px(11.))
                    .text_color(rgb(0x68776b))
                    .child("让 Agent 连通堡垒机后的主机"),
            )
            .when(cfg!(windows) && self.error.is_some(), |root| {
                root.child(
                    div()
                        .absolute()
                        .top(px(6.))
                        .right(px(12.))
                        .id("quit-startup")
                        .tab_index(0)
                        .cursor_pointer()
                        .text_size(px(12.))
                        .child("退出")
                        .on_click(|_, _, cx| cx.quit()),
                )
            })
    }
}
