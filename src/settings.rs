//! User settings (kept in NSUserDefaults) and the menu bar menu that changes them.

use std::cell::OnceCell;

use objc2::rc::Retained;
use objc2::runtime::Sel;
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSControlStateValueOff, NSControlStateValueOn, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
};
use objc2_foundation::{ns_string, NSObject, NSString, NSUserDefaults};

const KEY_OPEN_ONLY: &str = "OpenAppsOnly";
const KEY_OPACITY: &str = "RingOpacity";
const DEFAULT_OPACITY: isize = 80;
const OPACITIES: [isize; 5] = [20, 40, 60, 80, 100];
const TAG_ALL: isize = 1;
const TAG_OPEN: isize = 2;
const TAG_OPACITY_MENU: isize = 3;

pub fn open_apps_only() -> bool {
    defaults().boolForKey(&NSString::from_str(KEY_OPEN_ONLY))
}

/// Opacity of the ring's background, 0.2 to 1.0.
pub fn ring_opacity() -> f64 {
    let v = defaults().integerForKey(&NSString::from_str(KEY_OPACITY));
    let v = if v <= 0 { DEFAULT_OPACITY } else { v.min(100) };
    v as f64 / 100.0
}

fn opacity_percent() -> isize {
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
            defaults().setBool_forKey(item.tag() == TAG_OPEN, &NSString::from_str(KEY_OPEN_ONLY));
            sync();
        }

        #[unsafe(method(pickOpacity:))]
        fn pick_opacity(&self, item: &NSMenuItem) {
            defaults().setInteger_forKey(item.tag(), &NSString::from_str(KEY_OPACITY));
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
    MENU.with(|m| {
        let _ = m.set(Menu { _item: item, menu, _target: target });
    });
    sync();
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
