//! The settings page: everything ntls remembers about itself.
//!
//! A page and not a sheet, because these get read about as often as they get
//! changed: what interface am I on, where do my workspaces live.
//!
//! What a new tool starts on is stored per field, not per tool, so this page
//! needs no edit when a tool is added. It names `iface` and `method`; any tool
//! declaring one picks it up.

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::ui::app::App;
use crate::ui::store::ThemeChoice;
use crate::ui::theme::Theme;
use crate::ui::widgets::{Kind, Segment, Segments, Type, button, pill, space, switch};

use super::page::{Page, page_shell, section, setting};

impl App {
    pub(super) fn settings_page(
        &mut self,
        theme: &Theme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body = vec![
            self.appearance_section(theme, cx),
            self.scanning_section(theme, cx),
            self.results_section(theme, cx),
            self.notifications_section(theme, cx),
            self.files_section(theme, cx),
        ];
        page_shell(Page::Settings, theme, Vec::new(), body, cx)
    }

    // --- appearance ---------------------------------------------------------

    fn appearance_section(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let chosen = self.settings.theme;
        let choices: Vec<(SharedString, bool, ThemeChoice)> = ThemeChoice::ALL
            .into_iter()
            .map(|c| (SharedString::from(c.label()), c == chosen, c))
            .collect();
        let remember = self.settings.remember_layout;
        let theme_choice = choose(
            self,
            "theme",
            choices,
            theme,
            cx,
            |app: &mut App, choice: ThemeChoice, window: &mut Window, cx| {
                app.set_theme_choice(choice, window, cx)
            },
        );
        let animate = self.settings.animate;

        section(
            "Appearance",
            theme,
            vec![
                setting(
                    "Theme",
                    "System follows the desktop and changes when it does.",
                    theme,
                    theme_choice,
                ),
                setting(
                    "Animate",
                    "Sliding switches, pages that fade in, notices that rise. Off makes all of it instant.",
                    theme,
                    toggle("animate", animate, theme, cx, |app, on, _, cx| {
                        app.settings.animate = on;
                        app.save_settings();
                        cx.notify();
                    }),
                ),
                setting(
                    "Remember the side bar",
                    "Reopen it at the width and on the view it was left on.",
                    theme,
                    toggle("remember-layout", remember, theme, cx, |app, on, _, cx| {
                        app.settings.remember_layout = on;
                        if on {
                            app.remember_layout();
                        }
                        app.save_settings();
                        cx.notify();
                    }),
                ),
            ],
        )
    }

    // --- reading what came back ---------------------------------------------

    fn results_section(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let follow = self.settings.follow_results;
        let keep = self.settings.default_for("keep") == Some("true");

        section(
            "Results",
            theme,
            vec![
                setting(
                    "Follow new results",
                    "While you are at one end of the table, stay there as rows arrive.",
                    theme,
                    toggle("follow", follow, theme, cx, |app, on, _, cx| {
                        app.settings.follow_results = on;
                        app.save_settings();
                        cx.notify();
                    }),
                ),
                setting(
                    "Keep earlier results",
                    "New scans add to their table instead of replacing it.",
                    theme,
                    toggle("keep", keep, theme, cx, |app, on, _, cx| {
                        app.set_field_default("keep", Some(if on { "true" } else { "false" }), cx);
                    }),
                ),
            ],
        )
    }

    // --- what a new tool starts on ------------------------------------------

