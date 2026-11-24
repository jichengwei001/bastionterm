use crate::theme::Theme;
use anyhow::{Context, Result};
use gpui::{Rgba, Window, rgb};
use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSApplication, NSColor, NSColorSpace, NSColorSpaceModel, NSView, NSWindow,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

fn native(w: &Window) -> Result<Retained<NSWindow>> {
    let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(w)
        .map_err(|e| anyhow::anyhow!("无法读取窗口句柄：{e}"))?
        .as_raw()
    else {
        anyhow::bail!("未找到 macOS 窗口");
    };
    // GPUI owns the view for the duration of this main-thread window callback.
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    view.window().context("未找到终端窗口")
}

pub fn minimized(w: &Window) -> bool {
    native(w).is_ok_and(|window| window.isMiniaturized())
}

#[derive(Default)]
pub struct DisabledButtonColors {
    space: Option<Retained<NSColorSpace>>,
    colors: std::cell::RefCell<std::collections::HashMap<(u32, u32), Rgba>>,
}

impl DisabledButtonColors {
    pub fn update_display(&mut self, window: &Window) {
        let space = native(window).ok().and_then(|w| w.screen()?.colorSpace());
        if self.space != space {
            self.space = space;
            self.colors.get_mut().clear();
        }
    }

    pub fn blend(&self, surface: u32, foreground: u32) -> Option<Rgba> {
        let mut colors = self.colors.borrow_mut();
        if let Some(color) = colors.get(&(surface, foreground)) {
            return Some(*color);
        }
        // WebKit applies group opacity in the display profile. GPUI paints in sRGB.
        let space = self
            .space
            .as_deref()
            .filter(|s| s.colorSpaceModel() == NSColorSpaceModel::RGB)?;
        let convert = |value| {
            let c = rgb(value);
            NSColor::colorWithSRGBRed_green_blue_alpha(c.r.into(), c.g.into(), c.b.into(), 1.)
                .colorUsingColorSpace(space)
        };
        let bg = convert(surface)?;
        let fg = convert(foreground)?;
        let mix = |a: f64, b: f64| a * 0.6 + b * 0.4;
        let mut components = [
            mix(bg.redComponent(), fg.redComponent()),
            mix(bg.greenComponent(), fg.greenComponent()),
            mix(bg.blueComponent(), fg.blueComponent()),
            1.,
        ];
        // The RGB color space takes exactly the four live RGBA components above.
        let composite = unsafe {
            NSColor::colorWithColorSpace_components_count(
                space,
                std::ptr::NonNull::from(&mut components).cast(),
                4,
            )
        };
        let srgb = composite.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
        let result = Rgba {
            r: srgb.redComponent() as f32,
            g: srgb.greenComponent() as f32,
            b: srgb.blueComponent() as f32,
            a: 1.,
        };
        colors.insert((surface, foreground), result);
        Some(result)
    }
}

pub fn prepare(w: &Window, theme: Theme) -> Result<()> {
    let window = native(w)?;
    let mut frame = window.frame();
    // Match the decorated frame of the original terminal, including its native title bar.
    window.setMinSize(window.contentMinSize());
    frame.size = window.contentRectForFrameRect(frame).size;
    if let Some(screen) = window.screen() {
        let area = screen.visibleFrame();
        frame.size.width = frame.size.width.min(area.size.width);
        frame.size.height = frame.size.height.min(area.size.height);
        frame.origin.x = (area.origin.x + (area.size.width - frame.size.width) * 0.5).round();
        frame.origin.y = (area.origin.y + (area.size.height - frame.size.height) * 0.75).round();
    }
    window.setFrame_display(frame, false);
    // AppKit populated the original window list before GPUI attached our menu.
    let app = NSApplication::sharedApplication(MainThreadMarker::from(&*window));
    app.addWindowsItem_title_filename(&window, &window.title(), false);
    tint(&window, theme);
    Ok(())
}

pub fn apply_theme(w: &Window, theme: Theme) -> Result<()> {
    let window = native(w)?;
    tint(&window, theme);
    Ok(())
}

fn tint(window: &NSWindow, theme: Theme) {
    // These are the AppKit framework's immutable appearance names.
    let light = theme.palette()["scheme"] == "light";
    let name = unsafe {
        if light {
            NSAppearanceNameAqua
        } else {
            NSAppearanceNameDarkAqua
        }
    };
    window.setAppearance(NSAppearance::appearanceNamed(name).as_deref());
    let rgb = theme.color("bg");
    // WKWebView shades the native title strip relative to the page background.
    let component = |shift: u32| {
        let channel = f64::from((rgb >> shift) & 0xff) / 255.;
        if light {
            channel * 0.89
        } else {
            channel + (1. - channel) * 0.05
        }
    };
    let color =
        NSColor::colorWithSRGBRed_green_blue_alpha(component(16), component(8), component(0), 1.);
    window.setTitlebarAppearsTransparent(true);
    window.setBackgroundColor(Some(&color));
}

