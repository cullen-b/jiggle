//! The Settings window: the same settings as the Jiggle menu, plus the list of apps the user added.

use std::cell::{Cell, OnceCell, RefCell};

use objc2::rc::Retained;
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSButton, NSColor, NSControlStateValueOff, NSControlStateValueOn, NSFont,
    NSSlider, NSTextField, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSFileManager, NSObject, NSPoint, NSRect, NSSize, NSString};

use crate::settings;
use crate::shortcut;

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
    static SHAKE_LABEL: RefCell<Option<Retained<NSTextField>>> = const { RefCell::new(None) };
    // Shown under the shortcut row after a key press that can't be a shortcut on its own.
    static SHORTCUT_HINT: Cell<bool> = const { Cell::new(false) };
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

        #[unsafe(method(shakeChanged:))]
        fn shake_changed(&self, slider: &NSSlider) {
            settings::set_shake_sensitivity(slider.integerValue());
            set_shake_label(settings::shake_sensitivity());
        }

        #[unsafe(method(removeApp:))]
        fn remove_app(&self, button: &NSButton) {
            if let Some(path) = settings::extra_apps().get(button.tag() as usize) {
                settings::set_extra_app(path, false);
            }
            settings::changed();
        }

        #[unsafe(method(recordShortcut:))]
        fn record_shortcut(&self, _button: &NSButton) {
            SHORTCUT_HINT.with(|h| h.set(false));
            shortcut::set_recording(true);
            settings::changed();
        }

        #[unsafe(method(stopRecording:))]
        fn stop_recording(&self, _button: &NSButton) {
            shortcut::set_recording(false);
            settings::changed();
        }

        #[unsafe(method(clearShortcut:))]
        fn clear_shortcut(&self, _button: &NSButton) {
            settings::set_shortcut(None);
            settings::changed();
        }

        #[unsafe(method(unhideApp:))]
        fn unhide_app(&self, button: &NSButton) {
            if let Some(path) = settings::hidden_apps().get(button.tag() as usize) {
                settings::set_hidden_app(path, false);
            }
            settings::changed();
        }

        #[unsafe(method(hideApps:))]
        fn hide_apps(&self, _button: &NSButton) {
            settings::hide_apps(MainThreadMarker::from(self));
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

pub fn is_visible() -> bool {
    STATE.with(|s| s.get().is_some_and(|s| s.window.isVisible()))
}

/// A key press while recording a shortcut (called from the event tap).
pub fn recorded_key(key: u16, flags: u64) {
    match shortcut::record(key, flags) {
        shortcut::Recorded::Cancelled => shortcut::set_recording(false),
        shortcut::Recorded::Got(s) => {
            shortcut::set_recording(false);
            settings::set_shortcut(Some(s));
        }
        shortcut::Recorded::NeedsModifier => SHORTCUT_HINT.with(|h| h.set(true)),
    }
    settings::changed();
}

/// Redraws the window if it is open, after a setting changed somewhere else.
pub fn refresh() {
    if is_visible() {
        rebuild(MainThreadMarker::new().expect("main thread"));
    }
}

const SHAKE_NAMES: [&str; 6] = ["Off", "Very firm", "Firm", "Normal", "Light", "Very light"];

fn set_shake_label(level: isize) {
    SHAKE_LABEL.with(|l| {
        if let Some(l) = l.borrow().as_ref() {
            l.setStringValue(&NSString::from_str(SHAKE_NAMES[level.clamp(0, 5) as usize]));
        }
    });
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

struct AppList<'a> {
    heading: &'a str,
    empty: &'a str,
    apps: &'a [String],
    row_button: (&'a str, objc2::runtime::Sel),
    add_button: (&'a str, objc2::runtime::Sel),
}

fn list_height(rows: usize) -> f64 {
    22.0 + rows.max(1) as f64 * ROW + 8.0 + 30.0 + 20.0
}

/// A heading, one row per app (name + a button tagged with its index), and a button below to add more.
fn app_list(content: &NSView, top: &mut f64, list: &AppList, target: &Target, mtm: MainThreadMarker) {
    let inner = WIDTH - 2.0 * MARGIN;
    let heading = label(list.heading, mtm);
    heading.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
    place(content, top, &heading, MARGIN, inner, 22.0, true);
    if list.apps.is_empty() {
        let none = label(list.empty, mtm);
        none.setTextColor(Some(&NSColor::secondaryLabelColor()));
        place(content, top, &none, MARGIN, inner, ROW, true);
    }
    let fm = NSFileManager::defaultManager();
    for (i, path) in list.apps.iter().enumerate() {
        let name = fm.displayNameAtPath(&NSString::from_str(path)).to_string();
        let row = label(name.strip_suffix(".app").unwrap_or(&name), mtm);
        row.setToolTip(Some(&NSString::from_str(path)));
        // Nudged down to line up with the button's text.
        row.setFrame(NSRect::new(NSPoint::new(MARGIN, *top - ROW + 1.0), NSSize::new(inner - 116.0, ROW - 8.0)));
        content.addSubview(&row);
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(&NSString::from_str(list.row_button.0), Some(target), Some(list.row_button.1), mtm)
        };
        button.setTag(i as isize);
        place(content, top, &button, WIDTH - MARGIN - 110.0, 110.0, ROW, true);
    }
    *top -= 8.0;
    let add = unsafe {
        NSButton::buttonWithTitle_target_action(&NSString::from_str(list.add_button.0), Some(target), Some(list.add_button.1), mtm)
    };
    place(content, top, &add, MARGIN - 6.0, 120.0, 30.0, true);
    *top -= 20.0;
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
        let hidden = settings::hidden_apps();
        let recording = shortcut::is_recording();
        let hint = recording && SHORTCUT_HINT.with(Cell::get);
        let height = MARGIN + 24.0 + 18.0 + 22.0 + 28.0 + 22.0 + 22.0 + 28.0 + 18.0 + 22.0
            + list_height(apps.len())
            + list_height(hidden.len())
            + 22.0 + 30.0 + if hint { 22.0 } else { 0.0 } + 20.0 + 48.0 + MARGIN;
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

        let heading = label("Shake sensitivity", mtm);
        heading.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
        place(&content, &mut top, &heading, MARGIN, inner, 22.0, true);
        let level = settings::shake_sensitivity();
        let slider = unsafe {
            NSSlider::sliderWithValue_minValue_maxValue_target_action(
                level as f64,
                0.0,
                settings::MAX_SHAKE as f64,
                Some(target),
                Some(sel!(shakeChanged:)),
                mtm,
            )
        };
        slider.setNumberOfTickMarks(settings::MAX_SHAKE + 1);
        slider.setAllowsTickMarkValuesOnly(true);
        slider.setContinuous(true);
        slider.setToolTip(Some(&NSString::from_str("Left needs a bigger, longer shake; Off opens Jiggle only with the shortcut")));
        place(&content, &mut top, &slider, MARGIN, inner - 96.0, 28.0, false);
        let value = label(SHAKE_NAMES[level as usize], mtm);
        place(&content, &mut top, &value, WIDTH - MARGIN - 88.0, 88.0, 22.0, true);
        top -= 6.0;
        SHAKE_LABEL.with(|l| *l.borrow_mut() = Some(value));
        let caption = label("Firmer needs a bigger shake. Off: open Jiggle with the shortcut only.", mtm);
        caption.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        caption.setTextColor(Some(&NSColor::secondaryLabelColor()));
        place(&content, &mut top, &caption, MARGIN, inner, 18.0, true);
        top -= 16.0;

        app_list(&content, &mut top, &AppList {
            heading: "Apps you added",
            empty: "None yet. Added apps stay in the ring even when closed.",
            apps: &apps,
            row_button: ("Remove", sel!(removeApp:)),
            add_button: ("Add App\u{2026}", sel!(addApps:)),
        }, target, mtm);
        app_list(&content, &mut top, &AppList {
            heading: "Hidden apps",
            empty: "None. Hidden apps never appear in the ring, even when open.",
            apps: &hidden,
            row_button: ("Show Again", sel!(unhideApp:)),
            add_button: ("Hide App\u{2026}", sel!(hideApps:)),
        }, target, mtm);

        let heading = label("Keyboard shortcut", mtm);
        heading.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
        place(&content, &mut top, &heading, MARGIN, inner, 22.0, true);
        let current = settings::shortcut();
        let status = match (recording, current) {
            (true, _) => "Press the keys now (Esc cancels)".to_string(),
            (false, Some(s)) => s.display(),
            (false, None) => "Off".to_string(),
        };
        let status = label(&status, mtm);
        if current.is_none() && !recording {
            status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        }
        // The status text gets whatever room the buttons on the right leave.
        let buttons_width = match (recording, current) {
            (true, _) => 120.0,
            (false, Some(_)) => 240.0,
            (false, None) => 170.0,
        };
        status.setFrame(NSRect::new(NSPoint::new(MARGIN, top - 27.0), NSSize::new(inner - buttons_width, 22.0)));
        content.addSubview(&status);
        let button = |title: &str, action| unsafe {
            NSButton::buttonWithTitle_target_action(&NSString::from_str(title), Some(target), Some(action), mtm)
        };
        if recording {
            place(&content, &mut top, &button("Cancel", sel!(stopRecording:)), WIDTH - MARGIN - 110.0, 110.0, 30.0, true);
        } else {
            let record = button(if current.is_some() { "Change\u{2026}" } else { "Record Shortcut\u{2026}" }, sel!(recordShortcut:));
            if current.is_some() {
                place(&content, &mut top, &record, WIDTH - MARGIN - 228.0, 110.0, 30.0, false);
                place(&content, &mut top, &button("Turn Off", sel!(clearShortcut:)), WIDTH - MARGIN - 110.0, 110.0, 30.0, true);
            } else {
                place(&content, &mut top, &record, WIDTH - MARGIN - 160.0, 160.0, 30.0, true);
            }
        }
        if hint {
            let h = label("Include \u{2318}, \u{2303} or \u{2325} (or use an F-key).", mtm);
            h.setTextColor(Some(&NSColor::systemRedColor()));
            place(&content, &mut top, &h, MARGIN, inner, 22.0, true);
        }
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

