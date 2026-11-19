//! jiggle: shake the mouse to summon your Dock apps in a ring around the pointer.
//! Point toward an app and click to open it; Esc, right-click or clicking the middle dismisses.

use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::f64::consts::{FRAC_PI_2, TAU};
use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSBezierPath, NSColor,
    NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSImage, NSPanel, NSScreen,
    NSStringDrawing, NSView, NSWindowCollectionBehavior, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{
    ns_string, NSArray, NSDictionary, NSFileManager, NSNumber, NSPoint, NSRect, NSSize, NSString,
    NSURL, NSUserDefaults,
};

// Shake: this many direction reversals, each after a leg of at least SHAKE_MIN_LEG points, within SHAKE_WINDOW.
const SHAKE_REVERSALS: usize = 4;
const SHAKE_MIN_LEG: f64 = 40.0;
const SHAKE_WINDOW: Duration = Duration::from_millis(600);
const COOLDOWN: Duration = Duration::from_millis(700);
const ICON: f64 = 52.0;
const PAD: f64 = 22.0;
const DISMISS_DISTANCE: f64 = 450.0;

// CoreGraphics / CoreFoundation / HIServices FFI (just what the event tap needs).
type TapCallback = unsafe extern "C" fn(*mut c_void, u32, *mut c_void, *mut c_void) -> *mut c_void;
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    static kAXTrustedCheckOptionPrompt: *const c_void;
    fn CGEventTapCreate(tap: u32, place: u32, options: u32, mask: u64, cb: TapCallback, user: *mut c_void) -> *mut c_void;
    fn CGEventTapEnable(tap: *mut c_void, enable: bool);
    fn CGEventGetLocation(event: *mut c_void) -> NSPoint;
    fn CGEventGetDoubleValueField(event: *mut c_void, field: u32) -> f64;
    fn CGEventGetIntegerValueField(event: *mut c_void, field: u32) -> i64;
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFMachPortCreateRunLoopSource(alloc: *const c_void, port: *mut c_void, order: isize) -> *mut c_void;
    fn CFRunLoopGetMain() -> *mut c_void;
    fn CFRunLoopAddSource(rl: *mut c_void, source: *mut c_void, mode: *const c_void);
    static kCFRunLoopCommonModes: *const c_void;
}

const L_DOWN: u32 = 1;
const L_UP: u32 = 2;
const R_DOWN: u32 = 3;
const R_UP: u32 = 4;
const MOVED: u32 = 5;
const L_DRAG: u32 = 6;
const R_DRAG: u32 = 7;
const KEY_DOWN: u32 = 10;
const O_DOWN: u32 = 25;
const O_UP: u32 = 26;
const O_DRAG: u32 = 27;
const TAP_DISABLED_TIMEOUT: u32 = 0xFFFF_FFFE;
const TAP_DISABLED_USER: u32 = 0xFFFF_FFFF;
const FIELD_DELTA_X: u32 = 4;
const FIELD_DELTA_Y: u32 = 5;
const FIELD_KEYCODE: u32 = 9;
const KEY_ESCAPE: i64 = 53;

static TAP: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

struct App {
    url: Retained<NSURL>,
    name: Retained<NSString>,
    icon: Retained<NSImage>,
    running: bool,
}

/// Counts quick direction reversals along one axis.
#[derive(Default)]
struct Axis {
    dir: i8,
    leg: f64,
    flips: VecDeque<Instant>,
}

impl Axis {
    fn feed(&mut self, d: f64, now: Instant) -> usize {
        let dir = if d > 0.0 { 1 } else if d < 0.0 { -1 } else { 0 };
        if dir == self.dir {
            self.leg += d.abs();
        } else if dir != 0 && d.abs() >= 3.0 {
            // Small opposite deltas are sensor noise, not a reversal.
            if self.leg >= SHAKE_MIN_LEG {
                self.flips.push_back(now);
            }
            self.dir = dir;
            self.leg = d.abs();
        }
        while self.flips.front().is_some_and(|t| now - *t > SHAKE_WINDOW) {
            self.flips.pop_front();
        }
        self.flips.len()
    }
}

struct Model {
    apps: Vec<App>,
    icons: HashMap<String, Retained<NSImage>>,
    visible: bool,
    center: NSPoint,
    radius: f64,
    selected: Option<usize>,
    x: Axis,
    y: Axis,
    cooldown: Instant,
    swallow_up: bool,
}

struct Ui {
    panel: Retained<NSPanel>,
    view: Retained<RingView>,
}

thread_local! {
    static MODEL: RefCell<Model> = RefCell::new(Model {
        apps: Vec::new(),
        icons: HashMap::new(),
        visible: false,
        center: NSPoint::new(0.0, 0.0),
        radius: 0.0,
        selected: None,
        x: Axis::default(),
        y: Axis::default(),
        cooldown: Instant::now(),
        swallow_up: false,
    });
    static UI: OnceCell<Ui> = const { OnceCell::new() };
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "JiggleRingView"]
    struct RingView;

    impl RingView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            draw(self.bounds());
        }
    }
);

