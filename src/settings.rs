//! User settings and the Jiggle menu that changes them.
//!
//! Settings live in the `local.jiggle` defaults domain so people and AI agents can also change them from a
//! terminal (`defaults write local.jiggle RingOpacity -int 60`); the ring re-reads them each time it opens.
//! Keep the README's settings table in sync with the keys below.

use std::cell::OnceCell;
use std::ffi::c_void;

use objc2::rc::Retained;
use objc2::runtime::Sel;
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSControlStateValueOff, NSControlStateValueOn, NSImage, NSMenu, NSMenuItem, NSModalResponseOK,
    NSOpenPanel, NSStatusBar, NSStatusItem,
};
use objc2_foundation::{ns_string, NSArray, NSObject, NSPoint, NSString, NSURL, NSUserDefaults};

use crate::settings_window;
use crate::shortcut::Shortcut;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFPreferencesAppSynchronize(app: *const c_void) -> bool;
    static kCFPreferencesCurrentApplication: *const c_void;
}

const KEY_OPEN_ONLY: &str = "OpenAppsOnly";
const KEY_OPACITY: &str = "RingOpacity";
const KEY_EXTRA_APPS: &str = "ExtraApps";
const KEY_SHORTCUT: &str = "Shortcut";
const KEY_HIDDEN_APPS: &str = "HiddenApps";
const KEY_APP_ORDER: &str = "AppOrder";
const KEY_SHAKE: &str = "ShakeSensitivity";
const KEY_HIDE_DOCK: &str = "HideDock";
pub const DEFAULT_SHAKE: isize = 3;
pub const MAX_SHAKE: isize = 5;
const DEFAULT_OPACITY: isize = 80;
pub const MIN_OPACITY: isize = 0;
const OPACITIES: [isize; 6] = [0, 20, 40, 60, 80, 100];
const TAG_ALL: isize = 1;
const TAG_OPEN: isize = 2;
const TAG_OPACITY_MENU: isize = 3;

/// Picks up changes another process (e.g. `defaults write`) made since we last read.
pub fn reload() {
    unsafe { CFPreferencesAppSynchronize(kCFPreferencesCurrentApplication) };
}

pub fn open_apps_only() -> bool {
    defaults().boolForKey(&NSString::from_str(KEY_OPEN_ONLY))
}

pub fn set_open_apps_only(on: bool) {
    defaults().setBool_forKey(on, &NSString::from_str(KEY_OPEN_ONLY));
}

pub fn hide_dock() -> bool {
    defaults().boolForKey(&NSString::from_str(KEY_HIDE_DOCK))
}

pub fn set_hide_dock(on: bool) {
    defaults().setBool_forKey(on, &NSString::from_str(KEY_HIDE_DOCK));
    crate::dock::set_hidden(on);
}

/// Opacity of the ring's background, 0.0 (icons float on their own) to 1.0.
pub fn ring_opacity() -> f64 {
    let key = NSString::from_str(KEY_OPACITY);
    let v = match defaults().objectForKey(&key) {
        None => DEFAULT_OPACITY,
        Some(_) => defaults().integerForKey(&key).clamp(MIN_OPACITY, 100),
    };
    v as f64 / 100.0
}

pub fn set_opacity_percent(v: isize) {
    defaults().setInteger_forKey(v.clamp(MIN_OPACITY, 100), &NSString::from_str(KEY_OPACITY));
}

