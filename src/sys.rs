//! What differs between the systems ntls runs on.
//!
//! Everything platform-shaped that is not networking lives here, so the rest
//! of the program can ask a plain question (where is home, what is this
//! machine called, can this file be run) and get the same answer on macOS,
//! Linux and Windows.

use std::path::{Path, PathBuf};

/// This user's home directory.
///
/// `HOME` is the answer on macOS and Linux, and is often set on Windows too;
/// `USERPROFILE` is the one Windows always sets. A machine with neither still
/// runs, in the directory it was started from, and does not refuse to start.
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Where a browser and everything else puts what it downloads.
pub fn downloads() -> PathBuf {
    home().join("Downloads")
}

/// What this machine calls itself, for the peers that have to recognise it.
///
/// Windows sets `COMPUTERNAME` and Unix often sets `HOSTNAME`; failing both,
/// `hostname` is a program on all three. Failing that too, the name is the
/// program's own. A peer list of several "ntls" beats a peer
/// list of none.
pub fn hostname() -> String {
    for key in ["COMPUTERNAME", "HOSTNAME"] {
        if let Some(name) = std::env::var_os(key)
            && !name.is_empty()
        {
            return name.to_string_lossy().trim().to_string();
        }
    }
    quietly(&mut std::process::Command::new("hostname"))
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "ntls".into())
}

/// How tall the strip across the top of the window is, and how much room the
/// window's own buttons need at the left of it.
///
/// On macOS the system titlebar is hidden and ntls draws its own, so these two
/// have to agree with each other and with where the traffic lights are put:
/// the lights are 12pt across and sit in a row 20pt from the left edge, so the
/// strip is as tall as a light plus the same margin above and below it, and
/// [`TRAFFIC_LIGHT_TOP`] is what centres them in it. Getting this wrong is
/// what makes a window's own titlebar look subtly unlike every other window's.
///
/// Elsewhere the system draws the titlebar and this strip is just a toolbar,
/// which wants the same height for the same reason: it holds the same
/// controls.
pub const TITLEBAR_HEIGHT: f32 = 38.;

/// The macOS window buttons: their diameter, and where the row of them starts.
pub const TRAFFIC_LIGHT_SIZE: f32 = 12.;
pub const TRAFFIC_LIGHT_LEFT: f32 = 20.;

/// Where the top of a traffic light goes, so the row sits on the strip's
/// centre line.
pub const TRAFFIC_LIGHT_TOP: f32 = (TITLEBAR_HEIGHT - TRAFFIC_LIGHT_SIZE) / 2.;

/// Where the titlebar's own content can start: past all three buttons.
pub const TITLEBAR_INSET: f32 = TRAFFIC_LIGHT_LEFT + TRAFFIC_LIGHT_SIZE * 3. + 8. * 2. + 12.;

/// One Windows caption button: minimise, maximise and close are this wide
/// each, and as tall as the strip they sit in.
///
/// 46 is what Windows itself uses at 100%, and a window whose buttons are a
/// different size from every other window's is the sort of thing that reads as
/// wrong without being able to say why.
pub const CAPTION_BUTTON: f32 = 46.;

/// How much of the titlebar's top edge is left to the system.
///
/// With the system titlebar hidden, Windows asks what every point in the
/// window is before it decides what a click there means, and it asks ntls
/// first. A strip that claimed the whole height would claim the top edge with
/// it, and the top edge is what the window is resized by. This much is left
/// unclaimed, so the answer falls through to "this is the top border".
pub const TOP_RESIZE_EDGE: f32 = 4.;

/// `CREATE_NO_WINDOW`: the process flag that stops a child command opening a
/// console window of its own.
///
/// A program in the windows subsystem has no console, so every command it runs
/// is given a brand new one, which flashes up on screen and takes the focus
/// with it. Naming the constant here saves depending on `windows-sys` for one
/// number.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Runs a command without letting it open a console window.
///
/// Only Windows has anything to do here. Everywhere else a child process has
/// no window to begin with and this hands the command straight back.
pub fn quietly(command: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// What this build of ntls is, as a number.
///
/// ntls counts builds, not versions: CI stamps its pipeline's own count
/// in at compile time, so it goes up by one on every commit and a binary can
/// always be traced back to the one that made it. Empty means nobody stamped
/// it. Every build made outside CI is one.
pub const BUILD: &str = env!("NTLS_BUILD");

/// What to call this build on screen.
pub fn build_label() -> String {
    if BUILD.is_empty() { "local build".into() } else { format!("build {BUILD}") }
}

/// The same, for anything that has to be one token: a user agent, a file name.
pub fn build_tag() -> String {
    if BUILD.is_empty() { "dev".into() } else { BUILD.to_string() }
}

/// The modifier key application shortcuts are written with here.
///
/// macOS uses Command for what every other desktop uses Control for, and a
/// program that binds ⌘ everywhere is a program whose shortcuts do nothing on
/// Windows and Linux.
pub const ACCEL: &str = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };

/// Rewrites a shortcut written the macOS way for whichever system this is.
///
/// One place to say it, so a binding and the keycap that advertises it cannot
/// drift apart.
pub fn shortcut(combo: &str) -> String {
    combo.replace("cmd", ACCEL)
}

