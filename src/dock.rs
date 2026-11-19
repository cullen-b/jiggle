//! Reads and edits the Dock's pinned apps (com.apple.dock "persistent-apps"), like dockutil does,
//! and can hide the Dock entirely.

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

/// The Dock's own auto-hide keys, saved in our settings while Jiggle hides the Dock.
const HIDE_KEYS: [&str; 2] = ["autohide", "autohide-delay"];
const SAVED_KEY: &str = "DockBeforeHiding";
/// Auto-hide with a delay this long (seconds) keeps the Dock out of sight, even when the pointer reaches the edge.
const HIDE_DELAY: &str = "1000";

/// Hides the Dock (`hide`) or puts the user's own auto-hide settings back. Does nothing if it's already that way.
pub fn set_hidden(hide: bool) {
    let ours = NSUserDefaults::standardUserDefaults();
    let saved_key = NSString::from_str(SAVED_KEY);
    let saved = ours.dictionaryForKey(&saved_key);
    if hide == saved.is_some() {
        return;
    }
    if hide {
        let dock = dock_domain();
        let (mut keys, mut vals) = (Vec::new(), Vec::new());
        for k in HIDE_KEYS {
            let key = NSString::from_str(k);
            let v: Option<Retained<AnyObject>> = dock.as_ref().and_then(|d| unsafe { msg_send![&**d, objectForKey: &*key] });
            if let Some(v) = v {
                keys.push(key);
                vals.push(v);
            }
        }
        let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
        let val_refs: Vec<&AnyObject> = vals.iter().map(|v| &**v).collect();
        let before = NSDictionary::from_slices(&key_refs, &val_refs);
        unsafe { ours.setObject_forKey(Some(&before), &saved_key) };
        dock_defaults(&["write", "com.apple.dock", "autohide", "-bool", "true"]);
        dock_defaults(&["write", "com.apple.dock", "autohide-delay", "-float", HIDE_DELAY]);
    } else {
        let saved = saved.expect("checked above");
        for k in HIDE_KEYS {
            let v: Option<Retained<AnyObject>> = unsafe { msg_send![&*saved, objectForKey: &*NSString::from_str(k)] };
            let text = v.map(|v| unsafe { msg_send![&*v, description] }).map(|d: Retained<NSString>| d.to_string());
            match (k, text) {
                ("autohide", Some(t)) => dock_defaults(&["write", "com.apple.dock", k, "-bool", if t == "0" { "false" } else { "true" }]),
                (_, Some(t)) => dock_defaults(&["write", "com.apple.dock", k, "-float", &t]),
                (_, None) => dock_defaults(&["delete", "com.apple.dock", k]),
            }
        }
        ours.removeObjectForKey(&saved_key);
    }
    let _ = std::process::Command::new("/usr/bin/killall").arg("Dock").status();
}

fn dock_defaults(args: &[&str]) {
    let _ = std::process::Command::new("/usr/bin/defaults").args(args).stderr(std::process::Stdio::null()).status();
}