    fn scanning_section(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        // The choices come from the fields themselves, so a method or an
        // interface added to the tools shows up here without this page
        // knowing anything about it.
        let ifaces: Vec<(SharedString, bool, Option<String>)> =
            std::iter::once(("Default route".into(), self.default_iface().is_none(), None))
                .chain(self.ifaces.iter().map(|i| {
                    (
                        SharedString::from(i.name.clone()),
                        self.default_iface() == Some(i.name.as_str()),
                        Some(i.name.clone()),
                    )
                }))
                .collect();

        let current_method = self
            .settings
            .default_for("method")
            .unwrap_or("")
            .to_string();
        let methods: Vec<(SharedString, bool, Option<String>)> =
            crate::tools::prober::method_field()
                .options
                .into_iter()
                .map(|o| {
                    let picked = o.value == current_method
                        || (current_method.is_empty()
                            && o.value == crate::tools::prober::METHOD_ICMP);
                    (SharedString::from(o.label), picked, Some(o.value))
                })
                .collect();

        let resolve = self.settings.default_for("resolve") != Some("false");
        let vendors = self.settings.default_for("vendors") != Some("false");
        let timeouts: Vec<(SharedString, bool, &'static str)> = ["500ms", "1s", "2s", "5s"]
            .into_iter()
            .map(|t| {
                let picked = self.settings.default_for("timeout").unwrap_or("1s") == t;
                (SharedString::from(t), picked, t)
            })
            .collect();
        let concurrencies: Vec<(SharedString, bool, &'static str)> = ["32", "64", "128", "256"]
            .into_iter()
            .map(|n| {
                let picked = self.settings.default_for("concurrency").unwrap_or("64") == n;
                (SharedString::from(n), picked, n)
            })
            .collect();
        let timeout_choice = choose(
            self,
            "timeout",
            timeouts,
            theme,
            cx,
            |app: &mut App, t: &'static str, _: &mut Window, cx| {
                app.set_field_default("timeout", Some(t), cx);
            },
        );
        let concurrency_choice = choose(
            self,
            "concurrency",
            concurrencies,
            theme,
            cx,
            |app: &mut App, n: &'static str, _: &mut Window, cx| {
                app.set_field_default("concurrency", Some(n), cx);
            },
        );
        let iface_choice = choose(
            self,
            "iface",
            ifaces,
            theme,
            cx,
            |app: &mut App, name: Option<String>, _: &mut Window, cx| {
                app.set_field_default("iface", name.as_deref(), cx);
            },
        );
        let method_choice = choose(
            self,
            "method",
            methods,
            theme,
            cx,
            |app: &mut App, value: Option<String>, _: &mut Window, cx| {
                app.set_field_default("method", value.as_deref(), cx);
            },
        );

        section(
            "Scanning",
            theme,
            vec![
                setting(
                    "Interface",
                    "Which interface new tools send from. Every tool with an interface setting starts here.",
                    theme,
                    iface_choice,
                ),
                setting(
                    "Reachability",
                    "How new pings and sweeps ask. ARP is local only, and finds what ignores ICMP.",
                    theme,
                    method_choice,
                ),
                setting(
                    "Resolve names",
                    "Look up the reverse DNS name of everything that answers.",
                    theme,
                    toggle("resolve", resolve, theme, cx, |app, on, _, cx| {
                        app.set_field_default(
                            "resolve",
                            Some(if on { "true" } else { "false" }),
                            cx,
                        );
                    }),
                ),
                setting(
                    "Identify hardware",
                    "Resolve the MAC and vendor of hosts on this link.",
                    theme,
                    toggle("vendors", vendors, theme, cx, |app, on, _, cx| {
                        app.set_field_default(
                            "vendors",
                            Some(if on { "true" } else { "false" }),
                            cx,
                        );
                    }),
                ),
                setting(
                    "Timeout",
                    "How long a host is given to answer. Raise it on a slow or lossy link.",
                    theme,
                    timeout_choice,
                ),
                setting("At a time", "How many hosts a sweep probes at once.", theme, concurrency_choice),
            ],
        )
    }

    // --- what the application says ------------------------------------------

