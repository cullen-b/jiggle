//! jiggle: shake the mouse to summon your Dock apps in a ring around the pointer.
//! The ring is a pie: click anywhere in an app's slice to open it. Clicking outside the ring, its center,
//! or pressing Esc closes it. Right-clicking an app shows its Dock-style menu; right-clicking the center hub
//! (or the menu bar icon) shows the Jiggle menu with the settings.

mod appmenu;
mod dock;
mod settings;
mod settings_window;
mod shortcut;
mod update;

use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSBezierPath, NSColor, NSEvent,
    NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSImage, NSPanel, NSScreen,
    NSStringDrawing, NSView, NSWindowCollectionBehavior, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{NSDictionary, NSFileManager, NSNumber, NSPoint, NSRect, NSSize, NSString, NSURL};

// Shake: this many direction reversals, each after a leg of at least SHAKE_MIN_LEG points, within SHAKE_WINDOW.
const SHAKE_REVERSALS: usize = 4;
const SHAKE_MIN_LEG: f64 = 40.0;
const SHAKE_WINDOW: Duration = Duration::from_millis(600);
const COOLDOWN: Duration = Duration::from_millis(700);
const ICON: f64 = 52.0;
const PAD: f64 = 22.0;
const DISMISS_DISTANCE: f64 = 450.0;
// The center hub: a left click there closes the ring, a right click opens the Jiggle menu.
const CENTER_DEAD: f64 = 24.0;
const SELECTED_SCALE: f64 = 1.25;

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
    fn CGEventGetFlags(event: *mut c_void) -> u64;
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
const FIELD_AUTOREPEAT: u32 = 8;
const KEY_ESCAPE: i64 = 53;

static TAP: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

