//! jiggle: shake the mouse to summon your Dock apps in a ring around the pointer.
//! The ring is a pie: click anywhere in an app's slice to open it. Clicking outside the ring, its center,
//! or pressing Esc closes it. Right-clicking an app shows its Dock-style menu; right-clicking the center hub
//! (or the menu bar icon) shows the Jiggle menu with the settings.

mod appmenu;
mod dock;
mod order;
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

/// What counts as a shake: `reversals` direction changes, each after a leg of at least `min_leg` points,
/// all within `window`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ShakeParams {
    reversals: usize,
    min_leg: f64,
    window: Duration,
}

/// The shake for a sensitivity level (see `settings::shake_sensitivity`); None when shaking is off.
fn shake_params(level: isize) -> Option<ShakeParams> {
    let (reversals, min_leg, ms) = match level {
        i if i <= 0 => return None,
        1 => (6, 70.0, 700),
        2 => (5, 55.0, 650),
        3 => (4, 40.0, 600),
        4 => (3, 38.0, 600),
        5 => (3, 32.0, 600),
        _ => (3, 22.0, 650),
    };
    Some(ShakeParams { reversals, min_leg, window: Duration::from_millis(ms) })
}

// How often the mouse hook re-reads the sensitivity, so a change from a terminal takes effect on its own.
const SHAKE_RECHECK: Duration = Duration::from_secs(2);
const COOLDOWN: Duration = Duration::from_millis(700);
const ICON: f64 = 52.0;
const PAD: f64 = 22.0;
const DISMISS_DISTANCE: f64 = 450.0;
// The center hub: a left click there closes the ring, a right click opens the Jiggle menu.
const CENTER_DEAD: f64 = 24.0;
// Aiming: as the pointer moves from the hub toward the ring, the slice it points into widens and the icons
// near it grow while the far side makes room, so the app you're heading for becomes a bigger target.
const AIM_GROW: f64 = 1.1; // the aimed slice's share grows by up to this much (2.1x at the ring)
const AIM_SPREAD: f64 = 0.9; // how far, in slots, the growth reaches toward neighbors
const AIM_ICON_GROW: f64 = 0.45;
const AIM_ICON_SHRINK: f64 = 0.12;
const SELECTED_BONUS: f64 = 0.1;
// Moving this far with the button down turns a click on an app into a drag that reorders the ring.
const DRAG_THRESHOLD: f64 = 6.0;
const DRAG_SCALE: f64 = 1.2;

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
    fn feed(&mut self, d: f64, now: Instant, shake: &ShakeParams) -> usize {
        let dir = if d > 0.0 { 1 } else if d < 0.0 { -1 } else { 0 };
        if dir == self.dir {
            self.leg += d.abs();
        } else if dir != 0 && d.abs() >= 3.0 {
            // Small opposite deltas are sensor noise, not a reversal.
            if self.leg >= shake.min_leg {
                self.flips.push_back(now);
            }
            self.dir = dir;
            self.leg = d.abs();
        }
        while self.flips.front().is_some_and(|t| now - *t > shake.window) {
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
    // 0 = hidden, 1 = fully shown; `closing` fades it back out before the window is ordered out.
    appear: f64,
    closing: bool,
    wedge_angle: f64,
    wedge_half: f64,
    wedge_alpha: f64,
    // Per slot: slice center and width (radians) and icon scale. Hit-testing uses these too.
    centers: Vec<f64>,
    widths: Vec<f64>,
    scales: Vec<f64>,
}

/// A left button press on an app: a click if released in place, a drag if it moves.
struct Press {
    start: NSPoint,
    index: usize,
    dragging: bool,
}

struct Model {
    apps: Vec<App>,
    press: Option<Press>,
    // While dragging: the dragged app's current slot, and the pointer (screen coordinates).
    drag: Option<usize>,
    drag_point: NSPoint,
    opacity: f64,
    anim: Anim,
    icons: HashMap<String, Retained<NSImage>>,
    visible: bool,
    center: NSPoint,
    radius: f64,
    selected: Option<usize>,
    // How far the pointer is from the hub toward the ring, 0..1, eased.
    aim: f64,
    x: Axis,
    y: Axis,
    shake: Option<ShakeParams>,
    shake_checked: Option<Instant>,
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
        press: None,
        drag: None,
        drag_point: NSPoint::new(0.0, 0.0),
        opacity: 0.8,
        anim: Anim {
            running: false,
            last: Instant::now(),
            instant: false,
            appear: 1.0,
            closing: false,
            wedge_angle: 0.0,
            wedge_half: 0.0,
            wedge_alpha: 0.0,
            centers: Vec::new(),
            widths: Vec::new(),
            scales: Vec::new(),
        },
        icons: HashMap::new(),
        visible: false,
        center: NSPoint::new(0.0, 0.0),
        radius: 0.0,
        selected: None,
        aim: 0.0,
        x: Axis::default(),
        y: Axis::default(),
        shake: shake_params(settings::DEFAULT_SHAKE),
        shake_checked: None,
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
            let (running, appear, closed) = step_anim();
            UI.with(|ui| {
                let ui = ui.get().expect("ui");
                ui.panel.setAlphaValue(appear);
                if closed {
                    ui.panel.orderOut(None);
                }
            });
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

/// Slice centers, slice widths (radians) and icon scales for `n` slots when aiming at `sel` with the pointer
/// `t` (0 at the hub, 1 at the ring) of the way out. The aimed slice keeps its center; the others make room.
fn aim_layout(n: usize, sel: Option<usize>, t: f64) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let even = || ((0..n).map(|i| slot_angle(i, n)).collect(), vec![TAU / n as f64; n], vec![1.0; n]);
    let Some(s) = sel.filter(|&s| s < n) else { return even() };
    // Falloff by distance in slots from the aimed one, going the short way round.
    let g: Vec<f64> = (0..n)
        .map(|i| {
            let d = (i as isize - s as isize).rem_euclid(n as isize) as f64;
            (-(d.min(n as f64 - d) / AIM_SPREAD).powi(2)).exp()
        })
        .collect();
    let weights: Vec<f64> = g.iter().map(|g| 1.0 + AIM_GROW * t * g).collect();
    let total: f64 = weights.iter().sum();
    let widths: Vec<f64> = weights.iter().map(|w| TAU * w / total).collect();
    let mut centers = vec![0.0; n];
    centers[s] = slot_angle(s, n);
    for k in 1..n {
        let (i, prev) = ((s + k) % n, (s + k - 1) % n);
        centers[i] = centers[prev] - (widths[prev] + widths[i]) / 2.0;
    }
    let scales = (0..n)
        .map(|i| {
            let bonus = if i == s { SELECTED_BONUS } else { 0.0 };
            1.0 + t * (AIM_ICON_GROW * g[i] - AIM_ICON_SHRINK * (1.0 - g[i])) + bonus
        })
        .collect();
    (centers, widths, scales)
}

/// The slot whose slice (in the given layout) contains `angle`.
fn slice_in(angle: f64, centers: &[f64], widths: &[f64]) -> Option<usize> {
    let rel = |i: usize| wrap_angle(angle - centers[i]).abs() / (widths[i] / 2.0);
    (0..centers.len()).min_by(|&a, &b| rel(a).total_cmp(&rel(b)))
}

fn smoothstep(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
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
        // Three dots hint that the hub has a menu; they make way for the app name.
        let dots = 0.55 * (1.0 - a.wedge_alpha);
        if dots > 0.01 {
            NSColor::colorWithWhite_alpha(1.0, dots).setFill();
            for dx in [-7.0, 0.0, 7.0] {
                NSBezierPath::bezierPathWithOvalInRect(square(NSPoint::new(c.x + dx, c.y), 1.8)).fill();
            }
        }

        // The aimed app's slice of the pie.
        if a.wedge_alpha > 0.01 {
            let (mid, half) = (a.wedge_angle.to_degrees(), a.wedge_half.to_degrees());
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
            if m.drag == Some(i) {
                continue;
            }
            let ang = a.centers.get(i).copied().unwrap_or_else(|| slot_angle(i, n));
            let p = NSPoint::new(c.x + r * ang.cos(), c.y + r * ang.sin());
            let size = ICON * a.scales.get(i).copied().unwrap_or(1.0);
            app.icon.drawInRect(square(p, size / 2.0));
            if app.running {
                NSColor::colorWithWhite_alpha(1.0, 0.75).setFill();
                let dot = NSPoint::new(p.x, p.y - size / 2.0 - 4.0);
                NSBezierPath::bezierPathWithOvalInRect(square(dot, 2.0)).fill();
            }
        }

        // The app being dragged rides on the pointer, lifted a little, above everything else.
        if let Some(app) = m.drag.and_then(|i| m.apps.get(i)) {
            let p = NSPoint::new(c.x + m.drag_point.x - m.center.x, c.y + m.drag_point.y - m.center.y);
            let size = ICON * DRAG_SCALE;
            NSColor::colorWithWhite_alpha(0.0, 0.25).setFill();
            NSBezierPath::bezierPathWithOvalInRect(square(NSPoint::new(p.x, p.y - 6.0), size * 0.48)).fill();
            app.icon.drawInRect(square(p, size / 2.0));
        }

        // The aimed app's name on a soft pill, readable at any ring opacity.
        if let Some(i) = m.selected.filter(|_| a.wedge_alpha > 0.05) {
            let font = NSFont::systemFontOfSize_weight(13.0, unsafe { objc2_app_kit::NSFontWeightMedium });
            let color = NSColor::colorWithWhite_alpha(1.0, a.wedge_alpha);
            let keys = unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] };
            let vals: [&AnyObject; 2] = [&font, &color];
            let attrs = NSDictionary::from_slices(&keys, &vals);
            let name = &m.apps[i].name;
            let sz = unsafe { name.sizeWithAttributes(Some(&attrs)) };
            let (pw, ph) = (sz.width + 20.0, sz.height + 8.0);
            let pill = NSRect::new(NSPoint::new(c.x - pw / 2.0, c.y - ph / 2.0), NSSize::new(pw, ph));
            NSColor::colorWithWhite_alpha(0.0, 0.45 * a.wedge_alpha).setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(pill, ph / 2.0, ph / 2.0).fill();
            let at = NSPoint::new(c.x - sz.width / 2.0, c.y - sz.height / 2.0);
            unsafe { name.drawAtPoint_withAttributes(at, Some(&attrs)) };
        }
    });
}

