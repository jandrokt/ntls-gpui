//! Working out what to call a file, and where to put it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use percent_encoding::percent_decode_str;

/// Characters no file name may contain, plus the ones that make a name
/// awkward to type at a shell.
const FORBIDDEN: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|', '\0'];

/// Reduces anything to something safe to write to disk, keeping as much of the
/// original as it can. An empty or all-dots result is rejected, since both are
/// ways of naming a directory instead of a file.
pub fn sanitize(name: &str) -> Option<String> {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| if FORBIDDEN.contains(&c) || c.is_control() { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim_matches(['.', ' ']).to_string();
    if cleaned.is_empty() {
        return None;
    }
    // Long names are rejected by most filesystems well before the path limit,
    // and the extension is the part worth keeping.
    Some(if cleaned.chars().count() > 180 { truncate_keeping_extension(&cleaned, 180) } else { cleaned })
}

fn truncate_keeping_extension(name: &str, max: usize) -> String {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| e.len() <= 12)
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    let keep = max.saturating_sub(ext.chars().count());
    let stem: String = name.chars().take(keep).collect();
    format!("{stem}{ext}")
}

/// The name a URL suggests: the last segment of its path, percent-decoded.
pub fn from_url(raw: &str) -> Option<String> {
    let parsed = url::Url::parse(raw).ok()?;
    let last = parsed.path_segments()?.rfind(|s| !s.is_empty())?;
    sanitize(&percent_decode_str(last).decode_utf8_lossy())
}

/// The name a `Content-Disposition` header asks for, preferring the RFC 5987
/// `filename*` form that carries an encoding.
pub fn from_disposition(header: &str) -> Option<String> {
    if let Some(rest) = find_param(header, "filename*") {
        // `UTF-8''some%20name.bin`
        let encoded = rest.rsplit('\'').next().unwrap_or(&rest);
        if let Some(name) = sanitize(&percent_decode_str(encoded).decode_utf8_lossy()) {
            return Some(name);
        }
    }
    sanitize(&find_param(header, "filename")?)
}

