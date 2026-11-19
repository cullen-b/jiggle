//! The Settings window: the same settings as the Jiggle menu, plus the list of apps the user added.

use std::cell::{OnceCell, RefCell};

use objc2::rc::Retained;
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSButton, NSColor, NSControlStateValueOff, NSControlStateValueOn, NSFont,
    NSSlider, NSTextField, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSFileManager, NSObject, NSPoint, NSRect, NSSize, NSString};

use crate::settings;

const WIDTH: f64 = 420.0;
const MARGIN: f64 = 20.0;
const ROW: f64 = 28.0;

struct State {
    window: Retained<NSWindow>,
    target: Retained<Target>,
}

thread_local! {
    static STATE: OnceCell<State> = const { OnceCell::new() };
    static OPACITY_LABEL: RefCell<Option<Retained<NSTextField>>> = const { RefCell::new(None) };
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "JiggleSettingsTarget"]
    struct Target;

    impl Target {
        #[unsafe(method(toggleOpenOnly:))]
        fn toggle_open_only(&self, button: &NSButton) {
            settings::set_open_apps_only(button.state() == NSControlStateValueOn);
            settings::changed();
        }

        // Live while dragging, so only the label updates here (rebuilding would yank the slider away).
        #[unsafe(method(opacityChanged:))]
        fn opacity_changed(&self, slider: &NSSlider) {
            settings::set_opacity_percent(slider.integerValue());
            set_opacity_label(settings::opacity_percent());
        }

        #[unsafe(method(removeApp:))]
        fn remove_app(&self, button: &NSButton) {
            if let Some(path) = settings::extra_apps().get(button.tag() as usize) {
                settings::set_extra_app(path, false);
            }
            settings::changed();
        }

        #[unsafe(method(addApps:))]
        fn add_apps(&self, _button: &NSButton) {
            settings::add_apps(MainThreadMarker::from(self));
            settings::changed();
        }
    }
);

/// Opens the Settings window (or brings it to the front).
pub fn show(mtm: MainThreadMarker) {
    settings::reload();
    let first = STATE.with(|s| s.get().is_none());
    if first {
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, 300.0)),
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str("Jiggle Settings"));
        let target: Retained<Target> = {
            let this = Target::alloc(mtm).set_ivars(());
            unsafe { msg_send![super(this), init] }
        };
        STATE.with(|s| {
            let _ = s.set(State { window, target });
        });
    }
    rebuild(mtm);
    STATE.with(|s| {
        let s = s.get().expect("settings window");
        if first {
            s.window.center();
        }
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
        s.window.makeKeyAndOrderFront(None);
    });
}

/// Redraws the window if it is open, after a setting changed somewhere else.
pub fn refresh() {
    let visible = STATE.with(|s| s.get().is_some_and(|s| s.window.isVisible()));
    if visible {
        rebuild(MainThreadMarker::new().expect("main thread"));
    }
}

fn set_opacity_label(percent: isize) {
    OPACITY_LABEL.with(|l| {
        if let Some(l) = l.borrow().as_ref() {
            l.setStringValue(&NSString::from_str(&format!("{percent}%")));
        }
    });
}

/// Places `view` in the next `h` points down the window, moving `top` past it when `advance`.
fn place(content: &NSView, top: &mut f64, view: &NSView, x: f64, w: f64, h: f64, advance: bool) {
    view.setFrame(NSRect::new(NSPoint::new(x, *top - h), NSSize::new(w, h)));
    content.addSubview(view);
    if advance {
        *top -= h;
    }
}

fn label(text: &str, mtm: MainThreadMarker) -> Retained<NSTextField> {
    NSTextField::labelWithString(&NSString::from_str(text), mtm)
}

