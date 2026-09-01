//! The extractors other people maintain.
//!
//! `yt-dlp` knows how to get media off some eighteen hundred sites and
//! `gallery-dl` off several hundred more, and both are updated as those sites
//! change. Writing any of that here would mean maintaining it here, so ntls
//! asks them instead: whether they recognise a link, and, for the media
//! extractor whose answers are often several files to be put back together,
//! to do the download and report progress while it does.
//!
//! Neither is required. With both missing, the page reader in
//! [`super::resolve`] still handles direct links and the file hosts that
//! simply link to what they host.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::core::Cancel;

use super::fetch::Progress;
use super::resolve::Prefer;

/// One of the two extractors.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Extractor {
    YtDlp,
    GalleryDl,
}

impl Extractor {
    pub const ALL: [Extractor; 2] = [Extractor::YtDlp, Extractor::GalleryDl];

    pub fn binary(self) -> &'static str {
        match self {
            Extractor::YtDlp => "yt-dlp",
            Extractor::GalleryDl => "gallery-dl",
        }
    }

    /// How to install it, for the one log line that says it is missing.
    pub fn install(self) -> &'static str {
        match self {
            Extractor::YtDlp => "brew install yt-dlp  (or: pipx install yt-dlp)",
            Extractor::GalleryDl => "brew install gallery-dl  (or: pipx install gallery-dl)",
        }
    }

    /// Whether this extractor should be consulted, given what the user asked
    /// for.
    pub fn wanted_by(self, prefer: Prefer) -> bool {
        match prefer {
            Prefer::Auto => true,
            Prefer::YtDlp => self == Extractor::YtDlp,
            Prefer::GalleryDl => self == Extractor::GalleryDl,
            Prefer::Direct | Prefer::Page => false,
        }
    }

    /// Where the binary is, if it is anywhere.
    pub fn found(self) -> Option<PathBuf> {
        which(self.binary())
    }
}

/// What an extractor said about a link it recognised.
#[derive(Clone, Debug, Default)]
pub struct Claim {
    pub title: String,
    /// Plain URLs, for the extractor that hands them over.
    pub urls: Vec<String>,
}

/// Asks one extractor whether it knows the link, without downloading anything.
pub async fn claim(extractor: Extractor, url: &str, cancel: &Cancel) -> Result<Option<Claim>> {
    if extractor.found().is_none() {
        return Ok(None);
    }
    match extractor {
        Extractor::YtDlp => ytdlp_claim(url, cancel).await,
        Extractor::GalleryDl => gallerydl_claim(url, cancel).await,
    }
}

/// Asks `yt-dlp` to describe the link, via the crate that parses its output
/// into something typed.
async fn ytdlp_claim(url: &str, cancel: &Cancel) -> Result<Option<Claim>> {
    let url = url.to_string();
    let query = tokio::task::spawn_blocking(move || {
        youtube_dl::YoutubeDl::new(url)
            .socket_timeout("20")
            .extra_arg("--no-warnings")
            .extra_arg("--flat-playlist")
            .run()
    });

    let Some(outcome) = cancel.run(query).await else { return Ok(None) };
    match outcome.context("yt-dlp did not run")? {
        Ok(youtube_dl::YoutubeDlOutput::SingleVideo(video)) => {
            Ok(Some(Claim { title: video.title.unwrap_or_default(), urls: Vec::new() }))
        }
        Ok(youtube_dl::YoutubeDlOutput::Playlist(playlist)) => {
            let count = playlist.entries.as_ref().map_or(0, Vec::len);
            let title = playlist.title.unwrap_or_default();
            Ok(Some(Claim { title: format!("{title} ({count} items)"), urls: Vec::new() }))
        }
        // Not recognising a link is the ordinary case, not a failure.
        Err(_) => Ok(None),
    }
}

/// Asks `gallery-dl` for the plain URLs behind a gallery.
async fn gallerydl_claim(url: &str, cancel: &Cancel) -> Result<Option<Claim>> {
    let out = run(
        Extractor::GalleryDl.binary(),
        &["--get-urls", "--quiet", "--no-download", url],
        cancel,
    )
    .await?;
    let Some(out) = out else { return Ok(None) };
    if !out.ok {
        return Ok(None);
    }
    let urls: Vec<String> = out
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("http"))
        .map(str::to_string)
        .collect();
    Ok((!urls.is_empty()).then_some(Claim { title: String::new(), urls }))
}

