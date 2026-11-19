//! The right-click menu for an app in the ring, with the Dock's usual actions.

use std::cell::OnceCell;

use objc2::rc::Retained;
use objc2::runtime::Sel;
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSEventModifierFlags, NSMenu, NSMenuItem, NSRunningApplication, NSWorkspace};
use objc2_foundation::{ns_string, NSArray, NSObject, NSPoint, NSString, NSURL};

use crate::{dock, settings};

const FINDER: &str = "/System/Library/CoreServices/Finder.app";

thread_local! {
    static TARGET: OnceCell<Retained<Target>> = const { OnceCell::new() };
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "JiggleAppMenuTarget"]
    struct Target;

    impl Target {
        #[unsafe(method(openApp:))]
        fn open_app(&self, item: &NSMenuItem) {
            if let Some(p) = item_path(item) {
                NSWorkspace::sharedWorkspace().openURL(&file_url(&p));
            }
        }

        #[unsafe(method(keepInDock:))]
        fn keep_in_dock(&self, item: &NSMenuItem) {
            if let Some(p) = item_path(item) {
                dock::set_pinned(&p, true);
            }
        }

        #[unsafe(method(removeFromDock:))]
        fn remove_from_dock(&self, item: &NSMenuItem) {
            if let Some(p) = item_path(item) {
                dock::set_pinned(&p, false);
            }
        }

        #[unsafe(method(keepInJiggle:))]
        fn keep_in_jiggle(&self, item: &NSMenuItem) {
            if let Some(p) = item_path(item) {
                settings::set_extra_app(&p, true);
                settings::changed();
            }
        }

        #[unsafe(method(removeFromJiggle:))]
        fn remove_from_jiggle(&self, item: &NSMenuItem) {
            if let Some(p) = item_path(item) {
                settings::set_extra_app(&p, false);
                settings::changed();
            }
        }

        #[unsafe(method(neverShow:))]
        fn never_show(&self, item: &NSMenuItem) {
            if let Some(p) = item_path(item) {
                settings::set_hidden_app(&p, true);
                settings::changed();
            }
        }

        #[unsafe(method(showInFinder:))]
        fn show_in_finder(&self, item: &NSMenuItem) {
            if let Some(p) = item_path(item) {
                let urls = NSArray::from_retained_slice(&[file_url(&p)]);
                NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&urls);
            }
        }

        #[unsafe(method(hideApp:))]
        fn hide_app(&self, item: &NSMenuItem) {
            if let Some(a) = item_path(item).and_then(|p| running_app(&p)) {
                a.hide();
            }
        }

        #[unsafe(method(quitApp:))]
        fn quit_app(&self, item: &NSMenuItem) {
            if let Some(a) = item_path(item).and_then(|p| running_app(&p)) {
                a.terminate();
            }
        }

        #[unsafe(method(forceQuitApp:))]
        fn force_quit_app(&self, item: &NSMenuItem) {
            if let Some(a) = item_path(item).and_then(|p| running_app(&p)) {
                a.forceTerminate();
            }
        }
    }
);

fn target(mtm: MainThreadMarker) -> Retained<Target> {
    TARGET.with(|t| {
        t.get_or_init(|| {
            let this = Target::alloc(mtm).set_ivars(());
            unsafe { msg_send![super(this), init] }
        })
        .clone()
    })
}

fn item_path(item: &NSMenuItem) -> Option<String> {
    item.representedObject()?.downcast::<NSString>().ok().map(|s| s.to_string())
}

fn file_url(path: &str) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(path))
}

fn running_app(path: &str) -> Option<Retained<NSRunningApplication>> {
    NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .find(|a| a.bundleURL().and_then(|u| u.path()).is_some_and(|p| p.to_string() == path))
}

/// Shows the menu for the app at `path` at screen point `at`; returns once it closes.
pub fn popup(path: &str, at: NSPoint, mtm: MainThreadMarker) {
    let target = target(mtm);
    let ns_path = NSString::from_str(path);
    let menu = NSMenu::new(mtm);
    let add = |title: &str, action: Sel| {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &NSString::from_str(title), Some(action), ns_string!(""))
        };
        unsafe {
            item.setTarget(Some(&target));
            item.setRepresentedObject(Some(&ns_path));
        }
        menu.addItem(&item);
        item
    };

    let running = running_app(path);
    let is_finder = path == FINDER;
    if running.is_none() {
        add("Open", sel!(openApp:));
    }
    if !is_finder {
        if dock::pinned_paths().iter().any(|p| p == path) {
            add("Remove from Dock", sel!(removeFromDock:));
        } else {
            add("Keep in Dock", sel!(keepInDock:));
        }
    }
    if !is_finder {
        if settings::extra_apps().iter().any(|p| p == path) {
            add("Remove from Jiggle", sel!(removeFromJiggle:));
        } else {
            add("Keep in Jiggle", sel!(keepInJiggle:));
        }
    }
    add("Never Show in Jiggle", sel!(neverShow:));
    add("Show in Finder", sel!(showInFinder:));
    if running.is_some() {
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        add("Hide", sel!(hideApp:));
        if !is_finder {
            add("Quit", sel!(quitApp:));
            // Holding Option swaps Quit for Force Quit, as in the Dock.
            let force = add("Force Quit", sel!(forceQuitApp:));
            force.setKeyEquivalentModifierMask(NSEventModifierFlags::Option);
            force.setAlternate(true);
        }
    }
    menu.popUpMenuPositioningItem_atLocation_inView(None, at, None);
}