/// Lays the window out top to bottom from the current settings.
fn rebuild(mtm: MainThreadMarker) {
    STATE.with(|s| {
        let s = s.get().expect("settings window");
        let target = &s.target;
        let apps = settings::extra_apps();
        let rows = apps.len().max(1) as f64;
        let height = MARGIN + 24.0 + 18.0 + 22.0 + 28.0 + 22.0 + 22.0 + rows * ROW + 8.0 + 30.0 + 20.0 + 48.0 + MARGIN;
        let content = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, height)));
        let inner = WIDTH - 2.0 * MARGIN;
        let mut top = height - MARGIN;

        let open_only = unsafe {
            NSButton::checkboxWithTitle_target_action(
                &NSString::from_str("Show only open apps (plus apps you added)"),
                Some(target),
                Some(sel!(toggleOpenOnly:)),
                mtm,
            )
        };
        open_only.setState(if settings::open_apps_only() { NSControlStateValueOn } else { NSControlStateValueOff });
        place(&content, &mut top, &open_only, MARGIN, inner, 24.0, true);
        top -= 18.0;

        let heading = label("Ring opacity", mtm);
        heading.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
        place(&content, &mut top, &heading, MARGIN, inner, 22.0, true);
        let percent = settings::opacity_percent();
        let slider = unsafe {
            NSSlider::sliderWithValue_minValue_maxValue_target_action(
                percent as f64,
                settings::MIN_OPACITY as f64,
                100.0,
                Some(target),
                Some(sel!(opacityChanged:)),
                mtm,
            )
        };
        slider.setContinuous(true);
        place(&content, &mut top, &slider, MARGIN, inner - 56.0, 28.0, false);
        let value = label(&format!("{percent}%"), mtm);
        place(&content, &mut top, &value, WIDTH - MARGIN - 48.0, 48.0, 22.0, true);
        top -= 6.0;
        OPACITY_LABEL.with(|l| *l.borrow_mut() = Some(value));
        top -= 16.0;

        let heading = label("Apps you added", mtm);
        heading.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
        place(&content, &mut top, &heading, MARGIN, inner, 22.0, true);
        if apps.is_empty() {
            let none = label("None yet. Added apps stay in the ring even when closed.", mtm);
            none.setTextColor(Some(&NSColor::secondaryLabelColor()));
            place(&content, &mut top, &none, MARGIN, inner, ROW, true);
        }
        let fm = NSFileManager::defaultManager();
        for (i, path) in apps.iter().enumerate() {
            let name = fm.displayNameAtPath(&NSString::from_str(path)).to_string();
            let row = label(name.strip_suffix(".app").unwrap_or(&name), mtm);
            row.setToolTip(Some(&NSString::from_str(path)));
            // Nudged down to line up with the button's text.
            row.setFrame(NSRect::new(NSPoint::new(MARGIN, top - ROW + 1.0), NSSize::new(inner - 96.0, ROW - 8.0)));
            content.addSubview(&row);
            let remove = unsafe {
                NSButton::buttonWithTitle_target_action(&NSString::from_str("Remove"), Some(target), Some(sel!(removeApp:)), mtm)
            };
            remove.setTag(i as isize);
            place(&content, &mut top, &remove, WIDTH - MARGIN - 90.0, 90.0, ROW, true);
        }
        top -= 8.0;
        let add = unsafe {
            NSButton::buttonWithTitle_target_action(&NSString::from_str("Add App\u{2026}"), Some(target), Some(sel!(addApps:)), mtm)
        };
        place(&content, &mut top, &add, MARGIN - 6.0, 120.0, 30.0, true);
        top -= 20.0;

        let tip = NSTextField::wrappingLabelWithString(
            &NSString::from_str(
                "You or an AI agent can change these from a terminal too, for example: \
                 defaults write local.jiggle RingOpacity -int 60. The README lists every setting.",
            ),
            mtm,
        );
        tip.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        tip.setTextColor(Some(&NSColor::secondaryLabelColor()));
        tip.setSelectable(true);
        place(&content, &mut top, &tip, MARGIN, inner, 48.0, true);

        s.window.setContentView(Some(&content));
        s.window.setContentSize(NSSize::new(WIDTH, height));
    });
}

