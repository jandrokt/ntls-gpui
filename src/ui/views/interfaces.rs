//! The interfaces page.
//!
//! Shows every interface with an IPv4 address on it, and for each address the
//! network it belongs to, the broadcast address and the host count. Those are
//! the numbers you look up before a sweep, so the sweep button sits on the
//! same line as the network it would sweep.

use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px,
};

use crate::net::iface::Iface;
use crate::net::target::Prefix;
use crate::ui::app::App;
use crate::ui::icons::icon;
use crate::ui::menu::{Act, Item};
use crate::ui::theme::Theme;
use crate::ui::widgets::{Kind, Type, button, pill, space};

use super::page::{Page, card, page_shell};

/// Width of the label column in the fact lists. Fits "Broadcast".
const LABEL: gpui::Pixels = px(84.);

impl App {
    pub(super) fn interfaces_page(
        &mut self,
        theme: &Theme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let chosen = self.default_iface().map(str::to_string);
        let ifaces = self.ifaces.clone();

        let mut body = vec![self.machine_card(theme, cx)];
        if ifaces.is_empty() {
            body.push(nothing_attached(theme));
        }
        for i in &ifaces {
            body.push(interface_card(i, chosen.as_deref() == Some(&i.name), theme, cx));
        }

        page_shell(
            Page::Interfaces,
            theme,
            vec![
                button("ifaces-refresh", "Refresh", Kind::Normal, theme)
                    .on_click(cx.listener(|app, _, _, cx| app.refresh_interfaces(cx)))
                    .into_any_element(),
            ],
            body,
            cx,
        )
    }

    /// The machine itself: its name, the address it reaches the internet from,
    /// and which interface new tools will use.
    fn machine_card(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let outbound = crate::net::iface::outbound_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| "no route out".into());
        let chosen = self.default_iface().map(str::to_string);
        let routed = self.ifaces.iter().find(|i| i.default).map(|i| i.name.clone());

        let (sending, why) = match (&chosen, &routed) {
            (Some(name), _) => (name.clone(), "chosen here"),
            (None, Some(name)) => (name.clone(), "whatever the routing table picks"),
            (None, None) => (String::new(), "no interface to send from"),
        };
        let pinned = chosen.is_some();

        card(theme)
            .child(fact("Machine", text_value(crate::sys::hostname(), theme), theme))
            .child(fact("Reaches out", mono_value(outbound, theme), theme))
            .child(fact(
                "Tools use",
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SNUG))
                    .when(!sending.is_empty(), |d| {
                        d.child(
                            div()
                                .mono()
                                .text_small()
                                .text_color(if pinned { theme.accent } else { theme.text })
                                .child(sending),
                        )
                    })
                    .child(div().text_meta().text_color(theme.faint).child(why))
                    .when(pinned, |d| {
                        d.child(button("iface-clear", "Clear", Kind::Normal, theme).on_click(
                            cx.listener(|app, _, _, cx| app.set_field_default("iface", None, cx)),
                        ))
                    })
                    .into_any_element(),
                theme,
            ))
            .into_any_element()
    }
}

/// One interface: a heading, then a block per address on it.
fn interface_card(i: &Iface, picked: bool, theme: &Theme, cx: &mut Context<App>) -> AnyElement {
    let name = i.name.clone();
    // getifaddrs hands back 02:00:00:00:00:00 on recent macOS, so an empty
    // string here does not mean the interface has no hardware. Ask the way the
    // ARP prober asks before saying there is none.
    let mac = match (i.mac.is_empty(), i.addrs.first()) {
        (true, Some(p)) => crate::net::iface::self_mac_for(p.addr),
        _ => i.mac.clone(),
    };
    let vendor = crate::net::oui::describe(&mac);
    let addresses: Vec<AnyElement> =
        i.addrs.iter().map(|p| address_block(&name, p, theme, cx)).collect();

    card(theme)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(space::SNUG))
                .px(px(space::ROOMY))
                .py(px(space::SNUG))
                .bg(theme.raised)
                .border_b_1()
                .border_color(theme.border)
                .child(
                    div()
                        .size(px(7.))
                        .rounded_full()
                        .flex_shrink_0()
                        .bg(if i.default { theme.up } else { theme.faint }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .mono()
                        .text_body()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .truncate()
                        .child(name.clone()),
                )
                .when(picked, |d| d.child(pill("in use", theme.accent, theme.accent_soft)))
                .when(i.default, |d| d.child(pill("default route", theme.up, theme.up_soft)))
                .when(i.virtual_, |d| d.child(pill("tunnel", theme.dim, theme.track)))
                .when(!picked, |d| {
                    let pick = name.clone();
                    d.child(
                        button(
                            SharedString::from(format!("use-{name}")),
                            "Use for new tools",
                            Kind::Normal,
                            theme,
                        )
                        .on_click(cx.listener(move |app, _, _, cx| {
                            app.set_field_default("iface", Some(&pick), cx);
                        })),
                    )
                }),
        )
        .children(addresses)
        .when(!mac.is_empty(), |d| {
            let shown = if vendor.is_empty() { mac.clone() } else { format!("{mac}   {vendor}") };
            d.child(fact("Hardware", mono_value(shown, theme), theme))
        })
        .into_any_element()
}