/// The same, for showing to somebody: `cmd-shift-w` reads as `⇧⌘W` on macOS
/// and `Ctrl+Shift+W` everywhere else.
pub fn shortcut_label(combo: &str) -> String {
    let mut keys: Vec<&str> = combo.split('-').collect();
    let last = keys.pop().unwrap_or_default();
    let key = if last.chars().count() == 1 { last.to_uppercase() } else { title(last) };
    if cfg!(target_os = "macos") {
        let mut out = String::new();
        // The order the symbols are always written in.
        for (name, symbol) in [("ctrl", "\u{2303}"), ("alt", "\u{2325}"), ("shift", "\u{21e7}"), ("cmd", "\u{2318}")] {
            if keys.contains(&name) {
                out.push_str(symbol);
            }
        }
        out.push_str(&key);
        out
    } else {
        let mut out = String::new();
        for name in keys {
            out.push_str(&title(&name.replace("cmd", "ctrl")));
            out.push('+');
        }
        out.push_str(&key);
        out
    }
}

fn title(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Whether this path is something the system will run.
///
/// Unix asks the file: it is the execute bit. Windows asks the name, because
/// that is what Windows runs on, and `PATHEXT` says which names count.
pub fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let Some(extension) = path.extension().map(|e| e.to_string_lossy().to_lowercase()) else {
            return false;
        };
        let listed = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        listed
            .split(';')
            .map(|e| e.trim_start_matches('.').to_lowercase())
            .any(|e| e == extension)
    }
}

/// The names a program might have here.
///
/// `yt-dlp` on Windows is `yt-dlp.exe`, and a search of `PATH` that only looks
/// for the bare name finds nothing at all.
pub fn program_names(name: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        let listed = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        let mut names = vec![name.to_string()];
        names.extend(
            listed
                .split(';')
                .filter(|e| !e.trim().is_empty())
                .map(|e| format!("{name}{}", e.trim().to_lowercase())),
        );
        names
    }
    #[cfg(not(windows))]
    {
        vec![name.to_string()]
    }
}

/// Writes into a file at an offset, without moving anything else's idea of
/// where it is.
///
/// A download runs several ranged requests at once into one file, so every
/// writer needs its own offset. Unix has `write_at`; Windows has `seek_write`,
/// which is the same thing with the seek folded in.
pub fn write_at(file: &std::fs::File, buf: &[u8], offset: u64) -> std::io::Result<usize> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.write_at(buf, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        file.seek_write(buf, offset)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (file, buf, offset);
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "no positional writes here"))
    }
}

/// Writes a whole buffer at an offset, however many writes that takes.
pub fn write_all_at(file: &std::fs::File, mut buf: &[u8], mut offset: u64) -> std::io::Result<()> {
    while !buf.is_empty() {
        match write_at(file, buf, offset) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "the file stopped accepting writes",
                ));
            }
            Ok(n) => {
                buf = &buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_is_somewhere_even_when_nothing_says_where() {
        // The fallback is the directory ntls was started in, a real
        // place. The alternative is refusing to start.
        assert!(!home().as_os_str().is_empty());
        assert!(downloads().ends_with("Downloads"));
    }

    #[test]
    fn a_build_says_whether_it_came_from_ci() {
        // The number is stamped in by CI and nowhere else, so a binary that
        // claims one is a binary CI made.
        if BUILD.is_empty() {
            assert_eq!(build_label(), "local build");
            assert_eq!(build_tag(), "dev");
        } else {
            assert!(BUILD.chars().all(|c| c.is_ascii_digit()));
            assert_eq!(build_label(), format!("build {BUILD}"));
            assert_eq!(build_tag(), BUILD);
        }
    }

    #[test]
    fn this_machine_has_a_name() {
        assert!(!hostname().is_empty());
    }

    #[test]
    fn a_shortcut_is_written_the_way_this_system_writes_it() {
        if cfg!(target_os = "macos") {
            assert_eq!(shortcut("cmd-k"), "cmd-k");
            assert_eq!(shortcut_label("cmd-k"), "\u{2318}K");
            assert_eq!(shortcut_label("cmd-shift-w"), "\u{21e7}\u{2318}W");
        } else {
            assert_eq!(shortcut("cmd-k"), "ctrl-k");
            assert_eq!(shortcut_label("cmd-k"), "Ctrl+K");
            assert_eq!(shortcut_label("cmd-shift-w"), "Ctrl+Shift+W");
        }
    }

    #[test]
    fn a_program_is_looked_for_under_the_names_it_could_have() {
        let names = program_names("yt-dlp");
        assert!(names.contains(&"yt-dlp".to_string()));
        // On Windows the bare name is not the file name, and looking only for
        // it is how a tool that is installed reads as missing.
        #[cfg(windows)]
        assert!(names.iter().any(|n| n.ends_with(".exe")));
    }

    #[test]
    fn every_writer_puts_its_own_piece_where_it_belongs() {
        // Which is the whole reason a download can run several ranged
        // requests at once into one file.
        let path = std::env::temp_dir().join("ntls-sys-offset-test");
        let _ = std::fs::remove_file(&path);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .expect("the file");

        write_all_at(&file, b"second", 8).expect("the write");
        write_all_at(&file, b"first!!!", 0).expect("the write");
        drop(file);

        assert_eq!(std::fs::read(&path).expect("it to be readable"), b"first!!!second");
        let _ = std::fs::remove_file(&path);
    }
}