/// Pulls one parameter out of a header, unquoting it. Matching the longest
/// name first is what keeps `filename` from also matching `filename*`.
fn find_param(header: &str, key: &str) -> Option<String> {
    for part in header.split(';') {
        // The disposition itself (`attachment`) carries no value, and
        // neither do the parameters a server invents.
        let Some((name, value)) = part.trim().split_once('=') else { continue };
        if name.trim().eq_ignore_ascii_case(key) {
            return Some(value.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// The extension a MIME type implies, for a URL whose path has none.
pub fn extension_for(mime: &str) -> Option<&'static str> {
    let mime = mime.split(';').next()?.trim().to_ascii_lowercase();
    Some(match mime.as_str() {
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        "video/x-matroska" => "mkv",
        "audio/mpeg" => "mp3",
        "audio/mp4" | "audio/x-m4a" => "m4a",
        "audio/ogg" => "ogg",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/wav" | "audio/x-wav" => "wav",
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/avif" => "avif",
        "application/pdf" => "pdf",
        "application/zip" => "zip",
        "application/x-7z-compressed" => "7z",
        "application/vnd.rar" | "application/x-rar-compressed" => "rar",
        "application/gzip" | "application/x-gzip" => "gz",
        "application/x-tar" => "tar",
        "application/epub+zip" => "epub",
        "application/json" => "json",
        "text/plain" => "txt",
        _ => return None,
    })
}

/// Whether a content type is a page instead of a file. Resolution keeps
/// looking while the answer is yes.
pub fn is_markup(mime: &str) -> bool {
    let mime = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    matches!(mime.as_str(), "text/html" | "application/xhtml+xml" | "text/xml" | "application/xml" | "")
}

/// The destinations transfers in this process have already taken.
///
/// Asking the filesystem whether a name is free cannot decide this on its
/// own. A transfer's file does not appear under its final name until the
/// rename at the very end, so for as long as a download runs the name it is
/// going to take still looks free. Two links whose names agree, whether that
/// is the same address pasted twice, two hosts both serving `report.pdf`, or
/// two nameless links that both fall back to the same word, were handed the
/// same destination, shared one part file and one ledger, and wrote over each
/// other's bytes, with whichever finished first announced as a success.
static CLAIMED: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

/// A destination held for one transfer. The part file and the ledger hang off
/// this path by suffix, so holding it holds all three.
pub struct Claim {
    path: PathBuf,
}

impl Claim {
    /// Where the finished file goes.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        // A guard rather than a pair of calls, because the name has to come
        // back whichever way the transfer ended: delivered, failed part way
        // through, or cancelled. A poisoned lock is ignored rather than
        // unwrapped, since this runs while unwinding and a second panic there
        // would end the process.
        if let Ok(mut held) = CLAIMED.lock() {
            held.remove(&self.path);
        }
    }
}

/// Reserves a path in `dir` for `name`, adding ` (2)`, ` (3)` and so on before
/// the extension until it reaches one that nothing occupies and no other
/// transfer in this process is already using. The name is held until the
/// returned [`Claim`] is dropped.
///
/// `overwrite` says the user asked to replace what is on disk, so a file
/// already sitting there is not in the way. A transfer running beside this one
/// still is: two of them sharing a part file corrupt it.
pub fn claim(dir: &Path, name: &str, overwrite: bool) -> Claim {
    let stem = Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = Path::new(name).extension().and_then(|e| e.to_str());
    // Testing and taking under one lock is the whole point. Checking first and
    // reserving afterwards is the bug this replaces.
    let mut held = CLAIMED.lock().expect("claims");
    for n in 1..10_000u32 {
        let candidate = match (n, ext) {
            (1, _) => dir.join(name),
            (_, Some(e)) => dir.join(format!("{stem} ({n}).{e}")),
            (_, None) => dir.join(format!("{stem} ({n})")),
        };
        // A part file left behind by an interrupted transfer does not make a
        // name occupied. It is exactly what a resumed transfer picks up, so
        // only the finished file counts.
        if held.contains(&candidate) || (!overwrite && candidate.exists()) {
            continue;
        }
        held.insert(candidate.clone());
        return Claim { path: candidate };
    }
    // Ten thousand files of one name in one folder deserves no failure path
    // that it did not have before: the plain name is where it goes.
    let path = dir.join(name);
    held.insert(path.clone());
    Claim { path }
}

/// Where downloads go unless the user says otherwise.
pub fn default_dir() -> PathBuf {
    crate::sys::downloads().join("ntls")
}

/// Expands a leading `~` so the destination field can be typed the way it is
/// spoken.
pub fn expand_home(raw: &str) -> PathBuf {
    let raw = raw.trim();
    if raw.is_empty() {
        return default_dir();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return crate::sys::home().join(rest);
    }
    if raw == "~" {
        return crate::sys::home();
    }
    PathBuf::from(raw)
}

/// Today, as a folder name that sorts.
pub fn today() -> String {
    // Days since the epoch, turned into a date by the civil-from-days
    // algorithm. A whole date library to name a folder would be a poor trade.
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0) as i64;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Howard Hinnant's `civil_from_days`, which turns a day number into a date
/// without a calendar library.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Renders a byte count the way a transfer is talked about.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// Renders a transfer rate.
pub fn rate(bytes_per_second: f64) -> String {
    if bytes_per_second <= 0.0 {
        return "—".into();
    }
    format!("{}/s", bytes(bytes_per_second as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_survives_being_made_safe() {
        assert_eq!(sanitize("Holiday/2024: best.mp4").as_deref(), Some("Holiday_2024_ best.mp4"));
        assert_eq!(sanitize("   "), None);
        assert_eq!(sanitize("..."), None);
    }

    #[test]
    fn a_long_name_keeps_its_extension() {
        let long = "a".repeat(400) + ".mkv";
        let out = sanitize(&long).expect("a name");
        assert!(out.ends_with(".mkv"));
        assert_eq!(out.chars().count(), 180);
    }

    #[test]
    fn the_name_comes_from_the_header_before_the_url() {
        assert_eq!(
            from_disposition("attachment; filename=\"report final.pdf\"").as_deref(),
            Some("report final.pdf")
        );
        // The encoded form wins, since it is the one that can carry accents.
        assert_eq!(
            from_disposition("attachment; filename=\"a.pdf\"; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf")
                .as_deref(),
            Some("résumé.pdf")
        );
    }

    #[test]
    fn a_url_names_a_file_by_its_last_segment() {
        assert_eq!(from_url("https://h.example/a/b/my%20file.zip").as_deref(), Some("my file.zip"));
        assert_eq!(from_url("https://h.example/").as_deref(), None);
    }

    #[test]
    fn two_transfers_asking_for_the_same_name_at_once_are_given_separate_files() {
        // While a transfer runs, its final name is not on disk yet, so a name
        // checked only against the filesystem looks free to a second
        // transfer. Both then wrote into one part file, and one was reported
        // as finished while the other overwrote it.
        let dir = std::env::temp_dir().join("ntls-claim-same-name");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("the directory");

        let first = super::claim(&dir, "report.pdf", false);
        let second = super::claim(&dir, "report.pdf", false);
        assert_ne!(first.path(), second.path());
        assert_eq!(first.path(), dir.join("report.pdf"));
        assert_eq!(second.path(), dir.join("report (2).pdf"));

        // A name comes back the moment its transfer ends, however it ended.
        let taken = first.path().to_path_buf();
        drop(first);
        let third = super::claim(&dir, "report.pdf", false);
        assert_eq!(third.path(), taken);

        drop(second);
        drop(third);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_file_already_there_is_stepped_over_unless_replacing_it_was_asked_for() {
        let dir = std::env::temp_dir().join("ntls-claim-existing");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("the directory");
        std::fs::write(dir.join("report.pdf"), b"old").expect("a file");

        let stepped = super::claim(&dir, "report.pdf", false);
        assert_eq!(stepped.path(), dir.join("report (2).pdf"));
        drop(stepped);

        // Asked to replace it, the name on disk is not in the way.
        let over = super::claim(&dir, "report.pdf", true);
        assert_eq!(over.path(), dir.join("report.pdf"));
        drop(over);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_date_folder_sorts_and_is_a_real_date() {
        let name = today();
        assert_eq!(name.len(), 10, "{name}");
        let parts: Vec<&str> = name.split('-').collect();
        assert_eq!(parts.len(), 3);
        let year: i64 = parts[0].parse().expect("a year");
        let month: u32 = parts[1].parse().expect("a month");
        let day: u32 = parts[2].parse().expect("a day");
        assert!((2020..2200).contains(&year), "{name}");
        assert!((1..=12).contains(&month), "{name}");
        assert!((1..=31).contains(&day), "{name}");
    }

    #[test]
    fn known_days_turn_into_known_dates() {
        // The epoch, and a leap day, which is where this kind of arithmetic
        // usually goes wrong.
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }

    #[test]
    fn pages_are_told_apart_from_files() {
        assert!(is_markup("text/html; charset=utf-8"));
        assert!(!is_markup("video/mp4"));
        // A server that says nothing might be serving anything, so it is worth
        // another look instead of a download.
        assert!(is_markup(""));
    }
}
