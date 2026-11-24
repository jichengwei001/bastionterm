#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = bridge, no_json)]
pub(crate) struct OpenClipboardMenu {
    pub position: gpui::Point<gpui::Pixels>,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum EditCommand {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

impl EditCommand {
    pub(crate) fn action(self) -> Box<dyn gpui::Action> {
        use crate::field;
        match self {
            Self::Undo => Box::new(field::Undo),
            Self::Redo => Box::new(field::Redo),
            Self::Cut => Box::new(field::Cut),
            Self::Copy => Box::new(field::Copy),
            Self::Paste => Box::new(field::Paste),
            Self::SelectAll => Box::new(field::SelectAll),
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) use native::{NativeMenu, install};

#[cfg(target_os = "macos")]
mod native {
    use super::EditCommand;
    use crate::view::View;
    use anyhow::Result;
    use gpui::{App, Global, WindowHandle};
    use muda::{
        AboutMetadata, Menu, MenuEvent, MenuItem, PredefinedMenuItem as SystemItem, Submenu,
    };

    pub(crate) struct NativeMenu {
        _root: Menu,
        edits: Vec<(EditCommand, MenuItem)>,
    }
    impl Global for NativeMenu {}

    impl NativeMenu {
        pub fn sync(&self, available: impl Fn(EditCommand) -> bool) {
            for (command, item) in &self.edits {
                let enabled = available(*command);
                if item.is_enabled() != enabled {
                    item.set_enabled(enabled);
                }
            }
        }
    }

    pub(crate) fn install(window: WindowHandle<View>, cx: &mut App) -> Result<()> {
        let root = Menu::new();
        let separator = SystemItem::separator();
        let about = SystemItem::about(
            None,
            Some(AboutMetadata {
                name: Some("BastionTerm".into()),
                version: Some(env!("CARGO_PKG_VERSION").into()),
                ..Default::default()
            }),
        );
        let quit = MenuItem::with_id(
            "quit",
            "Quit BastionTerm",
            true,
            Some("Cmd+Q".parse()?),
        );
        root.append(&Submenu::with_items(
            "BastionTerm",
            true,
            &[
                &about,
                &separator,
                &SystemItem::services(None),
                &separator,
                &SystemItem::hide(None),
                &SystemItem::hide_others(None),
                &separator,
                &quit,
            ],
        )?)?;
        let file = Submenu::new("File", true);
        file.append(&SystemItem::close_window(None))?;
        root.append(&file)?;

        let edit = Submenu::new("Edit", true);
        let mut edits = Vec::new();
        for (command, title, shortcut) in [
            (EditCommand::Undo, "Undo", "CmdOrCtrl+Z"),
            (EditCommand::Redo, "Redo", "Cmd+Shift+Z"),
            (EditCommand::Cut, "Cut", "CmdOrCtrl+X"),
            (EditCommand::Copy, "Copy", "CmdOrCtrl+C"),
            (EditCommand::Paste, "Paste", "CmdOrCtrl+V"),
            (EditCommand::SelectAll, "Select All", "CmdOrCtrl+A"),
        ] {
            if command == EditCommand::Cut {
                edit.append(&separator)?;
            }
            let item = MenuItem::with_id(title, title, false, Some(shortcut.parse()?));
            edit.append(&item)?;
            edits.push((command, item));
        }
        root.append(&edit)?;
        root.append(&Submenu::with_items(
            "View",
            true,
            &[&SystemItem::fullscreen(None)],
        )?)?;
        let windows = Submenu::with_items(
            "Window",
            true,
            &[&SystemItem::minimize(None), &SystemItem::maximize(None)],
        )?;
        windows.append(&separator)?;
        windows.append(&SystemItem::close_window(None))?;
        root.append(&windows)?;
        let help = Submenu::new("Help", true);
        root.append(&help)?;
        root.append(&Submenu::with_items(
            "垒桥终端",
            true,
            &[
                &MenuItem::with_id("show-terminal", "显示终端", true, None),
                &separator,
                &MenuItem::with_id("quit-bridge", "退出垒桥终端与后台", true, None),
            ],
        )?)?;
        root.init_for_nsapp();
        windows.set_as_windows_menu_for_nsapp();
        help.set_as_help_menu_for_nsapp();
        cx.set_global(NativeMenu { _root: root, edits });
        let (send, mut events) = tokio::sync::mpsc::unbounded_channel();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let _ = send.send(e.id);
        }));
        cx.spawn(async move |cx| {
            while let Some(id) = events.recv().await {
                match id.as_ref() {
                    "quit" | "quit-bridge" => {
                        let _ = cx.update(|cx| cx.quit());
                    }
                    "show-terminal" => {
                        let _ = cx.update(crate::show);
                    }
                    _ => {
                        let _ = window.update(cx, |view, w, cx| {
                            let command = cx
                                .global::<NativeMenu>()
                                .edits
                                .iter()
                                .find(|(_, i)| i.id() == &id)
                                .map(|(cmd, _)| *cmd);
                            if let Some(command) = command {
                                view.menu_edit(command, w, cx);
                            }
                        });
                    }
                }
            }
        })
        .detach();
        Ok(())
    }
}