impl RingView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }
}

fn ring_radius(n: usize) -> f64 {
    (n as f64 * (ICON + 12.0) / TAU).max(96.0)
}

fn outer_radius(r: f64) -> f64 {
    r + ICON / 2.0 + PAD
}

/// Offset of slot `i` of `n` from the ring center: first slot at 12 o'clock, then clockwise.
fn slot(i: usize, n: usize, r: f64) -> (f64, f64) {
    let a = FRAC_PI_2 - i as f64 * TAU / n as f64;
    (r * a.cos(), r * a.sin())
}

fn draw(bounds: NSRect) {
    MODEL.with(|m| {
        let Ok(m) = m.try_borrow() else { return };
        let n = m.apps.len();
        if n == 0 {
            return;
        }
        let c = NSPoint::new(bounds.size.width / 2.0, bounds.size.height / 2.0);
        let outer = outer_radius(m.radius);
        let disc = NSBezierPath::bezierPathWithOvalInRect(square(c, outer - 1.0));
        NSColor::colorWithWhite_alpha(0.08, 0.82).setFill();
        disc.fill();
        NSColor::colorWithWhite_alpha(1.0, 0.14).setStroke();
        disc.setLineWidth(1.0);
        disc.stroke();

        for (i, app) in m.apps.iter().enumerate() {
            let (dx, dy) = slot(i, n, m.radius);
            let p = NSPoint::new(c.x + dx, c.y + dy);
            let selected = m.selected == Some(i);
            let size = if selected { ICON * 1.25 } else { ICON };
            if selected {
                NSColor::colorWithWhite_alpha(1.0, 0.22).setFill();
                NSBezierPath::bezierPathWithOvalInRect(square(p, size * 0.66)).fill();
            }
            app.icon.drawInRect(square(p, size / 2.0));
            if app.running {
                NSColor::colorWithWhite_alpha(1.0, 0.75).setFill();
                let dot = NSPoint::new(p.x, p.y - size / 2.0 - 4.0);
                NSBezierPath::bezierPathWithOvalInRect(square(dot, 2.0)).fill();
            }
        }

        if let Some(i) = m.selected {
            let font = NSFont::systemFontOfSize(14.0);
            let color = NSColor::whiteColor();
            let keys = unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] };
            let vals: [&AnyObject; 2] = [&font, &color];
            let attrs = NSDictionary::from_slices(&keys, &vals);
            let name = &m.apps[i].name;
            let sz = unsafe { name.sizeWithAttributes(Some(&attrs)) };
            let at = NSPoint::new(c.x - sz.width / 2.0, c.y - sz.height / 2.0);
            unsafe { name.drawAtPoint_withAttributes(at, Some(&attrs)) };
        }
    });
}

fn square(c: NSPoint, half: f64) -> NSRect {
    NSRect::new(NSPoint::new(c.x - half, c.y - half), NSSize::new(half * 2.0, half * 2.0))
}

/// Finder, then the Dock's pinned apps, then other running apps, like the Dock itself.
fn load_apps(icons: &mut HashMap<String, Retained<NSImage>>) -> Vec<App> {
    let ws = NSWorkspace::sharedWorkspace();
    let mut running = Vec::new();
    for a in ws.runningApplications().iter() {
        if a.activationPolicy() == NSApplicationActivationPolicy::Regular {
            if let Some(p) = a.bundleURL().and_then(|u| u.path()) {
                running.push(p.to_string());
            }
        }
    }
    let running_set: HashSet<&str> = running.iter().map(String::as_str).collect();

    let mut paths = vec!["/System/Library/CoreServices/Finder.app".to_string()];
    if let Some(dock) = NSUserDefaults::standardUserDefaults().persistentDomainForName(ns_string!("com.apple.dock")) {
        let urls: Option<Retained<NSArray>> = unsafe {
            msg_send![&*dock, valueForKeyPath: ns_string!("persistent-apps.tile-data.file-data._CFURLString")]
        };
        for item in urls.iter().flat_map(|a| a.iter()) {
            let Some(s) = item.downcast_ref::<NSString>() else { continue };
            let path = match NSURL::URLWithString(s).filter(|u| u.isFileURL()).and_then(|u| u.path()) {
                Some(p) => p.to_string(),
                None => s.to_string(),
            };
            if std::path::Path::new(&path).exists() {
                paths.push(path);
            }
        }
    }
    paths.extend(running.iter().cloned());

    let fm = NSFileManager::defaultManager();
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for path in paths {
        if !seen.insert(path.clone()) {
            continue;
        }
        let ns = NSString::from_str(&path);
        let icon = icons.entry(path.clone()).or_insert_with(|| ws.iconForFile(&ns)).clone();
        apps.push(App {
            url: NSURL::fileURLWithPath(&ns),
            name: fm.displayNameAtPath(&ns),
            icon,
            running: running_set.contains(path.as_str()),
        });
    }
    apps
}

