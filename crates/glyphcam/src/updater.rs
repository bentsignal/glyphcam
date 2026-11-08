//! Built-in build manager: lists this repo's GitHub Releases (including the
//! rolling `nightly` prerelease built from `main`), downloads the build for
//! this platform and installs it (upgrade, downgrade or switch channel).

use crate::config::{self, BUILD_SHA, CHANNEL, VERSION};
use parking_lot::Mutex;
use serde::Deserialize;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const REPO: &str = "bentsignal/glyphcam";

#[derive(Clone, Debug, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    pub html_url: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

impl Release {
    pub fn title(&self) -> String {
        self.name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| self.tag_name.clone())
    }

    pub fn date(&self) -> String {
        self.published_at
            .as_deref()
            .map(|d| d.chars().take(10).collect())
            .unwrap_or_default()
    }

    /// The asset that installs on this platform.
    pub fn platform_asset(&self) -> Option<&Asset> {
        let suffix = if cfg!(windows) {
            "-windows-x64-setup.exe"
        } else if cfg!(target_os = "macos") {
            "-macos-universal.app.zip"
        } else {
            return None;
        };
        self.assets.iter().find(|a| a.name.ends_with(suffix))
    }

    pub fn is_nightly(&self) -> bool {
        self.tag_name == "nightly"
    }

    /// Is this the build that's currently running?
    pub fn is_current(&self) -> bool {
        if self.is_nightly() {
            CHANNEL == "nightly" && self.title().contains(BUILD_SHA)
        } else {
            CHANNEL == "stable" && self.tag_name.trim_start_matches('v') == VERSION
        }
    }

    pub fn version(&self) -> Option<semver::Version> {
        semver::Version::parse(self.tag_name.trim_start_matches('v')).ok()
    }
}

#[derive(Clone, Debug, Default)]
pub enum ListState {
    #[default]
    Idle,
    Loading,
    Loaded(Vec<Release>),
    Failed(String),
}

#[derive(Clone, Debug, Default)]
pub enum Job {
    #[default]
    None,
    Downloading {
        tag: String,
        got: u64,
        total: u64,
    },
    Installing {
        tag: String,
    },
    Failed(String),
}

#[derive(Default)]
struct State {
    list: ListState,
    job: Job,
}

#[derive(Clone, Default)]
pub struct Updater {
    st: Arc<Mutex<State>>,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(600)))
        .build()
        .into()
}

impl Updater {
    pub fn list(&self) -> ListState {
        self.st.lock().list.clone()
    }

    pub fn job(&self) -> Job {
        self.st.lock().job.clone()
    }

    pub fn refresh(&self, wake: impl Fn() + Send + 'static) {
        {
            let mut s = self.st.lock();
            if matches!(s.list, ListState::Loading) {
                return;
            }
            s.list = ListState::Loading;
        }
        let st = self.st.clone();
        std::thread::spawn(move || {
            let res = fetch_releases();
            st.lock().list = match res {
                Ok(v) => ListState::Loaded(v),
                Err(e) => ListState::Failed(format!("{e:#}")),
            };
            wake();
        });
    }

