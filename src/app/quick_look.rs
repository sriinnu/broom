//! A native Quick Look view in a floating AppKit panel.
use std::cell::RefCell;
use std::path::Path;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{MainThreadMarker, MainThreadOnly, msg_send};
use objc2_app_kit::{NSBackingStoreType, NSPanel, NSView, NSWindowStyleMask};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

#[link(name = "QuickLookUI", kind = "framework")]
unsafe extern "C" {
    #[link_name = "OBJC_CLASS_$_QLPreviewView"]
    static QL_PREVIEW_VIEW_CLASS: AnyClass;
}

thread_local! {
    static PREVIEW: RefCell<Option<(gpui::WindowId, Retained<NSPanel>)>> = const { RefCell::new(None) };
}

pub(super) fn observe_window_close(cx: &gpui::App) {
    cx.on_window_closed(|_, closed| {
        let panel = PREVIEW.with(|slot| {
            let mut current = slot.borrow_mut();
            if current.as_ref().is_some_and(|(owner, _)| *owner == closed) {
                current.take().map(|(_, panel)| panel)
            } else {
                None
            }
        });
        if let Some(panel) = panel {
            panel.close();
        }
    }).detach();
}

pub(super) fn show(path: &Path, owner: gpui::WindowId) -> bool {
    let (Some(mtm), Ok(meta)) = (MainThreadMarker::new(), path.symlink_metadata()) else { return false };
    let Some(url) = crate::file_url(path, meta.is_dir()) else { return false };
    let frame = NSRect::new(NSPoint::ZERO, NSSize::new(720., 520.));
    // QLPreviewView is an NSView, and NSURL conforms to QLPreviewItem.
    let Some(view) = (unsafe {
        let allocated: Allocated<NSView> = msg_send![&QL_PREVIEW_VIEW_CLASS, alloc];
        let view: Option<Retained<NSView>> = msg_send![allocated, initWithFrame: frame];
        view
    }) else { return false };
    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
        NSPanel::alloc(mtm),
        frame,
        NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Resizable,
        NSBackingStoreType::Buffered,
        false,
    );
    // We own the panel and keep it alive until it is replaced. The preview
    // releases its renderer when the panel is closed.
    unsafe {
        panel.setReleasedWhenClosed(false);
        let _: () = msg_send![&*view, setShouldCloseWithWindow: true];
        let _: () = msg_send![&*view, setPreviewItem: &*url];
    }
    if let Some(name) = path.file_name() {
        panel.setTitle(&NSString::from_str(&name.to_string_lossy()));
    }
    panel.setFloatingPanel(true);
    panel.setHidesOnDeactivate(true);
    panel.setContentView(Some(&view));
    panel.center();
    let previous = PREVIEW.with(|slot| slot.borrow_mut().replace((owner, panel.clone())));
    if let Some((_, previous)) = previous {
        previous.close();
    }
    panel.makeKeyAndOrderFront(None::<&AnyObject>);
    true
}