    fn notifications_section(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let (toasts, runs, panel) = (
            self.settings.toasts,
            self.settings.notify_runs,
            self.settings.open_panel_on_failure,
        );
        let seconds = self.settings.toast_seconds;
        let durations: Vec<(SharedString, bool, u64)> = [3u64, 6, 12, 30]
            .into_iter()
            .map(|s| (SharedString::from(format!("{s}s")), s == seconds, s))
            .collect();
        let stay_choice =
            choose(self, "toast-seconds", durations, theme, cx, |app: &mut App, s: u64, _: &mut Window, cx| {
                app.settings.toast_seconds = s;
                app.save_settings();
                cx.notify();
            });

        section(
            "Notifications",
            theme,
            vec![
                setting(
                    "Show in the corner",
                    "Failures stay there until you dismiss them, whatever this is set to.",
                    theme,
                    toggle("toasts", toasts, theme, cx, |app, on, _, cx| {
                        app.settings.toasts = on;
                        app.save_settings();
                        cx.notify();
                    }),
                ),
                setting(
                    "Say when a run finishes",
                    "Only for runs you are not watching. Failures always say so.",
                    theme,
                    toggle("notify-runs", runs, theme, cx, |app, on, _, cx| {
                        app.settings.notify_runs = on;
                        app.save_settings();
                        cx.notify();
                    }),
                ),
                setting(
                    "How long they stay",
                    "",
                    theme,
                    stay_choice,
                ),
                setting(
                    "Open the output panel on failure",
                    "Failures usually explain themselves in the log.",
                    theme,
                    toggle("panel-on-failure", panel, theme, cx, |app, on, _, cx| {
                        app.settings.open_panel_on_failure = on;
                        app.save_settings();
                        cx.notify();
                    }),
                ),
            ],
        )
    }

    // --- where things live ---------------------------------------------------

    fn files_section(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let root = crate::ui::store::root();
        let shown = root.display().to_string();
        let reveal = root.clone();
        let closed = self.settings.closed.len();

        let mut rows = vec![setting(
            "Workspace folder",
            "A workspace is a directory in here, and every tool in it is a file.",
            theme,
            div()
                .flex()
                .items_center()
                .gap(px(space::SNUG))
                .child(
                    div()
                        .max_w(px(300.))
                        .mono()
                        .text_small()
                        .text_color(theme.dim)
                        .truncate()
                        .child(shown),
                )
                .child(
                    button("reveal-root", "Reveal", Kind::Normal, theme)
                        .on_click(cx.listener(move |app, _, _, _| app.reveal_path(&reveal))),
                )
                .into_any_element(),
        )];

        if closed > 0 {
            rows.push(setting(
                "Closed workspaces",
                "Still on disk and untouched, just not opened at startup.",
                theme,
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SNUG))
                    .child(pill(format!("{closed}"), theme.dim, theme.track))
                    .child(
                        button("forget-closed", "Open them all again", Kind::Normal, theme)
                            .on_click(cx.listener(|app, _, _, cx| app.reopen_closed(cx))),
                    )
                    .into_any_element(),
            ));
        }

        rows.push(setting(
            "Build",
            "ntls counts builds, not versions. Stamped in when the binary was made.",
            theme,
            div()
                .mono()
                .text_small()
                .text_color(theme.dim)
                .child(crate::sys::build_label())
                .into_any_element(),
        ));

        section("Files", theme, rows)
    }
}

/// A row of choices, one of them current.
///
/// The light slides between them and does not go out here and come on
/// there: that is what makes it read as one setting with several positions
/// instead of several settings of which one happens to be lit.
fn choose<T: Clone + 'static>(
    app: &mut App,
    id: &str,
    options: Vec<(SharedString, bool, T)>,
    theme: &Theme,
    cx: &mut Context<App>,
    pick: impl Fn(&mut App, T, &mut Window, &mut Context<App>) + Clone + 'static,
) -> AnyElement {
    let current = options.iter().position(|(_, on, _)| *on).unwrap_or(0);
    let from = app.segment_from(id, current);
    let options = options
        .into_iter()
        .map(|(label, _, value)| {
            let pick = pick.clone();
            Segment::new(
                label,
                cx.listener(move |app: &mut App, _, window: &mut Window, cx| {
                    pick(app, value.clone(), window, cx)
                }),
            )
        })
        .collect();
    Segments { id: format!("set-{id}"), options, current, from }.render(theme)
}

/// A switch, and what happens when it is thrown.
fn toggle(
    id: &str,
    on: bool,
    theme: &Theme,
    cx: &mut Context<App>,
    set: impl Fn(&mut App, bool, &mut Window, &mut Context<App>) + 'static,
) -> AnyElement {
    div()
        .id(SharedString::from(format!("set-{id}")))
        .flex()
        .items_center()
        .h(px(28.))
        .cursor_pointer()
        .child(switch(id, on, theme))
        .on_click(cx.listener(move |app, _, window, cx| set(app, !on, window, cx)))
        .into_any_element()
}