/// One address on an interface, and what follows from it.
fn address_block(iface: &str, p: &Prefix, theme: &Theme, cx: &mut Context<App>) -> AnyElement {
    let network = p.masked();
    let hosts = network.hosts().map(|h| h.len()).unwrap_or(0);
    let address = format!("{}/{}", p.addr, p.bits);
    let (sweep_net, sweep_iface) = (network.to_string(), iface.to_string());
    let copy = p.addr.to_string();

    div()
        .id(SharedString::from(format!("addr-{iface}-{copy}")))
        .flex()
        .flex_col()
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |app, e: &gpui::MouseDownEvent, _, cx| {
                let items =
                    vec![Item::choice(format!("Copy {copy}"), "link", Act::CopyText(copy.clone()))];
                app.open_menu(e.position, items, cx);
                cx.stop_propagation();
            }),
        )
        .child(fact("Address", mono_value(address, theme), theme))
        .child(fact(
            "Network",
            div()
                .flex()
                .items_center()
                .gap(px(space::ROOMY))
                .child(div().mono().text_small().text_color(theme.text).child(network.to_string()))
                .when(hosts > 0, |d| {
                    d.child(div().text_meta().text_color(theme.faint).child(match hosts {
                        1 => "1 host".to_string(),
                        n => format!("{n} hosts"),
                    }))
                })
                .child(div().flex_1())
                .when(hosts > 0, |d| {
                    d.child(
                        button(
                            SharedString::from(format!("sweep-{sweep_net}")),
                            "Sweep",
                            Kind::Normal,
                            theme,
                        )
                        .on_click(cx.listener(move |app, _, window, cx| {
                            app.scan_network(&sweep_net, &sweep_iface, window, cx);
                        })),
                    )
                })
                .into_any_element(),
            theme,
        ))
        .children(
            network.broadcast().map(|b| fact("Broadcast", mono_value(b.to_string(), theme), theme)),
        )
        .into_any_element()
}

/// A labelled line. The labels line up down the page, so the values do too.
fn fact(label: &str, value: AnyElement, theme: &Theme) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(space::ROOMY))
        .min_h(px(32.))
        .px(px(space::ROOMY))
        .py(px(space::TIGHT / 2.))
        .border_b_1()
        .border_color(theme.rule)
        .child(
            div()
                .w(LABEL)
                .flex_shrink_0()
                .text_meta()
                .text_color(theme.faint)
                .child(label.to_string()),
        )
        .child(div().flex_1().min_w_0().child(value))
        .into_any_element()
}

fn mono_value(text: String, theme: &Theme) -> AnyElement {
    div().mono().text_small().text_color(theme.text).truncate().child(text).into_any_element()
}

fn text_value(text: String, theme: &Theme) -> AnyElement {
    div().text_small().text_color(theme.text).truncate().child(text).into_any_element()
}

fn nothing_attached(theme: &Theme) -> AnyElement {
    card(theme)
        .px(px(space::ROOMY))
        .py(px(space::ROOMY))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(space::SNUG))
                .child(icon("globe", px(14.), theme.faint))
                .child(div().text_small().text_color(theme.dim).child(
                    "No interface has an IPv4 address. Ones that are up but unaddressed are left out.",
                )),
        )
        .into_any_element()
}