fn anchor(view: &NSView, bounds: gpui::Bounds<gpui::Pixels>) -> objc2_foundation::NSRect {
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    let y = if view.isFlipped() {
        f64::from(bounds.top())
    } else {
        view.bounds().size.height - f64::from(bounds.bottom())
    };
    NSRect::new(
        NSPoint::new(f64::from(bounds.left()), y),
        NSSize::new(f64::from(bounds.size.width), f64::from(bounds.size.height)),
    )
}

pub struct PickerMenu {
    view: Retained<NSView>,
    frame: objc2_foundation::NSRect,
    cell: Retained<objc2_app_kit::NSPopUpButtonCell>,
}

impl PickerMenu {
    pub fn new(
        window: &Window,
        titles: &[&str],
        selected: usize,
    ) -> Option<Self> {
        use objc2_app_kit::{NSFont, NSPopUpButtonCell};
        use objc2_foundation::NSString;
        let bounds = window.focused_bounds()?;
        let view = native(window).ok()?.contentView()?;
        let mtm = MainThreadMarker::from(&*view);
        let cell = NSPopUpButtonCell::initTextCell_pullsDown(
            NSPopUpButtonCell::alloc(mtm),
            &NSString::from_str(""),
            false,
        );
        cell.setAutoenablesItems(false);
        let font = NSFont::fontWithName_size(&NSString::from_str("Avenir Next"), 12.);
        if let Some(font) = font.as_deref() {
            cell.setFont(Some(font));
            // The AppKit setter requires a non-null font.
            unsafe { cell.menu()?.setFont(Some(font)) };
        }
        for title in titles {
            // Add a fresh item even when two saved profiles have the same name.
            cell.addItemWithTitle(&NSString::from_str(""));
            cell.lastItem()?.setTitle(&NSString::from_str(title));
        }
        cell.selectItemAtIndex(selected as isize);
        let frame = anchor(&view, bounds);
        Some(Self { view, frame, cell })
    }

    pub fn show(self) -> Option<usize> {
        self.cell.performClickWithFrame_inView(self.frame, &self.view);
        usize::try_from(self.cell.indexOfSelectedItem()).ok()
    }
}

/// AppKit positions the validation message against the field and supplies its
/// material, arrow, system font metrics and accessibility text.
pub struct ValidationPopover {
    popover: Retained<objc2_app_kit::NSPopover>,
}

impl ValidationPopover {
    pub fn show(w: &Window, bounds: gpui::Bounds<gpui::Pixels>, message: &str) -> Option<Self> {
        use objc2_app_kit::{NSFont, NSPopover, NSPopoverBehavior, NSTextField, NSViewController};
        use objc2_foundation::{NSPoint, NSRect, NSRectEdge, NSSize, NSString};
        let view = native(w).ok()?.contentView()?;
        let mtm = MainThreadMarker::from(&*view);
        let label = NSTextField::labelWithString(&NSString::from_str(message), mtm);
        label.setFont(Some(&NSFont::systemFontOfSize(13.)));
        label.setMaximumNumberOfLines(4);
        let size = label.sizeThatFits(NSSize::new(300., f64::MAX));
        label.setFrame(NSRect::new(NSPoint::new(5., 5.), size));
        let content = NSView::new(mtm);
        content.setFrameSize(NSSize::new(size.width + 10., size.height + 10.));
        content.addSubview(&label);
        let controller = NSViewController::new(mtm);
        controller.setView(&content);
        let popover = NSPopover::new(mtm);
        popover.setContentViewController(Some(&controller));
        popover.setBehavior(NSPopoverBehavior::Transient);
        popover.setAnimates(false);
        let edge = if view.isFlipped() {
            NSRectEdge::MinY
        } else {
            NSRectEdge::MaxY
        };
        popover.showRelativeToRect_ofView_preferredEdge(anchor(&view, bounds), &view, edge);
        Some(Self { popover })
    }

    pub fn reposition(&self, w: &Window, bounds: gpui::Bounds<gpui::Pixels>) {
        if let Some(view) = native(w).ok().and_then(|window| window.contentView()) {
            let rect = anchor(&view, bounds);
            if self.popover.isShown() && self.popover.positioningRect() != rect {
                self.popover.setPositioningRect(rect);
            }
        }
    }
}

impl Drop for ValidationPopover {
    fn drop(&mut self) {
        self.popover.close();
    }
}