    /// Newest stable release that is newer than the running version (or a
    /// new nightly when running a nightly).
    pub fn available_update(&self, include_pre: bool) -> Option<Release> {
        let ListState::Loaded(list) = &self.st.lock().list else {
            return None;
        };
        let cur = semver::Version::parse(VERSION).ok()?;
        if CHANNEL == "nightly" {
            if let Some(n) = list.iter().find(|r| r.is_nightly()) {
                if !n.is_current() && n.platform_asset().is_some() {
                    return Some(n.clone());
                }
            }
        }
        list.iter()
            .filter(|r| {
                !r.is_nightly() && (include_pre || !r.prerelease) && r.platform_asset().is_some()
            })
            .filter_map(|r| r.version().map(|v| (v, r)))
            .filter(|(v, _)| *v > cur)
            .max_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, r)| r.clone())
    }

    pub fn install(
        &self,
        rel: Release,
        on_ready_to_quit: impl FnOnce() + Send + 'static,
        wake: impl Fn() + Send + 'static,
    ) {
        let Some(asset) = rel.platform_asset().cloned() else {
            self.st.lock().job = Job::Failed("This release has no build for this platform".into());
            return;
        };
        let st = self.st.clone();
        st.lock().job = Job::Downloading {
            tag: rel.tag_name.clone(),
            got: 0,
            total: asset.size,
        };
        std::thread::spawn(move || {
            let tag = rel.tag_name.clone();
            let result = (|| -> anyhow::Result<()> {
                let dir = config::cache_dir().join("downloads");
                std::fs::create_dir_all(&dir)?;
                let file = dir.join(format!("{}-{}", sanitize(&tag), asset.name));
                download(&asset.browser_download_url, &file, |got, total| {
                    st.lock().job = Job::Downloading {
                        tag: tag.clone(),
                        got,
                        total: total.max(asset.size),
                    };
                    wake();
                })?;
                st.lock().job = Job::Installing { tag: tag.clone() };
                wake();
                launch_installer(&file)?;
                Ok(())
            })();
            match result {
                Ok(()) => on_ready_to_quit(),
                Err(e) => {
                    st.lock().job = Job::Failed(format!("{e:#}"));
                    wake();
                }
            }
        });
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn fetch_releases() -> anyhow::Result<Vec<Release>> {
    let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=40");
    let mut resp = agent()
        .get(&url)
        .header("User-Agent", "glyphcam-updater")
        .header("Accept", "application/vnd.github+json")
        .call()?;
    let mut list: Vec<Release> = resp.body_mut().read_json()?;
    list.retain(|r| !r.draft);
    // nightly first, then newest versions first
    list.sort_by(|a, b| {
        b.is_nightly()
            .cmp(&a.is_nightly())
            .then_with(|| b.published_at.cmp(&a.published_at))
    });
    Ok(list)
}

fn download(url: &str, dest: &Path, mut progress: impl FnMut(u64, u64)) -> anyhow::Result<()> {
    let resp = agent()
        .get(url)
        .header("User-Agent", "glyphcam-updater")
        .call()?;
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let mut reader = resp.into_body().into_reader();
    let tmp = dest.with_extension("part");
    let mut out = std::fs::File::create(&tmp)?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut got = 0u64;
    let mut last_report = std::time::Instant::now();
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        got += n as u64;
        if last_report.elapsed().as_millis() > 100 {
            progress(got, total);
            last_report = std::time::Instant::now();
        }
    }
    out.flush()?;
    drop(out);
    if total > 0 && got != total {
        anyhow::bail!("download incomplete ({got} of {total} bytes)");
    }
    std::fs::rename(&tmp, dest)?;
    progress(got, total);
    Ok(())
}

/// Start the installer for `file`. The caller quits the app right after.
#[cfg(windows)]
fn launch_installer(file: &Path) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    std::process::Command::new(file)
        .args(["/S", "/RELAUNCH"])
        .creation_flags(DETACHED_PROCESS)
        .spawn()?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn launch_installer(file: &Path) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let s = exe.to_string_lossy().to_string();
    let i = s
        .find(".app/Contents/MacOS/")
        .ok_or_else(|| anyhow::anyhow!("not running from an .app bundle"))?;
    let app = PathBuf::from(&s[..i + 4]);
    let script = config::cache_dir().join("install-update.sh");
    std::fs::write(
        &script,
        r#"#!/bin/sh
# glyphcam self-update: wait for the app to quit, swap the bundle, relaunch.
PID="$1"; ZIP="$2"; APP="$3"
for i in $(seq 1 100); do kill -0 "$PID" 2>/dev/null || break; sleep 0.2; done
TMP=$(mktemp -d)
/usr/bin/ditto -x -k "$ZIP" "$TMP" || exit 1
NEW=$(find "$TMP" -maxdepth 1 -name "*.app" | head -n 1)
[ -n "$NEW" ] || exit 1
rm -rf "$APP.old"
mv "$APP" "$APP.old" && mv "$NEW" "$APP" && rm -rf "$APP.old"
/usr/bin/xattr -dr com.apple.quarantine "$APP" 2>/dev/null
open "$APP"
rm -rf "$TMP"
"#,
    )?;
    std::process::Command::new("/bin/sh")
        .arg(&script)
        .arg(std::process::id().to_string())
        .arg(file)
        .arg(&app)
        .spawn()?;
    Ok(())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn launch_installer(_file: &Path) -> anyhow::Result<()> {
    anyhow::bail!("self-update is not supported on this platform")
}

#[allow(dead_code)]
fn _unused(_: PathBuf) {}
