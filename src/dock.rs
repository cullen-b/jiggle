//! Reads and edits the Dock's pinned apps (com.apple.dock "persistent-apps"), like dockutil does.

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{ns_string, NSArray, NSDictionary, NSNumber, NSString, NSURL, NSUserDefaults};

const URL_KEY_PATH: &str = "tile-data.file-data._CFURLString";

fn dock_domain() -> Option<Retained<NSDictionary<NSString, AnyObject>>> {
    NSUserDefaults::standardUserDefaults().persistentDomainForName(ns_string!("com.apple.dock"))
}

fn tile_path(url: &NSString) -> String {
    match NSURL::URLWithString(url).filter(|u| u.isFileURL()).and_then(|u| u.path()) {
        Some(p) => p.to_string(),
        None => url.to_string(),
    }
}

fn path_of_tile(tile: &AnyObject) -> Option<String> {
    let url: Option<Retained<AnyObject>> = unsafe { msg_send![tile, valueForKeyPath: &*NSString::from_str(URL_KEY_PATH)] };
    url.and_then(|u| u.downcast::<NSString>().ok()).map(|s| tile_path(&s))
}

/// Paths of the apps pinned in the Dock, in Dock order.
pub fn pinned_paths() -> Vec<String> {
    let Some(dock) = dock_domain() else { return Vec::new() };
    let apps: Option<Retained<NSArray>> = unsafe { msg_send![&*dock, objectForKey: ns_string!("persistent-apps")] };
    apps.iter().flat_map(|a| a.iter()).filter_map(|t| path_of_tile(&t)).collect()
}

/// Pins (`keep`) or unpins the app at `path`, then restarts the Dock so it shows the change.
pub fn set_pinned(path: &str, keep: bool) {
    let Some(dock) = dock_domain() else { return };
    let domain: Retained<AnyObject> = unsafe { msg_send![&*dock, mutableCopy] };
    let apps: Option<Retained<AnyObject>> = unsafe { msg_send![&*domain, objectForKey: ns_string!("persistent-apps")] };
    let list: Retained<AnyObject> = match apps {
        Some(a) => unsafe { msg_send![&*a, mutableCopy] },
        None => unsafe { msg_send![objc2::class!(NSMutableArray), array] },
    };
    let count: usize = unsafe { msg_send![&*list, count] };
    let mut found = false;
    for i in (0..count).rev() {
        let tile: Retained<AnyObject> = unsafe { msg_send![&*list, objectAtIndex: i] };
        if path_of_tile(&tile).as_deref() == Some(path) {
            found = true;
            if !keep {
                let _: () = unsafe { msg_send![&*list, removeObjectAtIndex: i] };
            }
        }
    }
    if keep == found {
        return;
    }
    if keep {
        let tile = new_tile(path);
        let _: () = unsafe { msg_send![&*list, addObject: &*tile] };
    }
    unsafe {
        let _: () = msg_send![&*domain, setObject: &*list, forKey: ns_string!("persistent-apps")];
        let defaults = NSUserDefaults::standardUserDefaults();
        let _: () = msg_send![&*defaults, setPersistentDomain: &*domain, forName: ns_string!("com.apple.dock")];
    }
    let _ = std::process::Command::new("/usr/bin/killall").arg("Dock").status();
}

/// A minimal Dock tile; the Dock fills in the rest when it loads it.
fn new_tile(path: &str) -> Retained<NSDictionary<NSString, AnyObject>> {
    let url = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(path), true);
    let url_string = url.absoluteString().unwrap_or_else(|| NSString::from_str(path));
    let kind = NSNumber::new_i32(15); // _CFURLStringType 15: the string is a file:// URL
    let file_data_vals: [&AnyObject; 2] = [&url_string, &kind];
    let file_data = NSDictionary::from_slices(&[ns_string!("_CFURLString"), ns_string!("_CFURLStringType")], &file_data_vals);
    let tile_data_vals: [&AnyObject; 1] = [&file_data];
    let tile_data = NSDictionary::from_slices(&[ns_string!("file-data")], &tile_data_vals);
    let tile_vals: [&AnyObject; 2] = [&tile_data, ns_string!("file-tile")];
    NSDictionary::from_slices(&[ns_string!("tile-data"), ns_string!("tile-type")], &tile_vals)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tile_reads_back_as_its_path() {
        let tile = new_tile("/Applications/Safari.app");
        assert_eq!(path_of_tile(&tile).as_deref(), Some("/Applications/Safari.app"));
    }

    #[test]
    fn reads_this_machines_dock() {
        // Read-only: every pinned entry should parse to an absolute path.
        let paths = pinned_paths();
        assert!(paths.iter().all(|p| p.starts_with('/')), "{paths:?}");
        eprintln!("{} pinned apps, first: {:?}", paths.len(), paths.first());
    }
}