/// Advances the animation one frame. Returns whether it is still moving, the window alpha, and whether
/// a fade-out just finished (so the window can be ordered out).
fn step_anim() -> (bool, f64, bool) {
    MODEL.with(|m| {
        let mut m = m.borrow_mut();
        let m = &mut *m;
        let n = m.apps.len();
        let a = &mut m.anim;
        if !m.visible && !a.closing {
            a.running = false;
            return (false, a.appear, false);
        }
        let now = Instant::now();
        let dt = (now - a.last).as_secs_f64().min(0.05);
        a.last = now;
        let ease = |rate: f64| if a.instant { 1.0 } else { 1.0 - (-dt * rate).exp() };
        // Layout follows the pointer briskly; the window fades in a touch slower and out quicker.
        let k = ease(22.0);
        let k_appear = if a.closing { ease(26.0) } else { ease(16.0) };
        let appear_target = if a.closing { 0.0 } else { 1.0 };
        a.appear += (appear_target - a.appear) * k_appear;
        let mut settled = (appear_target - a.appear).abs() < 0.003;

        // While fading out, the layout holds still.
        if !a.closing {
            let (centers, widths, scales) = aim_layout(n, m.selected, m.aim);
            for i in 0..n.min(a.centers.len()) {
                a.centers[i] += wrap_angle(centers[i] - a.centers[i]) * k;
                a.widths[i] += (widths[i] - a.widths[i]) * k;
                a.scales[i] += (scales[i] - a.scales[i]) * k;
                settled &= wrap_angle(centers[i] - a.centers[i]).abs() < 0.0005
                    && (widths[i] - a.widths[i]).abs() < 0.0005
                    && (scales[i] - a.scales[i]).abs() < 0.001;
            }
            match m.selected.filter(|&s| s < a.centers.len()) {
                Some(s) => {
                    let (angle, half) = (a.centers[s], a.widths[s] / 2.0);
                    if a.wedge_alpha < 0.02 {
                        (a.wedge_angle, a.wedge_half) = (angle, half);
                    } else {
                        a.wedge_angle += wrap_angle(angle - a.wedge_angle) * k;
                        a.wedge_half += (half - a.wedge_half) * k;
                    }
                    a.wedge_alpha += (1.0 - a.wedge_alpha) * k;
                    settled &= wrap_angle(angle - a.wedge_angle).abs() < 0.0005
                        && (half - a.wedge_half).abs() < 0.0005
                        && 1.0 - a.wedge_alpha < 0.003;
                }
                None => {
                    a.wedge_alpha -= a.wedge_alpha * k;
                    settled &= a.wedge_alpha < 0.003;
                }
            }
        }

        if !settled {
            return (true, a.appear, false);
        }
        a.running = false;
        a.appear = appear_target;
        let closed = std::mem::take(&mut a.closing);
        (false, a.appear, closed)
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
/// With `open_only`, only running apps and the user's own added apps. Hidden apps never appear.
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
    let hidden: HashSet<String> = settings::hidden_apps().into_iter().collect();
    let extras: HashSet<String> = settings::extra_apps().into_iter().filter(|p| std::path::Path::new(p).exists()).collect();
    paths.extend(settings::extra_apps().into_iter().filter(|p| extras.contains(p)));
    paths.extend(running.iter().cloned());

    let mut seen = HashSet::new();
    paths.retain(|p| {
        let running = running_set.contains(p.as_str());
        !hidden.contains(p) && !(open_only && !running && !extras.contains(p)) && seen.insert(p.clone())
    });
    // The user's saved order decides where everything goes; first-seen apps join at the end.
    let saved = settings::app_order();
    let (paths, order) = order::apply(&saved, paths);
    if order != saved {
        settings::set_app_order(&order);
    }

    let fm = NSFileManager::defaultManager();
    let mut apps = Vec::new();
    for path in paths {
        let running = running_set.contains(path.as_str());
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
        m.aim = 0.0;
        m.visible = true;
        let n = m.apps.len();
        let (centers, widths, scales) = aim_layout(n, None, 0.0);
        let a = &mut m.anim;
        a.instant = NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
        (a.appear, a.closing, a.wedge_alpha) = (0.0, false, 0.0);
        (a.centers, a.widths, a.scales) = (centers, widths, scales);
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

/// Stops taking input at once and fades the ring out (the tick orders the window out when done).
fn hide() {
    let fade = MODEL.with(|m| {
        let mut m = m.borrow_mut();
        let was_visible = m.visible;
        m.visible = false;
        // A press or drag in progress ends with the ring; releasing later does nothing.
        m.press = None;
        m.drag = None;
        m.x = Axis::default();
        m.y = Axis::default();
        m.cooldown = Instant::now() + COOLDOWN;
        m.anim.closing = was_visible;
        was_visible
    });
    if fade {
        start_anim();
    }
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
            // Hit-test against the layout as drawn, so what you see is what you click.
            let n = m.apps.len();
            let sel = if n == 0 || dist < CENTER_DEAD || dist > outer_radius(m.radius) {
                None
            } else {
                slice_in(vy.atan2(vx), &m.anim.centers, &m.anim.widths)
            };
            let aim = if sel.is_some() { smoothstep((dist - CENTER_DEAD) / (m.radius - CENTER_DEAD)) } else { 0.0 };
            if sel != m.selected || (aim - m.aim).abs() > 0.002 {
                m.selected = sel;
                m.aim = aim;
                return Act::Animate;
            }
            return Act::Nothing;
        }
        let now = Instant::now();
        let (dx, dy) = unsafe { (CGEventGetDoubleValueField(ev, FIELD_DELTA_X), CGEventGetDoubleValueField(ev, FIELD_DELTA_Y)) };
        if m.shake_checked.is_none_or(|t| now - t > SHAKE_RECHECK) {
            settings::reload();
            m.shake = shake_params(settings::shake_sensitivity());
            m.shake_checked = Some(now);
            dock::set_hidden(settings::hide_dock());
        }
        let Some(shake) = m.shake else { return Act::Nothing };
        let flips = m.x.feed(dx, now, &shake).max(m.y.feed(dy, now, &shake));
        if flips >= shake.reversals && now >= m.cooldown {
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

/// Left button down: on an app it starts a press (click or drag, decided later); elsewhere it closes the ring.
fn on_left_down(p: NSPoint) -> bool {
    let (visible, on_app) = MODEL.with(|m| {
        let mut m = m.borrow_mut();
        if !m.visible {
            return (false, false);
        }
        match m.selected {
            Some(index) => {
                m.press = Some(Press { start: p, index, dragging: false });
                (true, true)
            }
            None => {
                m.swallow_up = true;
                (true, false)
            }
        }
    });
    if visible && !on_app {
        hide();
    }
    visible
}

/// Dragging with the button down: past a few points it picks the app up and slides it to the slot under the pointer.
fn on_drag(p: NSPoint) {
    let moved = MODEL.with(|m| {
        let mut m = m.borrow_mut();
        let m = &mut *m;
        let Some(press) = m.press.as_mut() else { return false };
        if !press.dragging {
            if (p.x - press.start.x).hypot(p.y - press.start.y) < DRAG_THRESHOLD {
                return false;
            }
            press.dragging = true;
            m.drag = Some(press.index);
            m.selected = None;
            m.aim = 0.0;
        }
        m.drag_point = p;
        let n = m.apps.len();
        let (Some(from), true) = (m.drag, n > 1) else { return true };
        let step = TAU / n as f64;
        let angle = (p.y - m.center.y).atan2(p.x - m.center.x);
        let to = (((FRAC_PI_2 - angle) / step).round() as i64).rem_euclid(n as i64) as usize;
        if to != from {
            // Move the app and its animation state together, so every icon glides from where it is.
            let app = m.apps.remove(from);
            m.apps.insert(to, app);
            let a = &mut m.anim;
            for v in [&mut a.centers, &mut a.widths, &mut a.scales] {
                let x = v.remove(from);
                v.insert(to, x);
            }
            m.drag = Some(to);
        }
        true
    });
    if moved {
        start_anim();
    }
}

/// Left button up: a press that didn't move opens the app; a drag drops it and saves the new order.
fn on_left_up() -> bool {
    enum Done {
        Open(Retained<NSURL>),
        Dropped,
    }
    let done = MODEL.with(|m| {
        let mut m = m.borrow_mut();
        let press = m.press.take()?;
        if !press.dragging {
            return m.apps.get(press.index).map(|a| Done::Open(a.url.clone()));
        }
        let dragged = m.drag.take().and_then(|i| m.apps.get(i)).map(|a| a.path.clone());
        if let Some(dragged) = dragged {
            let visible: Vec<String> = m.apps.iter().map(|a| a.path.clone()).collect();
            settings::set_app_order(&order::after_move(&settings::app_order(), &visible, &dragged));
        }
        Some(Done::Dropped)
    });
    match done {
        None => false,
        Some(Done::Open(url)) => {
            hide();
            NSWorkspace::sharedWorkspace().openURL(&url);
            true
        }
        Some(Done::Dropped) => {
            start_anim();
            true
        }
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
        L_DRAG if MODEL.with(|m| m.borrow().press.is_some()) => {
            on_drag(event_point(ev, mtm));
            return null_mut();
        }
        L_UP => {
            if on_left_up() {
                return null_mut();
            }
        }
        MOVED | L_DRAG | R_DRAG | O_DRAG => on_move(ev, mtm),
        R_DOWN => {
            if on_right_click(event_point(ev, mtm)) {
                return null_mut();
            }
        }
        L_DOWN => {
            if on_left_down(event_point(ev, mtm)) {
                return null_mut();
            }
        }
        O_DOWN => {
            if on_click(false) {
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
    dock::set_hidden(settings::hide_dock());
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
                flips = axis.feed(d, now, &shake_params(3).unwrap());
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
            a.feed(d, now, &shake_params(3).unwrap());
        }
        assert_eq!((a.dir, a.leg), (1, 80.0));
    }

    #[test]
    fn sensitivity_levels() {
        assert_eq!(shake_params(0), None);
        assert_eq!(shake_params(3), Some(ShakeParams { reversals: 4, min_leg: 40.0, window: Duration::from_millis(600) }));
        // Each step up is never harder than the one before.
        for l in 1..settings::MAX_SHAKE {
            let (a, b) = (shake_params(l).unwrap(), shake_params(l + 1).unwrap());
            assert!(b.reversals <= a.reversals && b.min_leg < a.min_leg);
        }
        // A light shake that "Normal" ignores opens it at "Very light".
        let mut normal = Axis::default();
        let mut light = Axis::default();
        let now = Instant::now();
        for leg in 0..4 {
            let d = if leg % 2 == 0 { 6.0 } else { -6.0 };
            for _ in 0..4 {
                normal.feed(d, now, &shake_params(3).unwrap());
                light.feed(d, now, &shake_params(settings::MAX_SHAKE).unwrap());
            }
        }
        assert_eq!(normal.flips.len(), 0);
        assert_eq!(light.flips.len(), 3);
    }

    #[test]
    fn slots_go_clockwise_from_top() {
        assert!((slot_angle(0, 4) - FRAC_PI_2).abs() < 1e-9);
        assert!(slot_angle(1, 4).abs() < 1e-9);
    }

    #[test]
    fn even_layout_slices_split_halfway_between_icons() {
        let (c, w, s) = aim_layout(4, None, 0.0);
        let at = |deg: f64| slice_in(deg.to_radians(), &c, &w);
        assert_eq!(at(90.0), Some(0)); // top
        assert_eq!(at(0.0), Some(1)); // right
        assert_eq!(at(-90.0), Some(2)); // bottom
        assert_eq!(at(180.0), Some(3)); // left
        assert_eq!(at(46.0), Some(0));
        assert_eq!(at(44.0), Some(1));
        assert!(s.iter().all(|&x| x == 1.0));
    }

    #[test]
    fn aiming_widens_the_target_and_keeps_it_in_place() {
        let n = 9;
        let (c, w, s) = aim_layout(n, Some(2), 1.0);
        let step = TAU / n as f64;
        assert!((w.iter().sum::<f64>() - TAU).abs() < 1e-9);
        assert!(w[2] > 1.6 * step, "aimed slice should be much wider: {}", w[2] / step);
        assert!(w[6] < step, "far side should make room");
        assert!((c[2] - slot_angle(2, n)).abs() < 1e-9);
        assert!(s[2] > s[1] && s[1] > s[6] && s[6] < 1.0);
        // A point just past where the old boundary was still lands on the aimed app.
        let boundary = slot_angle(2, n) - step / 2.0;
        assert_eq!(slice_in(boundary - 0.15, &c, &w), Some(2));
        // Neighbors are symmetric around the aimed slot.
        assert!((w[1] - w[3]).abs() < 1e-9 && (wrap_angle(c[1] - c[2]) + wrap_angle(c[3] - c[2])).abs() < 1e-9);
    }

    #[test]
    fn aim_at_hub_is_even() {
        let (_, w, s) = aim_layout(6, Some(1), 0.0);
        assert!(w.iter().all(|&x| (x - TAU / 6.0).abs() < 1e-9));
        assert!((s[1] - 1.0 - SELECTED_BONUS).abs() < 1e-9 && (s[0] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn wrap_takes_the_short_way_round() {
        assert!((wrap_angle(TAU - 0.1) + 0.1).abs() < 1e-9);
        assert!((wrap_angle(-TAU + 0.1) - 0.1).abs() < 1e-9);
    }
}
