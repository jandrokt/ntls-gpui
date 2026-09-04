//! Notices: the things ntls has to tell you.
//!
//! A run finishes in a tab you are not looking at. A workspace will not write
//! itself to disk. An export lands somewhere. None of that fits in the window
//! you happen to be looking at, so it collects here: shown briefly in the
//! corner, then kept in a list.
//!
//! A notice says what happened. Where there is somewhere to go, it carries
//! [`About`] and clicking it goes there.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::core::Level;

/// What a notice is about, so reading it can take you to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum About {
    /// A run, wherever it lives. Opening it selects its workspace and its tab.
    Job(usize),
    /// A file or directory, revealed, not opened: ntls does
    /// not decide what should read your files.
    File(PathBuf),
}

/// One thing worth telling the user.
#[derive(Clone, Debug)]
pub struct Notice {
    pub id: usize,
    pub level: Level,
    /// One line, in the fewest words that still say which thing this is about.
    pub title: String,
    /// The detail, when there is any. Empty is normal.
    pub body: String,
    pub at: Instant,
    pub about: Option<About>,
    /// When it should stop showing itself in the corner. `None` means it
    /// stays until it is dismissed, as anything that went wrong
    /// does. A failure that scrolls away unread was never reported.
    pub until: Option<Instant>,
    /// Whether it has been read. The count on the bell is of the ones that
    /// have not.
    pub read: bool,
}

impl Notice {
    /// Whether it should still be showing itself in the corner.
    pub fn showing(&self, now: Instant) -> bool {
        !self.read && self.until.is_none_or(|until| until > now)
    }

    /// How long ago it happened, in the words a person would use.
    pub fn ago(&self) -> String {
        let seconds = self.at.elapsed().as_secs();
        match seconds {
            0..=5 => "now".into(),
            6..=59 => format!("{seconds}s ago"),
            60..=5399 => format!("{} min ago", (seconds as f64 / 60.).round().max(1.) as u64),
            _ => format!("{} h ago", (seconds as f64 / 3600.).round() as u64),
        }
    }

    /// The icon that stands for what kind of news this is.
    pub fn icon(&self) -> &'static str {
        match self.level {
            Level::Good => "check",
            Level::Warn | Level::Error => "alert",
            Level::Info => "bell",
        }
    }
}

/// How many notices are kept. Past this the oldest fall off, as the log
/// pane's do.
const MAX: usize = 200;

/// How long past its deadline a notice still asks the window to repaint.
///
/// The window draws only when it is told to, and the frame that leaves a
/// spent notice out of the corner can only be drawn after its deadline has
/// passed. Asking for nothing the moment the deadline lands meant no such
/// frame was ever asked for: the last one drawn still had the toast in it, so
/// on a machine nobody was touching it sat in the corner until something
/// unrelated happened to repaint the window. This is longer than the
/// half-second tick that does the asking, so at least one frame arrives
/// after the deadline, and short enough that a notice nobody is waiting on
/// stops costing frames.
const LINGER: Duration = Duration::from_secs(1);

/// The notices raised so far, oldest first.
#[derive(Default)]
pub struct Notices {
    items: Vec<Notice>,
    next_id: usize,
    /// Whether the list is open over the status bar.
    pub open: bool,
}

impl Notices {
    /// Records one, and answers with its id.
    ///
    /// `keep_for` is how long it shows itself in the corner; `None` keeps it
    /// there until it is dismissed.
    pub fn push(
        &mut self,
        level: Level,
        title: impl Into<String>,
        body: impl Into<String>,
        about: Option<About>,
        keep_for: Option<Duration>,
    ) -> usize {
        self.next_id += 1;
        let now = Instant::now();
        self.items.push(Notice {
            id: self.next_id,
            level,
            title: title.into(),
            body: body.into(),
            at: now,
            about,
            until: keep_for.map(|d| now + d),
            read: false,
        });
        if self.items.len() > MAX {
            self.items.drain(..self.items.len() - MAX);
        }
        self.next_id
    }

    /// Every notice, newest first, the order a list of news reads in.
    pub fn newest_first(&self) -> impl Iterator<Item = &Notice> {
        self.items.iter().rev()
    }

    /// How many have not been read. This is the number on the bell, so it
    /// counts what is waiting instead of what has happened.
    pub fn unread(&self) -> usize {
        self.items.iter().filter(|n| !n.read).count()
    }

    /// The ones still showing themselves in the corner, oldest first so the
    /// newest ends up nearest the bell.
    ///
    /// Only a few at once: a scan finishing in every one of eight workspaces
    /// should not cover the window.
    pub fn showing(&self, limit: usize) -> Vec<&Notice> {
        let now = Instant::now();
        let mut out: Vec<&Notice> = self.items.iter().filter(|n| n.showing(now)).collect();
        if out.len() > limit {
            out.drain(..out.len() - limit);
        }
        out
    }

