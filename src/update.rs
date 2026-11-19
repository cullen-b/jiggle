//! Self-update from GitHub releases. Shells out to curl, ditto and codesign to keep the binary small.
//! A release is installed only if it is newer and signed by the pinned jiggle certificate.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const REPO: &str = "cullen-b/jiggle";
const REQUIREMENT: &str = include_str!("../scripts/requirement.txt");
const FIRST_CHECK: Duration = Duration::from_secs(20);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// Starts the background update loop when running from an installed .app bundle.
pub fn spawn() {
    let Some(app) = installed_app() else { return };
    std::thread::spawn(move || {
        std::thread::sleep(FIRST_CHECK);
        loop {
            check_once(&app);
            std::thread::sleep(CHECK_EVERY);
        }
    });
}

/// One blocking check, for when the app is stuck waiting for permission and the loop never gets going.
pub fn check_now() {
    if let Some(app) = installed_app() {
        check_once(&app);
    }
}

fn installed_app() -> Option<PathBuf> {
    if std::env::var_os("JIGGLE_NO_UPDATE").is_some() {
        return None;
    }
    app_bundle()
}

fn check_once(app: &Path) {
    match check_and_install(app) {
        Ok(Some(tag)) => {
            eprintln!("jiggle: updated to {tag}, restarting");
            restart(app);
        }
        Ok(None) => {}
        Err(e) => eprintln!("jiggle: update check failed: {e}"),
    }
}

fn app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let app = exe.parent()?.parent()?.parent()?;
    (app.extension()? == "app").then(|| app.to_path_buf())
}

fn check_and_install(app: &Path) -> io::Result<Option<String>> {
    let Some(tag) = latest_tag()? else { return Ok(None) };
    let (Some(latest), Some(current)) = (parse_version(&tag), parse_version(env!("CARGO_PKG_VERSION"))) else {
        return Ok(None);
    };
    if latest <= current {
        return Ok(None);
    }
    install(&tag, app)?;
    Ok(Some(tag))
}

/// The tag `releases/latest` redirects to, e.g. "v0.3.0"; None when there is no release yet.
fn latest_tag() -> io::Result<Option<String>> {
    let url = format!("https://github.com/{REPO}/releases/latest");
    let out = run_output("/usr/bin/curl", &["-fsSI", "--max-time", "30", "-o", "/dev/null", "-w", "%{redirect_url}", &url])?;
    Ok(out.rsplit_once("/tag/").map(|(_, t)| t.trim().to_string()).filter(|t| !t.is_empty()))
}

fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let mut it = s.trim_start_matches('v').split('.').map(|p| p.parse::<u64>().ok());
    Some((it.next()??, it.next()??, it.next()??))
}

fn install(tag: &str, app: &Path) -> io::Result<()> {
    // Stage next to the app so the final swap is a same-volume rename.
    let work = app.with_file_name(".jiggle-update");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let result = (|| {
        let zip = work.join("Jiggle.zip");
        let url = format!("https://github.com/{REPO}/releases/download/{tag}/Jiggle.zip");
        run("/usr/bin/curl", &["-fsSL", "--max-time", "300", "-o", path_str(&zip)?, &url])?;
        run("/usr/bin/ditto", &["-x", "-k", path_str(&zip)?, path_str(&work)?])?;
        let new = work.join("Jiggle.app");
        let requirement = format!("-R={}", REQUIREMENT.trim());
        run("/usr/bin/codesign", &["--verify", "--deep", "--strict", &requirement, path_str(&new)?])?;
        let old = work.join("previous.app");
        std::fs::rename(app, &old)?;
        if let Err(e) = std::fs::rename(&new, app) {
            std::fs::rename(&old, app)?;
            return Err(e);
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// Under the LaunchAgent, exiting is enough (KeepAlive relaunches the new build); otherwise reopen it.
fn restart(app: &Path) -> ! {
    if std::env::var("XPC_SERVICE_NAME").as_deref() != Ok("local.jiggle") {
        let _ = Command::new("/usr/bin/open").arg("-n").arg(app).spawn();
    }
    std::process::exit(0);
}

fn path_str(p: &Path) -> io::Result<&str> {
    p.to_str().ok_or_else(|| io::Error::other("non-UTF-8 path"))
}

fn run(cmd: &str, args: &[&str]) -> io::Result<()> {
    run_output(cmd, args).map(|_| ())
}

fn run_output(cmd: &str, args: &[&str]) -> io::Result<String> {
    let out = Command::new(cmd).args(args).output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(io::Error::other(format!("{cmd} failed: {}", err.trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(parse_version("v0.10.0"), Some((0, 10, 0)));
        assert!(parse_version("v0.10.0") > parse_version("0.9.9"));
        assert_eq!(parse_version("v1.2"), None);
        assert_eq!(parse_version("nightly"), None);
    }

    #[test]
    fn requirement_pins_identifier_and_certificate() {
        assert!(REQUIREMENT.contains(r#"identifier "local.jiggle""#));
        assert!(REQUIREMENT.contains("certificate leaf = H\""));
    }
}