/// Hands a link to `yt-dlp` and follows along.
///
/// Progress comes back on stdout in a format we choose, so there is no output
/// scraping to keep in step with the tool's own formatting.
pub async fn ytdlp_download(
    url: &str,
    dir: &std::path::Path,
    quality: &str,
    cancel: &Cancel,
    progress: &Arc<Progress>,
    log: impl Fn(String) + Send + 'static,
) -> Result<Vec<PathBuf>> {
    let binary = Extractor::YtDlp
        .found()
        .with_context(|| format!("yt-dlp is not installed. {}", Extractor::YtDlp.install()))?;
    std::fs::create_dir_all(dir)?;

    let output_template = dir.join("%(title)s [%(id)s].%(ext)s");
    let mut command = tokio::process::Command::new(binary);
    command
        .arg("--newline")
        .arg("--no-colors")
        .arg("--no-warnings")
        .arg("--no-part")
        .arg("--concurrent-fragments")
        .arg("4")
        .arg("--progress-template")
        .arg(concat!(
            "download:", "NTLS\t%(progress.downloaded_bytes)s\t",
            "%(progress.total_bytes,progress.total_bytes_estimate)s\t%(info.title)s"
        ))
        .arg("--print")
        .arg("after_move:NTLSFILE\t%(filepath)s")
        .arg("-o")
        .arg(&output_template);
    for arg in quality_args(quality) {
        command.arg(arg);
    }
    command.arg(url).stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = command.spawn().context("cannot start yt-dlp")?;
    let stdout = child.stdout.take().context("yt-dlp has no output")?;
    let stderr = child.stderr.take().context("yt-dlp has no error output")?;

    let progress = progress.clone();
    let reader = tokio::spawn(async move {
        let mut files = Vec::new();
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(rest) = line.strip_prefix("NTLS\t") {
                let mut parts = rest.split('\t');
                let done: u64 = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                let total: u64 = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                progress.done.store(done, Ordering::Relaxed);
                progress.total.store(total, Ordering::Relaxed);
            } else if let Some(path) = line.strip_prefix("NTLSFILE\t") {
                files.push(PathBuf::from(path.trim()));
            } else if !line.trim().is_empty() {
                log(line);
            }
        }
        files
    });

    // yt-dlp says everything interesting about a failure on stderr, and it is
    // worth keeping only if the run fails.
    let errors = tokio::spawn(async move {
        let mut collected = String::new();
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            collected.push_str(line.trim());
            collected.push('\n');
        }
        collected
    });

    let status = match cancel.run(child.wait()).await {
        Some(status) => status?,
        None => {
            child.start_kill().ok();
            bail!("cancelled");
        }
    };
    let files = reader.await.unwrap_or_default();
    let errors = errors.await.unwrap_or_default();

    if !status.success() {
        let reason = errors.lines().rev().find(|l| !l.is_empty()).unwrap_or("yt-dlp failed");
        bail!("{reason}");
    }
    Ok(files)
}

/// The format selection behind each quality choice, in yt-dlp's own language.
fn quality_args(quality: &str) -> Vec<String> {
    let owned = |args: &[&str]| args.iter().map(|s| s.to_string()).collect();
    match quality {
        "audio" => owned(&["-f", "bestaudio/best", "-x", "--audio-format", "mp3"]),
        "1080" => owned(&["-f", "bestvideo[height<=1080]+bestaudio/best[height<=1080]/best"]),
        "720" => owned(&["-f", "bestvideo[height<=720]+bestaudio/best[height<=720]/best"]),
        _ => owned(&["-f", "bestvideo+bestaudio/best"]),
    }
}

struct Output {
    ok: bool,
    stdout: String,
}

/// Runs a command to completion, giving up if the job is cancelled.
async fn run(binary: &str, args: &[&str], cancel: &Cancel) -> Result<Option<Output>> {
    let Some(path) = which(binary) else { return Ok(None) };
    let child = tokio::process::Command::new(path)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .with_context(|| format!("cannot start {binary}"))?;

    let Some(outcome) = cancel.run(child.wait_with_output()).await else {
        bail!("cancelled");
    };
    let outcome = outcome.with_context(|| format!("{binary} did not finish"))?;
    Ok(Some(Output {
        ok: outcome.status.success(),
        stdout: String::from_utf8_lossy(&outcome.stdout).into_owned(),
    }))
}

/// Finds a binary on `PATH`, plus the places package managers put things that
/// a windowed application's environment does not always include.
pub fn which(binary: &str) -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    // The places a package manager puts things that `PATH` may not mention,
    // because a GUI application does not inherit a login shell's environment.
    #[cfg(not(windows))]
    for extra in ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"] {
        roots.push(PathBuf::from(extra));
    }
    let home = crate::sys::home();
    roots.push(home.join(".local/bin"));
    roots.push(home.join("bin"));
    #[cfg(windows)]
    {
        // winget and pip put programs here, and neither is on the PATH a
        // desktop application starts with.
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let local = PathBuf::from(local);
            roots.push(local.join("Microsoft").join("WindowsApps"));
            roots.push(local.join("Programs"));
        }
    }

    // On Windows the program is not called what it is called: `yt-dlp` is
    // `yt-dlp.exe`, and looking only for the bare name finds nothing.
    let names = crate::sys::program_names(binary);
    roots
        .into_iter()
        .flat_map(|root| names.iter().map(move |name| root.join(name)))
        .find(|candidate| crate::sys::is_executable(candidate))
}

/// A line for the log saying what is available to work with.
pub fn availability() -> String {
    let mut have = Vec::new();
    let mut missing = Vec::new();
    for extractor in Extractor::ALL {
        if extractor.found().is_some() {
            have.push(extractor.binary());
        } else {
            missing.push(extractor.binary());
        }
    }
    match (have.is_empty(), missing.is_empty()) {
        (_, true) => format!("Extractors: {}", have.join(", ")),
        (true, _) => format!(
            "No extractors installed. Direct links and simple file hosts only. Install with: {}",
            Extractor::YtDlp.install()
        ),
        _ => format!("Extractors: {} · missing: {}", have.join(", "), missing.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_maps_to_a_format_selector() {
        assert!(quality_args("audio").contains(&"-x".to_string()));
        assert!(quality_args("720").iter().any(|a| a.contains("height<=720")));
        assert!(quality_args("best").iter().any(|a| a.contains("bestvideo")));
    }

    #[test]
    fn a_preference_narrows_which_extractors_are_asked() {
        assert!(Extractor::ALL.iter().all(|e| e.wanted_by(Prefer::Auto)));
        assert!(Extractor::YtDlp.wanted_by(Prefer::YtDlp));
        assert!(!Extractor::GalleryDl.wanted_by(Prefer::YtDlp));
        assert!(Extractor::ALL.iter().all(|e| !e.wanted_by(Prefer::Direct)));
    }

    #[test]
    fn a_binary_that_exists_is_found_and_one_that_does_not_is_not() {
        assert!(which("sh").is_some());
        assert!(which("ntls-definitely-not-a-real-binary").is_none());
    }
}