fn read_list(key: &str) -> Vec<String> {
    defaults()
        .stringArrayForKey(&NSString::from_str(key))
        .map(|a| a.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default()
}

fn write_list(key: &str, apps: &[String]) {
    let strings: Vec<Retained<NSString>> = apps.iter().map(|p| NSString::from_str(p)).collect();
    let array = NSArray::from_retained_slice(&strings);
    unsafe { defaults().setObject_forKey(Some(&array), &NSString::from_str(key)) };
}

/// Adds (`on`) or removes `path` from the list stored under `key`.
fn set_in_list(key: &str, path: &str, on: bool) {
    let mut apps = read_list(key);
    apps.retain(|p| p != path);
    if on {
        apps.push(path.to_string());
    }
    write_list(key, &apps);
}

/// How easily a mouse shake opens the ring: 0 = off, 1 (firm) to 5 (light); 3 when unset.
pub fn shake_sensitivity() -> isize {
    let key = NSString::from_str(KEY_SHAKE);
    if defaults().objectForKey(&key).is_none() {
        return DEFAULT_SHAKE;
    }
    defaults().integerForKey(&key).clamp(0, MAX_SHAKE)
}

pub fn set_shake_sensitivity(level: isize) {
    defaults().setInteger_forKey(level.clamp(0, MAX_SHAKE), &NSString::from_str(KEY_SHAKE));
}

/// The ring's app order (every app ever shown, in order); see `order.rs`.
pub fn app_order() -> Vec<String> {
    read_list(KEY_APP_ORDER)
}

pub fn set_app_order(order: &[String]) {
    write_list(KEY_APP_ORDER, order);
}

/// Apps the user added to the ring themselves; shown even when closed and not in the Dock.
pub fn extra_apps() -> Vec<String> {
    read_list(KEY_EXTRA_APPS)
}

/// Adds (`keep`) or removes an app from the user's own list. Adding one also un-hides it.
pub fn set_extra_app(path: &str, keep: bool) {
    set_in_list(KEY_EXTRA_APPS, path, keep);
    if keep {
        set_in_list(KEY_HIDDEN_APPS, path, false);
    }
}

/// Apps that never appear in the ring, whether open, closed, in the Dock or added.
pub fn hidden_apps() -> Vec<String> {
    read_list(KEY_HIDDEN_APPS)
}

/// Hides an app from the ring for good (`hide`), or lets it appear again. Hiding one also un-adds it.
pub fn set_hidden_app(path: &str, hide: bool) {
    set_in_list(KEY_HIDDEN_APPS, path, hide);
    if hide {
        set_in_list(KEY_EXTRA_APPS, path, false);
    }
}

/// The system-wide shortcut that opens the ring; None (the default) means off.
pub fn shortcut() -> Option<Shortcut> {
    let text = defaults().stringForKey(&NSString::from_str(KEY_SHORTCUT))?;
    Shortcut::parse(&text.to_string())
}

pub fn set_shortcut(shortcut: Option<Shortcut>) {
    let key = NSString::from_str(KEY_SHORTCUT);
    match shortcut {
        Some(s) => unsafe { defaults().setObject_forKey(Some(&NSString::from_str(&s.to_setting())), &key) },
        None => defaults().removeObjectForKey(&key),
    }
}

pub fn opacity_percent() -> isize {
    (ring_opacity() * 100.0).round() as isize
}

fn defaults() -> Retained<NSUserDefaults> {
    NSUserDefaults::standardUserDefaults()
}

struct Menu {
    _item: Retained<NSStatusItem>,
    menu: Retained<NSMenu>,
    _target: Retained<Target>,
}

thread_local! {
    static MENU: OnceCell<Menu> = const { OnceCell::new() };
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "JiggleMenuTarget"]
    struct Target;

    impl Target {
        #[unsafe(method(pickMode:))]
        fn pick_mode(&self, item: &NSMenuItem) {
            set_open_apps_only(item.tag() == TAG_OPEN);
            changed();
        }

        #[unsafe(method(pickOpacity:))]
        fn pick_opacity(&self, item: &NSMenuItem) {
            set_opacity_percent(item.tag());
            changed();
        }

        #[unsafe(method(addApps:))]
        fn add_apps(&self, _item: &NSMenuItem) {
            add_apps(MainThreadMarker::from(self));
            changed();
        }

        #[unsafe(method(openSettings:))]
        fn open_settings(&self, _item: &NSMenuItem) {
            settings_window::show(MainThreadMarker::from(self));
        }

        // NSMenuDelegate: refresh checkmarks in case settings changed from elsewhere.
        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, _menu: &NSMenu) {
            reload();
            sync();
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _item: &NSMenuItem) {
            quit();
        }
    }
);

impl Target {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

/// Puts the Jiggle icon in the menu bar with the settings menu.
pub fn install_menu(mtm: MainThreadMarker) {
    let target = Target::new(mtm);
    let add = |menu: &NSMenu, title: &str, action: Option<Sel>, tag: isize| {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &NSString::from_str(title), action, ns_string!(""))
        };
        if action.is_some() {
            unsafe { item.setTarget(Some(&target)) };
        }
        item.setTag(tag);
        menu.addItem(&item);
        item
    };

    let menu = NSMenu::new(mtm);
    add(&menu, "Show all Dock apps", Some(sel!(pickMode:)), TAG_ALL);
    add(&menu, "Show only open apps", Some(sel!(pickMode:)), TAG_OPEN);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    let opacity = NSMenu::new(mtm);
    for v in OPACITIES {
        add(&opacity, &format!("{v}%"), Some(sel!(pickOpacity:)), v);
    }
    add(&menu, "Ring opacity", None, TAG_OPACITY_MENU).setSubmenu(Some(&opacity));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    add(&menu, "Add App to Jiggle\u{2026}", Some(sel!(addApps:)), 0);
    add(&menu, "Settings\u{2026}", Some(sel!(openSettings:)), 0);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    add(&menu, "Quit Jiggle", Some(sel!(quit:)), 0);

    let item = NSStatusBar::systemStatusBar().statusItemWithLength(-1.0); // NSVariableStatusItemLength
    if let Some(button) = item.button(mtm) {
        match NSImage::imageWithSystemSymbolName_accessibilityDescription(ns_string!("circle.circle"), Some(ns_string!("Jiggle"))) {
            Some(img) => {
                img.setTemplate(true);
                button.setImage(Some(&img));
            }
            None => button.setTitle(ns_string!("J")),
        }
    }
    item.setMenu(Some(&menu));
    let _: () = unsafe { msg_send![&*menu, setDelegate: &*target] };
    MENU.with(|m| {
        let _ = m.set(Menu { _item: item, menu, _target: target });
    });
    sync();
}