fn screen_frames(mtm: MainThreadMarker) -> Vec<NSRect> {
    NSScreen::screens(mtm).iter().map(|s| s.frame()).collect()
}

/// CG global coordinates (top-left origin) to AppKit screen coordinates (bottom-left origin).
fn event_point(ev: *mut c_void, mtm: MainThreadMarker) -> NSPoint {
    let p = unsafe { CGEventGetLocation(ev) };
    let h = screen_frames(mtm).first().map_or(0.0, |f| f.size.height);
    NSPoint::new(p.x, h - p.y)
}

fn contains(r: NSRect, p: NSPoint) -> bool {
    p.x >= r.origin.x && p.x <= r.origin.x + r.size.width && p.y >= r.origin.y && p.y <= r.origin.y + r.size.height
}

fn show(p: NSPoint, mtm: MainThreadMarker) {
    let screen = screen_frames(mtm).into_iter().find(|f| contains(*f, p));
    let frame = MODEL.with(|m| {
        let mut m = m.borrow_mut();
        let m = &mut *m;
        m.apps = load_apps(&mut m.icons);
        if m.apps.is_empty() {
            return None;
        }
        m.radius = ring_radius(m.apps.len());
        let outer = outer_radius(m.radius);
        let mut c = p;
        if let Some(f) = screen {
            // Keep the whole ring on the screen the pointer is on.
            c.x = c.x.min(f.origin.x + f.size.width - outer).max(f.origin.x + outer);
            c.y = c.y.min(f.origin.y + f.size.height - outer).max(f.origin.y + outer);
        }
        m.center = c;
        m.selected = None;
        m.visible = true;
        Some(square(c, outer))
    });
    let Some(frame) = frame else { return };
    UI.with(|ui| {
        let ui = ui.get().expect("ui");
        ui.panel.setFrame_display(frame, false);
        ui.view.setNeedsDisplay(true);
        ui.panel.orderFrontRegardless();
    });
}

fn hide() {
    MODEL.with(|m| {
        let mut m = m.borrow_mut();
        m.visible = false;
        m.selected = None;
        m.x = Axis::default();
        m.y = Axis::default();
        m.cooldown = Instant::now() + COOLDOWN;
    });
    UI.with(|ui| ui.get().expect("ui").panel.orderOut(None));
}

fn redraw() {
    UI.with(|ui| ui.get().expect("ui").view.setNeedsDisplay(true));
}

enum Act {
    Nothing,
    Show,
    Hide,
    Redraw,
}

fn on_move(ev: *mut c_void, mtm: MainThreadMarker) {
    let visible = MODEL.with(|m| m.borrow().visible);
    let point = visible.then(|| event_point(ev, mtm));
    let act = MODEL.with(|m| {
        let mut m = m.borrow_mut();
        if let Some(p) = point {
            let (vx, vy) = (p.x - m.center.x, p.y - m.center.y);
            let dist = vx.hypot(vy);
            if dist > m.radius + DISMISS_DISTANCE {
                return Act::Hide;
            }
            let n = m.apps.len();
            let sel = (dist > (m.radius * 0.35).max(28.0)).then(|| {
                let step = TAU / n as f64;
                let k = ((FRAC_PI_2 - vy.atan2(vx)) / step).round() as i64;
                k.rem_euclid(n as i64) as usize
            });
            if sel != m.selected {
                m.selected = sel;
                return Act::Redraw;
            }
            return Act::Nothing;
        }
        let now = Instant::now();
        let (dx, dy) = unsafe { (CGEventGetDoubleValueField(ev, FIELD_DELTA_X), CGEventGetDoubleValueField(ev, FIELD_DELTA_Y)) };
        let flips = m.x.feed(dx, now).max(m.y.feed(dy, now));
        if flips >= SHAKE_REVERSALS && now >= m.cooldown {
            m.x = Axis::default();
            m.y = Axis::default();
            Act::Show
        } else {
            Act::Nothing
        }
    });
    match act {
        Act::Nothing => {}
        Act::Show => show(event_point(ev, mtm), mtm),
        Act::Hide => hide(),
        Act::Redraw => redraw(),
    }
}

/// Returns true when the click was ours and must be swallowed.
fn on_click(left: bool) -> bool {
    let (visible, url) = MODEL.with(|m| {
        let mut m = m.borrow_mut();
        if !m.visible {
            return (false, None);
        }
        m.swallow_up = true;
        let url = if left { m.selected.map(|i| m.apps[i].url.clone()) } else { None };
        (true, url)
    });
    if !visible {
        return false;
    }
    hide();
    if let Some(url) = url {
        NSWorkspace::sharedWorkspace().openURL(&url);
    }
    true
}

