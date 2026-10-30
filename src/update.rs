//! Looks on GitHub once at start whether a newer Solarum is out.
//!
//! No HTTP library: macOS and Windows 10/11 both ship `curl`, so a background
//! thread asks the GitHub API with it and reads the tag of the latest release.
//! Any failure (offline, no curl, rate limit) simply means "no news".

use std::sync::mpsc::{channel, Receiver};

pub const REPO_URL: &str = "https://github.com/tj537/solarum";
/// The tip jar, linked from the settings and the website
pub const KOFI_URL: &str = "https://ko-fi.com/tj537";
pub const RELEASES_URL: &str = "https://github.com/tj537/solarum/releases/latest";
const API_URL: &str = "https://api.github.com/repos/tj537/solarum/releases/latest";

/// A version newer than this one, e.g. "0.3.0"
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
}

/// Starts the check in the background; the receiver gets one answer (or none)
pub fn check(wake: impl Fn() + Send + 'static) -> Receiver<Release> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        if let Some(tag) = fetch().as_deref().and_then(latest_tag) {
            if is_newer(&tag, env!("CARGO_PKG_VERSION")) {
                let _ = tx.send(Release {
                    version: tag.trim_start_matches('v').to_string(),
                });
                wake();
            }
        }
    });
    rx
}

/// curl where the system keeps it – never one that happens to come first on the search path
fn curl() -> std::path::PathBuf {
    #[cfg(windows)]
    {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        std::path::Path::new(&root).join("System32").join("curl.exe")
    }
    #[cfg(not(windows))]
    {
        std::path::PathBuf::from("/usr/bin/curl")
    }
}

fn fetch() -> Option<String> {
    let mut cmd = std::process::Command::new(curl());
    cmd.args(["-sSfL", "--max-time", "8", "-H", "Accept: application/vnd.github+json", API_URL]);
    #[cfg(windows)]
    {
        // Don't flash a console window
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The `tag_name` of a GitHub release JSON
pub fn latest_tag(json: &str) -> Option<String> {
    let rest = &json[json.find("\"tag_name\"")? + 10..];
    let rest = &rest[rest.find('"')? + 1..];
    Some(rest[..rest.find('"')?].to_string())
}

fn parse(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.trim().trim_start_matches('v');
    // Ignore pre-release suffixes like "-beta.1"
    let v = v.split(['-', '+']).next()?;
    let mut it = v.split('.').map(|p| p.parse::<u32>().ok());
    Some((it.next()??, it.next().flatten().unwrap_or(0), it.next().flatten().unwrap_or(0)))
}

/// Is release `tag` newer than `current`?
pub fn is_newer(tag: &str, current: &str) -> bool {
    match (parse(tag), parse(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_tag() {
        let json = r#"{"url":"x","tag_name": "v0.3.1","name":"Solarum 0.3.1"}"#;
        assert_eq!(latest_tag(json).as_deref(), Some("v0.3.1"));
        assert_eq!(latest_tag("{}"), None);
        assert_eq!(latest_tag(r#"{"tag_name": "#), None);
    }

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.3.0", "0.2.0"));
        assert!(is_newer("v0.2.1", "0.2.0"));
        assert!(is_newer("v1.0", "0.9.9"));
        assert!(!is_newer("v0.2.0", "0.2.0"));
        assert!(!is_newer("v0.1.9", "0.2.0"));
        assert!(!is_newer("nightly", "0.2.0"));
        assert!(!is_newer("v0.2.0-beta.1", "0.2.0"));
    }
}