/// Shows the Jiggle menu at a screen point (right-click on the ring's center); returns once it closes.
pub fn popup_menu(at: NSPoint) {
    let menu = MENU.with(|m| m.get().map(|m| m.menu.clone()));
    if let Some(menu) = menu {
        menu.popUpMenuPositioningItem_atLocation_inView(None, at, None);
    }
}

/// Call after any setting changes so the menu and the Settings window show it.
pub fn changed() {
    sync();
    settings_window::refresh();
}

/// Lets the user pick apps to keep in the ring.
pub fn add_apps(mtm: MainThreadMarker) {
    for path in pick_apps(mtm, "Add to Jiggle", "Choose apps to keep in Jiggle's ring") {
        set_extra_app(&path, true);
    }
}

/// Lets the user pick apps that should never appear in the ring.
pub fn hide_apps(mtm: MainThreadMarker) {
    for path in pick_apps(mtm, "Hide", "Choose apps Jiggle should never show") {
        set_hidden_app(&path, true);
    }
}

/// An open panel in /Applications; returns the chosen .app paths.
fn pick_apps(mtm: MainThreadMarker, prompt: &str, message: &str) -> Vec<String> {
    // An accessory app has to come forward for its panel to show on top.
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(true);
    panel.setDirectoryURL(Some(&NSURL::fileURLWithPath(ns_string!("/Applications"))));
    panel.setPrompt(Some(&NSString::from_str(prompt)));
    panel.setMessage(Some(&NSString::from_str(message)));
    if panel.runModal() != NSModalResponseOK {
        return Vec::new();
    }
    panel.URLs().iter().filter_map(|u| u.path().map(|p| p.to_string())).filter(|p| p.ends_with(".app")).collect()
}

/// Puts the checkmarks next to the current settings.
fn sync() {
    MENU.with(|m| {
        let Some(m) = m.get() else { return };
        let open_only = open_apps_only();
        let state = |on: bool| if on { NSControlStateValueOn } else { NSControlStateValueOff };
        if let Some(i) = m.menu.itemWithTag(TAG_ALL) {
            i.setState(state(!open_only));
        }
        if let Some(i) = m.menu.itemWithTag(TAG_OPEN) {
            i.setState(state(open_only));
        }
        let current = opacity_percent();
        if let Some(sub) = m.menu.itemWithTag(TAG_OPACITY_MENU).and_then(|i| i.submenu()) {
            for v in OPACITIES {
                if let Some(i) = sub.itemWithTag(v) {
                    i.setState(state(v == current));
                }
            }
        }
    });
}

/// Under the LaunchAgent (KeepAlive), unload the job so launchd doesn't relaunch us; it loads again at next login.
/// A hidden Dock comes back while Jiggle isn't running, so nobody is left without either.
fn quit() -> ! {
    crate::dock::set_hidden(false);
    if std::env::var("XPC_SERVICE_NAME").as_deref() == Ok("local.jiggle") {
        let _ = std::process::Command::new("/bin/launchctl").args(["remove", "local.jiggle"]).status();
    }
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    // One test: the cases share the same defaults domain, so they can't run in parallel.
    fn app_lists() {
        let (extra, hidden) = (NSString::from_str(KEY_EXTRA_APPS), NSString::from_str(KEY_HIDDEN_APPS));
        set_extra_app("/Applications/C.app", true);
        set_hidden_app("/Applications/C.app", true);
        assert_eq!(hidden_apps(), ["/Applications/C.app"]);
        assert!(!extra_apps().iter().any(|p| p == "/Applications/C.app"));
        set_extra_app("/Applications/C.app", true);
        assert!(hidden_apps().is_empty());
        defaults().removeObjectForKey(&extra);
        defaults().removeObjectForKey(&hidden);

        // Order is kept and re-adding moves an app to the end.
        let key = extra;
        set_extra_app("/Applications/A.app", true);
        set_extra_app("/Applications/B.app", true);
        set_extra_app("/Applications/A.app", true);
        assert_eq!(extra_apps(), ["/Applications/B.app", "/Applications/A.app"]);
        set_extra_app("/Applications/B.app", false);
        assert_eq!(extra_apps(), ["/Applications/A.app"]);
        defaults().removeObjectForKey(&key);
        assert!(extra_apps().is_empty());

        // 0% is a real choice, not "unset".
        let key = NSString::from_str(KEY_OPACITY);
        assert_eq!(opacity_percent(), DEFAULT_OPACITY);
        set_opacity_percent(0);
        assert_eq!(opacity_percent(), 0);
        defaults().removeObjectForKey(&key);
    }
}