unsafe extern "C" fn on_event(_proxy: *mut c_void, ty: u32, ev: *mut c_void, _user: *mut c_void) -> *mut c_void {
    // The tap's run loop source lives on the main run loop.
    let mtm = MainThreadMarker::new_unchecked();
    match ty {
        TAP_DISABLED_TIMEOUT | TAP_DISABLED_USER => CGEventTapEnable(TAP.load(Ordering::Relaxed), true),
        MOVED | L_DRAG | R_DRAG | O_DRAG => on_move(ev, mtm),
        L_DOWN | R_DOWN | O_DOWN => {
            if on_click(ty == L_DOWN) {
                return null_mut();
            }
        }
        L_UP | R_UP | O_UP => {
            if MODEL.with(|m| std::mem::take(&mut m.borrow_mut().swallow_up)) {
                return null_mut();
            }
        }
        KEY_DOWN => {
            if MODEL.with(|m| m.borrow().visible) {
                hide();
                if CGEventGetIntegerValueField(ev, FIELD_KEYCODE) == KEY_ESCAPE {
                    return null_mut();
                }
            }
        }
        _ => {}
    }
    ev
}

fn wait_for_accessibility() {
    let key = unsafe { &*(kAXTrustedCheckOptionPrompt as *const NSString) };
    let yes = NSNumber::new_bool(true);
    let opts = NSDictionary::from_slices(&[key], &[&*yes]);
    if unsafe { AXIsProcessTrustedWithOptions(Retained::as_ptr(&opts).cast()) } {
        return;
    }
    eprintln!("jiggle: grant Accessibility access in System Settings > Privacy & Security > Accessibility");
    while !unsafe { AXIsProcessTrustedWithOptions(std::ptr::null()) } {
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn install_tap() {
    let mask: u64 = [L_DOWN, L_UP, R_DOWN, R_UP, MOVED, L_DRAG, R_DRAG, KEY_DOWN, O_DOWN, O_UP, O_DRAG]
        .iter()
        .fold(0, |acc, t| acc | 1 << t);
    unsafe {
        // Session tap, head insert, active (so clicks on the ring can be swallowed).
        let tap = CGEventTapCreate(1, 0, 0, mask, on_event, null_mut());
        if tap.is_null() {
            eprintln!("jiggle: could not create event tap (Accessibility permission missing?)");
            std::process::exit(1);
        }
        TAP.store(tap, Ordering::Relaxed);
        let src = CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0);
        CFRunLoopAddSource(CFRunLoopGetMain(), src, kCFRunLoopCommonModes);
        CGEventTapEnable(tap, true);
    }
}

fn build_ui(mtm: MainThreadMarker) -> Ui {
    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
        NSPanel::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(10.0, 10.0)),
        NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
        NSBackingStoreType::Buffered,
        false,
    );
    unsafe { panel.setReleasedWhenClosed(false) };
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setHasShadow(false);
    panel.setIgnoresMouseEvents(true);
    panel.setLevel(101); // NSPopUpMenuWindowLevel
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    let view = RingView::new(mtm);
    panel.setContentView(Some(&view));
    Ui { panel, view }
}

fn main() {
    let mtm = MainThreadMarker::new().expect("must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    wait_for_accessibility();
    UI.with(|u| {
        let _ = u.set(build_ui(mtm));
    });
    install_tap();
    app.run();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shake(axis: &mut Axis, legs: usize, step: f64) -> usize {
        let now = Instant::now();
        let mut flips = 0;
        for leg in 0..legs {
            let d = if leg % 2 == 0 { step } else { -step };
            for _ in 0..5 {
                flips = axis.feed(d, now);
            }
        }
        flips
    }

    #[test]
    fn fast_shake_counts_reversals() {
        assert_eq!(shake(&mut Axis::default(), 5, 15.0), 4);
    }

    #[test]
    fn small_wiggles_do_not_count() {
        assert_eq!(shake(&mut Axis::default(), 9, 5.0), 0);
    }

    #[test]
    fn noise_does_not_reverse() {
        let mut a = Axis::default();
        let now = Instant::now();
        for d in [20.0, 20.0, -1.0, 20.0, 20.0] {
            a.feed(d, now);
        }
        assert_eq!((a.dir, a.leg), (1, 80.0));
    }

    #[test]
    fn slots_go_clockwise_from_top() {
        let (x, y) = slot(0, 4, 100.0);
        assert!(x.abs() < 1e-9 && (y - 100.0).abs() < 1e-9);
        let (x, y) = slot(1, 4, 100.0);
        assert!((x - 100.0).abs() < 1e-9 && y.abs() < 1e-9);
    }
}
