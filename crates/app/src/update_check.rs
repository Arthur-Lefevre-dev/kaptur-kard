//! In-app update check against GitHub Releases (non-blocking).

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

const RELEASES_LATEST: &str =
    "https://api.github.com/repos/Arthur-Lefevre-dev/kaptur-kard/releases/latest";
const RELEASES_PAGE: &str = "https://github.com/Arthur-Lefevre-dev/kaptur-kard/releases";
const USER_AGENT: &str = "KapturKard-CaptureView-UpdateCheck";

#[derive(Clone, Debug, Default)]
pub struct UpdateStatus {
    pub checking: bool,
    pub latest_tag: String,
    pub html_url: String,
    pub body: String,
    pub newer: bool,
    pub error: String,
    pub checked_at: Option<Instant>,
}

#[derive(Debug)]
enum UpdateMsg {
    Ok {
        tag: String,
        html_url: String,
        body: String,
        newer: bool,
    },
    Err(String),
}

pub struct UpdateChecker {
    rx: Option<Receiver<UpdateMsg>>,
    pub status: UpdateStatus,
    last_auto: Option<Instant>,
}

impl Default for UpdateChecker {
    fn default() -> Self {
        Self {
            rx: None,
            status: UpdateStatus::default(),
            last_auto: None,
        }
    }
}

impl UpdateChecker {
    pub fn poll(&mut self) {
        let Some(rx) = self.rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(UpdateMsg::Ok {
                tag,
                html_url,
                body,
                newer,
            }) => {
                self.status.checking = false;
                self.status.latest_tag = tag;
                self.status.html_url = html_url;
                self.status.body = body;
                self.status.newer = newer;
                self.status.error.clear();
                self.status.checked_at = Some(Instant::now());
                self.rx = None;
            }
            Ok(UpdateMsg::Err(e)) => {
                self.status.checking = false;
                self.status.error = e;
                self.status.checked_at = Some(Instant::now());
                self.rx = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.status.checking = false;
                self.rx = None;
            }
        }
    }

    /// Fire a background check at most once unless `force`.
    pub fn request_check(&mut self, force: bool) {
        self.poll();
        if self.status.checking {
            return;
        }
        if !force {
            if let Some(at) = self.last_auto {
                if at.elapsed() < Duration::from_secs(6 * 3600) {
                    return;
                }
            }
        }
        self.last_auto = Some(Instant::now());
        self.status.checking = true;
        self.status.error.clear();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let current = env!("CARGO_PKG_VERSION").to_string();
        let _ = thread::Builder::new()
            .name("update-check".into())
            .spawn(move || {
                let result = fetch_latest(&current);
                let _ = tx.send(result);
            });
    }

    pub fn open_releases_page() {
        let url = RELEASES_PAGE;
        #[cfg(windows)]
        {
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", "", url])
                .spawn();
        }
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg(url).spawn();
        }
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("xdg-open").arg(url).spawn();
        }
    }

    pub fn open_latest(&self) {
        let url = if self.status.html_url.is_empty() {
            RELEASES_PAGE.to_string()
        } else {
            self.status.html_url.clone()
        };
        #[cfg(windows)]
        {
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", "", &url])
                .spawn();
        }
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg(&url).spawn();
        }
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
        }
    }
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: String,
}

fn fetch_latest(current: &str) -> UpdateMsg {
    let agent = match ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(8))
        .build()
        .get(RELEASES_LATEST)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .call()
    {
        Ok(r) => r,
        Err(e) => return UpdateMsg::Err(format!("network: {e}")),
    };
    let release: GhRelease = match agent.into_json() {
        Ok(r) => r,
        Err(e) => return UpdateMsg::Err(format!("parse: {e}")),
    };
    let tag = release.tag_name.trim().trim_start_matches('v').to_string();
    let newer = is_newer(&tag, current);
    UpdateMsg::Ok {
        tag: release.tag_name,
        html_url: release.html_url,
        body: release.body.chars().take(800).collect(),
        newer,
    }
}

/// Compare dotted semver-ish tags (major.minor.patch). Non-numeric → not newer.
pub fn is_newer(remote: &str, local: &str) -> bool {
    let r = parse_ver(remote);
    let l = parse_ver(local);
    r > l
}

fn parse_ver(s: &str) -> (u32, u32, u32) {
    let s = s.trim().trim_start_matches('v');
    let mut parts = s.split('.').filter_map(|p| {
        let digits: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse::<u32>().ok()
    });
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_compare() {
        assert!(is_newer("0.3.0", "0.2.3"));
        assert!(!is_newer("0.2.3", "0.2.3"));
        assert!(!is_newer("0.2.2", "0.2.3"));
        assert!(is_newer("v1.0.0", "0.9.9"));
    }
}