struct App {
    path: String,
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

/// Smoothed values the ring is drawn with; they ease toward their targets on every frame.
struct Anim {
    running: bool,
    last: Instant,
    instant: bool,
    appear: f64,
    wedge_angle: f64,
    wedge_alpha: f64,
    scales: Vec<f64>,
}

struct Model {
    apps: Vec<App>,
    opacity: f64,
    anim: Anim,
    icons: HashMap<String, Retained<NSImage>>,
    visible: bool,
    center: NSPoint,
    radius: f64,
    selected: Option<usize>,
    x: Axis,
    y: Axis,
    cooldown: Instant,
    swallow_up: bool,
    // What the pending right-click menu is for (an app, or None for the Jiggle menu), and whether a menu is open.
    menu_for: Option<Option<usize>>,
    menu_open: bool,
}

struct Ui {
    panel: Retained<NSPanel>,
    view: Retained<RingView>,
}

thread_local! {
    static MODEL: RefCell<Model> = RefCell::new(Model {
        apps: Vec::new(),
        opacity: 0.8,
        anim: Anim {
            running: false,
            last: Instant::now(),
            instant: false,
            appear: 1.0,
            wedge_angle: 0.0,
            wedge_alpha: 0.0,
            scales: Vec::new(),
        },
        icons: HashMap::new(),
        visible: false,
        center: NSPoint::new(0.0, 0.0),
        radius: 0.0,
        selected: None,
        x: Axis::default(),
        y: Axis::default(),
        cooldown: Instant::now(),
        swallow_up: false,
        menu_for: None,
        menu_open: false,
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

        #[unsafe(method(jiggleMenu))]
        fn open_menu(&self) {
            show_app_menu();
        }

        #[unsafe(method(jiggleTick))]
        fn tick(&self) {
            let (running, appear) = step_anim();
            UI.with(|ui| ui.get().expect("ui").panel.setAlphaValue(appear));
            self.setNeedsDisplay(true);
            if running {
                schedule_tick();
            }
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

/// Angle of slot `i` of `n`: first slot at 12 o'clock, then clockwise.
fn slot_angle(i: usize, n: usize) -> f64 {
    FRAC_PI_2 - i as f64 * TAU / n as f64
}

/// The slot whose pie slice contains the offset (vx, vy) from the center, if it's inside the ring.
fn slice_at(vx: f64, vy: f64, n: usize, outer: f64) -> Option<usize> {
    let d = vx.hypot(vy);
    if n == 0 || d < CENTER_DEAD || d > outer {
        return None;
    }
    let k = ((FRAC_PI_2 - vy.atan2(vx)) / (TAU / n as f64)).round() as i64;
    Some(k.rem_euclid(n as i64) as usize)
}

/// Shortest signed angular distance, in -PI..PI.
fn wrap_angle(a: f64) -> f64 {
    (a + PI).rem_euclid(TAU) - PI
}

fn draw(bounds: NSRect) {
    MODEL.with(|m| {
        let Ok(m) = m.try_borrow() else { return };
        let n = m.apps.len();
        if n == 0 {
            return;
        }
        let a = &m.anim;
        let c = NSPoint::new(bounds.size.width / 2.0, bounds.size.height / 2.0);
        let outer = outer_radius(m.radius);
        let disc = NSBezierPath::bezierPathWithOvalInRect(square(c, outer - 1.0));
        NSColor::colorWithWhite_alpha(0.08, m.opacity).setFill();
        disc.fill();
        NSColor::colorWithWhite_alpha(1.0, 0.14).setStroke();
        disc.setLineWidth(1.0);
        disc.stroke();
        let hub = NSBezierPath::bezierPathWithOvalInRect(square(c, CENTER_DEAD));
        NSColor::colorWithWhite_alpha(1.0, 0.08).setFill();
        hub.fill();
        if m.selected.is_none() {
            // Three dots hint that the hub has a menu.
            NSColor::colorWithWhite_alpha(1.0, 0.55).setFill();
            for dx in [-7.0, 0.0, 7.0] {
                NSBezierPath::bezierPathWithOvalInRect(square(NSPoint::new(c.x + dx, c.y), 1.8)).fill();
            }
        }

        // The selected app's pie slice.
        if a.wedge_alpha > 0.01 {
            let half = 180.0 / n as f64;
            let mid = a.wedge_angle.to_degrees();
            let wedge = NSBezierPath::bezierPath();
            wedge.moveToPoint(c);
            wedge.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle(c, outer - 1.0, mid - half, mid + half);
            wedge.closePath();
            NSColor::colorWithWhite_alpha(1.0, 0.16 * a.wedge_alpha).setFill();
            wedge.fill();
        }

        // Icons start a little inward and ease out to the ring as it appears.
        let r = m.radius * (0.82 + 0.18 * a.appear);
        for (i, app) in m.apps.iter().enumerate() {
            let ang = slot_angle(i, n);
            let p = NSPoint::new(c.x + r * ang.cos(), c.y + r * ang.sin());
            let size = ICON * a.scales.get(i).copied().unwrap_or(1.0);
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

/// Advances the animation one frame. Returns whether it is still moving, and the window alpha.
fn step_anim() -> (bool, f64) {
    MODEL.with(|m| {
        let mut m = m.borrow_mut();
        let m = &mut *m;
        let n = m.apps.len();
        let a = &mut m.anim;
        if !m.visible {
            a.running = false;
            return (false, 1.0);
        }
        let now = Instant::now();
        let dt = (now - a.last).as_secs_f64().min(0.05);
        a.last = now;
        let ease = |rate: f64| if a.instant { 1.0 } else { 1.0 - (-dt * rate).exp() };
        let (k, k_appear) = (ease(20.0), ease(14.0));

        a.appear += (1.0 - a.appear) * k_appear;
        match m.selected {
            Some(i) => {
                let target = slot_angle(i, n);
                if a.wedge_alpha < 0.02 {
                    a.wedge_angle = target;
                } else {
                    a.wedge_angle += wrap_angle(target - a.wedge_angle) * k;
                }
                a.wedge_alpha += (1.0 - a.wedge_alpha) * k;
            }
            None => a.wedge_alpha -= a.wedge_alpha * k,
        }
        let mut settled = 1.0 - a.appear < 0.002;
        let wedge_target = if m.selected.is_some() { 1.0 } else { 0.0 };
        settled &= (wedge_target - a.wedge_alpha).abs() < 0.002;
        if let Some(i) = m.selected {
            settled &= wrap_angle(slot_angle(i, n) - a.wedge_angle).abs() < 0.001;
        }
        for (j, s) in a.scales.iter_mut().enumerate() {
            let t = if m.selected == Some(j) { SELECTED_SCALE } else { 1.0 };
            *s += (t - *s) * k;
            settled &= (t - *s).abs() < 0.002;
        }
        if settled {
            a.appear = 1.0;
            a.wedge_alpha = wedge_target;
            for (j, s) in a.scales.iter_mut().enumerate() {
                *s = if m.selected == Some(j) { SELECTED_SCALE } else { 1.0 };
            }
            a.running = false;
        }
        (!settled, a.appear)
    })
}

fn start_anim() {
    let start = MODEL.with(|m| {
        let mut m = m.borrow_mut();
        if m.anim.running {
            return false;
        }
        m.anim.running = true;
        m.anim.last = Instant::now();
        true
    });
    if start {
        schedule_tick();
    }
}

fn schedule_tick() {
    UI.with(|ui| {
        let view = &ui.get().expect("ui").view;
        let none: Option<&AnyObject> = None;
        let _: () = unsafe { msg_send![&**view, performSelector: sel!(jiggleTick), withObject: none, afterDelay: 1.0f64 / 120.0] };
    });
}

fn square(c: NSPoint, half: f64) -> NSRect {
    NSRect::new(NSPoint::new(c.x - half, c.y - half), NSSize::new(half * 2.0, half * 2.0))
}

/// Finder, the Dock's pinned apps, the user's own added apps, then other running apps.
/// With `open_only`, only running apps and the user's own added apps.
fn load_apps(icons: &mut HashMap<String, Retained<NSImage>>, open_only: bool) -> Vec<App> {
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
    paths.extend(dock::pinned_paths().into_iter().filter(|p| std::path::Path::new(p).exists()));
    let extras: HashSet<String> = settings::extra_apps().into_iter().filter(|p| std::path::Path::new(p).exists()).collect();
    paths.extend(settings::extra_apps().into_iter().filter(|p| extras.contains(p)));
    paths.extend(running.iter().cloned());

    let fm = NSFileManager::defaultManager();
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for path in paths {
        let running = running_set.contains(path.as_str());
        if (open_only && !running && !extras.contains(&path)) || !seen.insert(path.clone()) {
            continue;
        }
        let ns = NSString::from_str(&path);
        let icon = icons.entry(path.clone()).or_insert_with(|| ws.iconForFile(&ns)).clone();
        apps.push(App {
            path: path.clone(),
            url: NSURL::fileURLWithPath(&ns),
            name: app_name(&fm.displayNameAtPath(&ns)),
            icon,
            running,
        });
    }
    apps
}

/// Display name without ".app", which Finder adds when "Show all filename extensions" is on.
fn app_name(display: &NSString) -> Retained<NSString> {
    let s = display.to_string();
    NSString::from_str(s.strip_suffix(".app").unwrap_or(&s))
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
        settings::reload();
        m.apps = load_apps(&mut m.icons, settings::open_apps_only());
        m.opacity = settings::ring_opacity();
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
        m.anim.instant = NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
        m.anim.appear = 0.0;
        m.anim.wedge_alpha = 0.0;
        m.anim.scales = vec![1.0; m.apps.len()];
        Some(square(c, outer))
    });
    let Some(frame) = frame else { return };
    UI.with(|ui| {
        let ui = ui.get().expect("ui");
        ui.panel.setAlphaValue(0.0);
        ui.panel.setFrame_display(frame, false);
        ui.view.setNeedsDisplay(true);
        ui.panel.orderFrontRegardless();
    });
    start_anim();
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

enum Act {
    Nothing,
    Show,
    Hide,
    Animate,
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
            let sel = slice_at(vx, vy, m.apps.len(), outer_radius(m.radius));
            if sel != m.selected {
                m.selected = sel;
                return Act::Animate;
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
        Act::Animate => start_anim(),
    }
}

/// Right-click on an app opens its menu, on the center hub the Jiggle menu (once this event returns);
/// anywhere else it closes the ring.
fn on_right_click(p: NSPoint) -> bool {
    let (visible, target) = MODEL.with(|m| {
        let mut m = m.borrow_mut();
        if !m.visible {
            return (false, None);
        }
        m.swallow_up = true;
        let on_hub = (p.x - m.center.x).hypot(p.y - m.center.y) < CENTER_DEAD;
        let target = if on_hub { Some(None) } else { m.selected.map(Some) };
        m.menu_for = target;
        (true, target)
    });
    if !visible {
        return false;
    }
    match target {
        Some(_) => UI.with(|ui| {
            let view = &ui.get().expect("ui").view;
            let none: Option<&AnyObject> = None;
            let _: () = unsafe { msg_send![&**view, performSelector: sel!(jiggleMenu), withObject: none, afterDelay: 0.0f64] };
        }),
        None => hide(),
    }
    true
}

fn show_app_menu() {
    let mtm = MainThreadMarker::new().expect("main thread");
    // Some(Some(path)) for an app's menu, Some(None) for the Jiggle menu.
    let target = MODEL.with(|m| {
        let mut m = m.borrow_mut();
        let target = m.menu_for.take().filter(|_| m.visible).map(|t| t.and_then(|i| m.apps.get(i)).map(|a| a.path.clone()));
        m.menu_open = target.is_some();
        target
    });
    let Some(target) = target else { return };
    // The ring stays up behind the menu so it's clear what it's for; events pass through meanwhile.
    match target {
        Some(path) => appmenu::popup(&path, NSEvent::mouseLocation(), mtm),
        None => settings::popup_menu(NSEvent::mouseLocation()),
    }
    MODEL.with(|m| m.borrow_mut().menu_open = false);
    hide();
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

/// Shortcut handling for a key press; returns true when the key was ours and must be swallowed.
fn on_key(ev: *mut c_void) -> bool {
    let (key, flags, repeat) = unsafe {
        (
            CGEventGetIntegerValueField(ev, FIELD_KEYCODE) as u16,
            CGEventGetFlags(ev),
            CGEventGetIntegerValueField(ev, FIELD_AUTOREPEAT) != 0,
        )
    };
    if shortcut::is_recording() {
        // Recording only makes sense while the Settings window is up to show it.
        if !settings_window::is_visible() {
            shortcut::set_recording(false);
            return false;
        }
        if !repeat {
            settings_window::recorded_key(key, flags);
        }
        return true;
    }
    if !shortcut::could_be_shortcut(key, flags) || !settings::shortcut().is_some_and(|s| s.matches(key, flags)) {
        return false;
    }
    // Holding the keys down repeats the press; only the first one toggles the ring.
    if !repeat {
        if MODEL.with(|m| m.borrow().visible) {
            hide();
        } else {
            show(NSEvent::mouseLocation(), MainThreadMarker::new().expect("main thread"));
        }
    }
    true
}

unsafe extern "C" fn on_event(_proxy: *mut c_void, ty: u32, ev: *mut c_void, _user: *mut c_void) -> *mut c_void {
    // The tap's run loop source lives on the main run loop.
    let mtm = MainThreadMarker::new_unchecked();
    match ty {
        TAP_DISABLED_TIMEOUT | TAP_DISABLED_USER => CGEventTapEnable(TAP.load(Ordering::Relaxed), true),
        L_UP | R_UP | O_UP if MODEL.with(|m| std::mem::take(&mut m.borrow_mut().swallow_up)) => return null_mut(),
        _ if MODEL.with(|m| m.borrow().menu_open) => {}
        MOVED | L_DRAG | R_DRAG | O_DRAG => on_move(ev, mtm),
        R_DOWN => {
            if on_right_click(event_point(ev, mtm)) {
                return null_mut();
            }
        }
        L_DOWN | O_DOWN => {
            if on_click(ty == L_DOWN) {
                return null_mut();
            }
        }
        KEY_DOWN => {
            if on_key(ev) {
                return null_mut();
            }
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
    // macOS keeps answering "not trusted" to a running process after the user grants access,
    // so while waiting we re-exec ourselves every few seconds to get a fresh answer.
    let prompted = std::env::var_os("JIGGLE_PROMPTED").is_some();
    let key = unsafe { &*(kAXTrustedCheckOptionPrompt as *const NSString) };
    let flag = NSNumber::new_bool(!prompted);
    let opts = NSDictionary::from_slices(&[key], &[&*flag]);
    if unsafe { AXIsProcessTrustedWithOptions(Retained::as_ptr(&opts).cast()) } {
        return;
    }
    if !prompted {
        eprintln!("jiggle: grant Accessibility access in System Settings > Privacy & Security > Accessibility");
        update::check_now();
    }
    for _ in 0..3 {
        std::thread::sleep(Duration::from_secs(1));
        if unsafe { AXIsProcessTrustedWithOptions(std::ptr::null()) } {
            return;
        }
    }
    let exe = std::env::current_exe().expect("current exe");
    let err = std::os::unix::process::CommandExt::exec(std::process::Command::new(exe).env("JIGGLE_PROMPTED", "1"));
    eprintln!("jiggle: re-exec failed: {err}");
    std::process::exit(1);
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
    update::spawn();
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    wait_for_accessibility();
    UI.with(|u| {
        let _ = u.set(build_ui(mtm));
    });
    install_tap();
    settings::install_menu(mtm);
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
        assert!((slot_angle(0, 4) - FRAC_PI_2).abs() < 1e-9);
        assert!(slot_angle(1, 4).abs() < 1e-9);
    }

    #[test]
    fn slices_cover_the_whole_pie_inside_the_ring() {
        // Four apps: top, right, bottom, left. Just past the center already picks a slice.
        assert_eq!(slice_at(0.0, 30.0, 4, 150.0), Some(0));
        assert_eq!(slice_at(30.0, 0.0, 4, 150.0), Some(1));
        assert_eq!(slice_at(0.0, -140.0, 4, 150.0), Some(2));
        assert_eq!(slice_at(-100.0, 10.0, 4, 150.0), Some(3));
        // Slice edges sit halfway between icons.
        assert_eq!(slice_at(30.0, 31.0, 4, 150.0), Some(0));
        assert_eq!(slice_at(31.0, 30.0, 4, 150.0), Some(1));
    }

    #[test]
    fn center_and_outside_select_nothing() {
        assert_eq!(slice_at(3.0, 4.0, 4, 150.0), None);
        assert_eq!(slice_at(0.0, 20.0, 4, 150.0), None);
        assert_eq!(slice_at(0.0, 151.0, 4, 150.0), None);
        assert_eq!(slice_at(0.0, 50.0, 0, 150.0), None);
    }

    #[test]
    fn wrap_takes_the_short_way_round() {
        assert!((wrap_angle(TAU - 0.1) + 0.1).abs() < 1e-9);
        assert!((wrap_angle(-TAU + 0.1) - 0.1).abs() < 1e-9);
    }
}
