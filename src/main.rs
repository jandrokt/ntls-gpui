//! ntls: a desktop toolbox for the network questions you ask most often. Is
//! this host up, what else is on this network, and what is it listening on.

// A Windows executable declares which subsystem it wants, and the default is
// the console one: Windows then opens a console window next to the actual
// window, and the program appears to run in two. Saying `windows` here is how
// a program says it draws its own window and needs no console.
//
// Only in a release build. A debug build keeps the console, because that is
// where a panic message and everything printed while working goes, and losing
// it would mean a crash on Windows left nothing behind to read.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod core;
mod dl;
mod doc;
mod expr;
mod flow;
mod net;
mod sys;
mod tools;
mod ui;

use gpui::{
    App, AppContext, Application, Bounds, Menu, MenuItem, TitlebarOptions, WindowBounds,
    WindowOptions, point, px, size,
};

use ui::app::App as Ntls;

fn main() {
    // A wide connect scan needs far more descriptors than the default soft
    // limit allows, and failing to raise it is not worth refusing to start
    // over: the scan still runs, just less concurrently.
    net::portscan::raise_file_limit();

    Application::new().with_assets(ui::icons::Assets).run(|cx: &mut App| {
        ui::app::bind_keys(cx);
        ui::text_input::bind_keys(cx);
        ui::editor::bind_keys(cx);
        cx.on_action(|_: &ui::app::Quit, cx| cx.quit());
        cx.set_menus(menus());

        let bounds = Bounds::centered(None, size(px(1180.), px(760.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(880.), px(560.))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("ntls".into()),
                        // The strip ntls draws across the top is the titlebar,
                        // so the system's own is hidden and what would have
                        // been in it is drawn into the strip: the traffic
                        // lights on macOS, which the system still draws and
                        // which are only nudged down to sit level with it, and
                        // the minimise, maximise and close buttons on Windows,
                        // which ntls draws itself.
                        //
                        // Linux keeps the titlebar its desktop draws. There is
                        // no one set of window buttons to stand in for there.
                        appears_transparent: cfg!(any(
                            target_os = "macos",
                            target_os = "windows"
                        )),
                        // Sized and placed from one set of numbers, so the
                        // strip ntls draws and the buttons the system draws
                        // on top of it agree about where the middle is.
                        traffic_light_position: Some(point(
                            px(sys::TRAFFIC_LIGHT_LEFT),
                            px(sys::TRAFFIC_LIGHT_TOP),
                        )),
                    }),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Ntls::new(window, cx)),
            )
            .expect("cannot open a window");

        // Workspaces are written as they change, but a last pass on the way
        // out catches anything a run finished a moment ago.
        let closing = window;
        cx.on_app_quit(move |cx| {
            closing.update(cx, |view: &mut Ntls, _, _| view.save_all()).ok();
            async {}
        })
        .detach();

        cx.activate(true);
        window
            .update(cx, |view, window, cx| {
                window.focus(&view.focus_handle);
                cx.notify();
            })
            .ok();
    });
}

fn menus() -> Vec<Menu> {
    vec![
        Menu {
            name: "ntls".into(),
            items: vec![
                MenuItem::action("Settings…", ui::app::Preferences),
                MenuItem::separator(),
                MenuItem::action("Quit ntls", ui::app::Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("Save", ui::app::Save),
            ],
        },
        Menu {
            name: "Workspace".into(),
            items: vec![
                MenuItem::action("New Workspace", ui::app::NewWorkspace),
                MenuItem::action("Close Workspace", ui::app::CloseWorkspace),
                MenuItem::separator(),
                MenuItem::action("Next Workspace", ui::app::NextWorkspace),
                MenuItem::action("Previous Workspace", ui::app::PrevWorkspace),
            ],
        },
        Menu {
            name: "Tool".into(),
            items: vec![
                MenuItem::action("Add Tool…", ui::app::AddTool),
                MenuItem::action("Run", ui::app::Run),
                MenuItem::action("Stop", ui::app::Stop),
                MenuItem::action("Show Form", ui::app::Settings),
                MenuItem::action("Name and Colour", ui::app::ToggleProperties),
                MenuItem::action("Note on Selection", ui::app::FocusNote),
                MenuItem::separator(),
                MenuItem::action("Close Tool", ui::app::CloseJob),
                MenuItem::action("Next Tool", ui::app::NextJob),
                MenuItem::action("Previous Tool", ui::app::PrevJob),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Filter Results", ui::app::FocusFilter),
                MenuItem::action("Toggle Log", ui::app::ToggleLog),
                MenuItem::action("Toggle Chart", ui::app::ToggleChart),
                MenuItem::action("Toggle Theme", ui::app::ToggleTheme),
                MenuItem::action("Notifications", ui::app::ShowNotices),
            ],
        },
    ]
}
