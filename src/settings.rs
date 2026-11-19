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
const DEFAULT_OPACITY: isize = 80;
pub const MIN_OPACITY: isize = 10;
const OPACITIES: [isize; 5] = [20, 40, 60, 80, 100];
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

/// Opacity of the ring's background, 0.1 to 1.0.
pub fn ring_opacity() -> f64 {
    let v = defaults().integerForKey(&NSString::from_str(KEY_OPACITY));
    let v = if v <= 0 { DEFAULT_OPACITY } else { v.clamp(MIN_OPACITY, 100) };
    v as f64 / 100.0
}

pub fn set_opacity_percent(v: isize) {
    defaults().setInteger_forKey(v.clamp(MIN_OPACITY, 100), &NSString::from_str(KEY_OPACITY));
}

/// Apps the user added to the ring themselves; shown even when closed and not in the Dock.
pub fn extra_apps() -> Vec<String> {
    defaults()
        .stringArrayForKey(&NSString::from_str(KEY_EXTRA_APPS))
        .map(|a| a.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default()
}

/// Adds (`keep`) or removes an app from the user's own list.
pub fn set_extra_app(path: &str, keep: bool) {
    let mut apps = extra_apps();
    apps.retain(|p| p != path);
    if keep {
        apps.push(path.to_string());
    }
    let strings: Vec<Retained<NSString>> = apps.iter().map(|p| NSString::from_str(p)).collect();
    let array = NSArray::from_retained_slice(&strings);
    unsafe { defaults().setObject_forKey(Some(&array), &NSString::from_str(KEY_EXTRA_APPS)) };
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
    // An accessory app has to come forward for its panel to show on top.
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(true);
    panel.setDirectoryURL(Some(&NSURL::fileURLWithPath(ns_string!("/Applications"))));
    panel.setPrompt(Some(ns_string!("Add to Jiggle")));
    panel.setMessage(Some(ns_string!("Choose apps to keep in Jiggle's ring")));
    if panel.runModal() != NSModalResponseOK {
        return;
    }
    for url in panel.URLs().iter() {
        if let Some(path) = url.path().map(|p| p.to_string()).filter(|p| p.ends_with(".app")) {
            set_extra_app(&path, true);
        }
    }
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
fn quit() -> ! {
    if std::env::var("XPC_SERVICE_NAME").as_deref() == Ok("local.jiggle") {
        let _ = std::process::Command::new("/bin/launchctl").args(["remove", "local.jiggle"]).status();
    }
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_apps_round_trip() {
        let key = NSString::from_str(KEY_EXTRA_APPS);
        set_extra_app("/Applications/A.app", true);
        set_extra_app("/Applications/B.app", true);
        set_extra_app("/Applications/A.app", true);
        assert_eq!(extra_apps(), ["/Applications/B.app", "/Applications/A.app"]);
        set_extra_app("/Applications/B.app", false);
        assert_eq!(extra_apps(), ["/Applications/A.app"]);
        defaults().removeObjectForKey(&key);
        assert!(extra_apps().is_empty());
    }
}