    /// Whether anything is counting down, so the window knows to keep
    /// repainting until it has gone.
    pub fn any_fading(&self) -> bool {
        let now = Instant::now();
        self.items
            .iter()
            .any(|n| !n.read && n.until.is_some_and(|until| until + LINGER > now))
    }

    pub fn get(&self, id: usize) -> Option<&Notice> {
        self.items.iter().find(|n| n.id == id)
    }

    /// Marks one as read. Taking it out of the corner means this.
    pub fn dismiss(&mut self, id: usize) {
        if let Some(n) = self.items.iter_mut().find(|n| n.id == id) {
            n.read = true;
        }
    }

    /// Marks everything as read. Opening the list does this.
    pub fn mark_all_read(&mut self) {
        for n in &mut self.items {
            n.read = true;
        }
    }

    /// Forgets everything. The list is news, not a record; the log pane of
    /// each run is where what happened is kept.
    pub fn clear(&mut self) {
        self.items.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notices() -> Notices {
        Notices::default()
    }

    #[test]
    fn something_that_went_wrong_stays_until_it_is_dismissed() {
        let mut n = notices();
        let good = n.push(
            Level::Good,
            "Ping finished",
            "",
            None,
            Some(Duration::from_secs(5)),
        );
        let bad = n.push(
            Level::Error,
            "IP scan failed",
            "no route to host",
            None,
            None,
        );

        // A failure that scrolls away unread was never reported, so it gets
        // no deadline at all.
        assert!(n.get(bad).expect("it").until.is_none());
        assert!(n.get(good).expect("it").until.is_some());

        assert_eq!(n.showing(8).len(), 2);
        n.dismiss(bad);
        assert_eq!(n.showing(8).len(), 1);
        assert_eq!(n.unread(), 1);
    }

    #[test]
    fn one_that_has_run_out_of_time_stops_showing_itself_and_stays_in_the_list() {
        let mut n = notices();
        n.push(
            Level::Good,
            "Ping finished",
            "",
            None,
            Some(Duration::from_millis(0)),
        );
        assert!(n.showing(8).is_empty(), "its time is up");
        assert_eq!(n.newest_first().count(), 1, "but it is still news");
        assert_eq!(n.unread(), 1);
    }

    #[test]
    fn one_whose_time_is_up_still_asks_for_the_frame_that_takes_it_off_the_screen() {
        let mut n = notices();
        n.push(
            Level::Good,
            "Ping finished",
            "",
            None,
            Some(Duration::from_millis(0)),
        );
        // Its time is up, so it is not drawn any more; but the frame that
        // stops drawing it has still to be asked for, and the deadline is
        // already behind us.
        assert!(n.showing(8).is_empty());
        assert!(n.any_fading(), "the corner still has to be redrawn once");
    }

    #[test]
    fn one_long_past_its_deadline_stops_asking_for_frames() {
        let mut n = notices();
        n.push(
            Level::Good,
            "Ping finished",
            "",
            None,
            Some(Duration::from_secs(6)),
        );
        let Some(long_ago) = n.items[0].at.checked_sub(LINGER * 4) else {
            // The clock cannot go that far back, which is only true within a
            // few seconds of boot. Nothing to say here then.
            return;
        };
        n.items[0].until = Some(long_ago);
        assert!(
            !n.any_fading(),
            "news nobody is waiting to see leave should not keep the window busy"
        );
    }

    #[test]
    fn one_that_never_expires_never_asks_for_frames() {
        let mut n = notices();
        n.push(
            Level::Error,
            "IP scan failed",
            "no route to host",
            None,
            None,
        );
        // It stays until it is dismissed, and dismissing it repaints the
        // window itself, so there is nothing to count down to.
        assert!(!n.any_fading());
    }

    #[test]
    fn only_the_newest_few_are_shown_at_once() {
        let mut n = notices();
        for i in 0..10 {
            n.push(
                Level::Info,
                format!("run {i}"),
                "",
                None,
                Some(Duration::from_secs(60)),
            );
        }
        let showing = n.showing(3);
        assert_eq!(showing.len(), 3);
        // Oldest first, so the newest ends up nearest the bell.
        assert_eq!(showing[0].title, "run 7");
        assert_eq!(showing[2].title, "run 9");
    }

    #[test]
    fn opening_the_list_is_reading_it() {
        let mut n = notices();
        n.push(Level::Warn, "a", "", None, None);
        n.push(Level::Warn, "b", "", None, None);
        assert_eq!(n.unread(), 2);
        n.mark_all_read();
        assert_eq!(n.unread(), 0);
        assert!(n.showing(8).is_empty());
        assert_eq!(n.newest_first().count(), 2);
    }
}
