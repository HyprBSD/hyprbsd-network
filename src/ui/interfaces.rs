//! The Interfaces section: a row per network card with its switch, and a card's settings.

use super::{
    Action, HEADER, Root, button, checkbox, follow_focus, message, one_line, right_row, sense, small_button, switch,
    text_field, with_id,
};
use crate::cards::{Addressing, Card, Cards, Kind, Manual, State, ifconfig_var, lagg_words};
use crate::theme::{self, BACKGROUND, DIM, FAINT, ORANGE, RADIUS, RED, SURFACE, SURFACE_HOVER, TEAL, TEXT};
use eframe::egui::{self, Align, Id, Layout, Margin, RichText, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2};

/// Where a row's text starts.
const TEXT_LEFT: f32 = 16.0;
/// How wide the labels before a card's fields are.
const LABEL: f32 = 72.0;
/// How tall a card's fields are.
const FIELD: f32 = 28.0;
/// The lagg modes a lagg's page offers: lagg(4)'s word, and what the page says.
const MODES: [(&str, &str); 4] = [
    ("failover", "Failover: the first card, then the next when it goes"),
    ("lacp", "LACP: every card at once, through a switch that does LACP"),
    ("loadbalance", "Load balance: every card at once, by connection"),
    ("roundrobin", "Round robin: every card at once, in turn"),
];

/// A card's settings as typed.
#[derive(Debug, Default)]
pub(super) struct Page {
    pub card: String,
    pub manual: bool,
    pub address: String,
    pub prefix: String,
    pub gateway: String,
    pub dns: String,
    /// What the address field shows while it's empty: DHCP's address, for an Automatic card.
    pub address_hint: Option<String>,
    /// Why Apply didn't, or what the helper said.
    pub error: Option<String>,
    /// A lagg's mode and ports.
    pub lagg: Option<LaggForm>,
}

/// A lagg's mode and ports as picked on its page.
#[derive(Debug, Default)]
pub(super) struct LaggForm {
    /// The lagg isn't there yet: Create makes it.
    pub new: bool,
    pub proto: Option<&'static str>,
    /// In the order ticked; failover's main card first.
    pub ports: Vec<String>,
    /// Automatic or Manual has been picked.
    pub picked: bool,
    /// Its first port as rc.conf has it, whose row has the keyboard after Remove.
    pub first_port: Option<String>,
}

impl LaggForm {
    /// The root helper's `lagg` arguments before the address, or what's still to pick.
    fn args(&self, name: &str) -> Result<Vec<String>, String> {
        let proto = self.proto.ok_or("Pick a mode.")?;
        if self.ports.len() < 2 {
            return Err("Tick two cards or more.".to_owned());
        }
        if !self.picked {
            return Err("Pick Automatic or Manual.".to_owned());
        }
        Ok(vec!["lagg".to_owned(), name.to_owned(), proto.to_owned(), self.ports.join(",")])
    }
}

impl Page {
    /// A new lagg's page, under a name no card has; nothing picked.
    pub fn new_lagg(cards: &Cards) -> Page {
        let lagg = LaggForm { new: true, ..LaggForm::default() };
        Page { card: cards.free_lagg(), lagg: Some(lagg), ..Page::default() }
    }

    /// The settings of `name` as rc.conf has them; none for a card the page doesn't open. An
    /// Automatic card's Manual settings start from what DHCP gave it, but for the address, which
    /// the router may give to another computer later: that's the address field's hint. A lagg's
    /// page starts from its mode and ports.
    pub fn open(cards: &Cards, name: &str) -> Option<Page> {
        let card = cards.cards.iter().find(|c| c.name == name).filter(|c| opens(c))?;
        let mut page = Page { card: name.to_owned(), ..Page::default() };
        if card.kind == Kind::Lagg {
            let (proto, ports) = lagg_words(&card.settings.value);
            let proto = MODES.iter().map(|(word, _)| *word).find(|word| Some(*word) == proto);
            let ports: Vec<String> = ports.into_iter().map(str::to_owned).collect();
            let first_port = ports.first().cloned();
            page.lagg = Some(LaggForm { new: false, proto, ports, picked: true, first_port });
        }
        let prefix = match card.settings.addressing {
            Addressing::Manual { address, prefix } => {
                page.manual = true;
                page.address = address.to_string();
                prefix
            }
            Addressing::Automatic => {
                let lease = card.inet.first();
                page.address_hint = lease.map(|(address, _)| format!("{address} (from DHCP)"));
                lease.map(|&(_, len)| len)
            }
            _ => return Some(page),
        };
        page.prefix = prefix.map(|p| p.to_string()).unwrap_or_default();
        page.gateway = cards.gateway(name).map(|g| g.to_string()).unwrap_or_default();
        page.dns = card.dns.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
        Some(page)
    }

    /// The helper run Apply or Create asks for, or why it can't.
    fn apply(&self) -> Result<Root, String> {
        let mut args = match &self.lagg {
            Some(lagg) => lagg.args(&self.card)?,
            None => vec!["iface".to_owned(), self.card.clone()],
        };
        if self.manual {
            args.extend(Manual::check(&self.address, &self.prefix, &self.gateway, &self.dns)?.args());
        } else {
            args.push("dhcp".to_owned());
        }
        Ok(Root::Apply { card: self.card.clone(), args })
    }

    /// The helper run that removes a lagg; its first port's row has the keyboard after.
    fn remove(&self) -> Root {
        let card = self.lagg.as_ref().and_then(|l| l.first_port.as_ref()).unwrap_or(&self.card).clone();
        Root::Apply { card, args: ["lagg", &self.card, "remove"].map(str::to_owned).to_vec() }
    }
}

/// The id of a card's row, the same in every frame.
pub(super) fn row_id(name: &str) -> Id {
    Id::new(("iface", name))
}

/// Whether a card has a switch: one with a driver and an interface, not a lagg or part of a
/// bridge or lagg.
fn switchable(card: &Card) -> bool {
    matches!(card.state, State::Connected | State::NoLink | State::Off)
        && card.part_of.is_none()
        && card.kind != Kind::Lagg
}

/// Whether a card has settings to open: one with an interface (not Wi-Fi turned off, whose wlan
/// isn't there to set up) that isn't part of a bridge or lagg.
fn opens(card: &Card) -> bool {
    card.has_interface() && card.part_of.is_none()
}

/// What a row says under the card's name.
fn caption(card: &Card) -> String {
    let kind = match card.kind {
        Kind::Wired => "Wired".to_owned(),
        Kind::Wifi => "Wi-Fi".to_owned(),
        Kind::Usb => "USB".to_owned(),
        Kind::Lagg if card.members.is_empty() => "Lagg".to_owned(),
        Kind::Lagg => format!("Lagg of {}", card.members.join(", ")),
    };
    let state = match (&card.state, card.part_of.as_deref()) {
        (_, Some(parent)) => format!("Part of {parent}"),
        (State::Connected, _) => match card.inet.first() {
            Some((address, len)) => format!("Connected, {address}/{len}"),
            None => "Connected, no address yet".to_owned(),
        },
        (State::NoLink, _) => match card.kind {
            Kind::Wired => "Cable unplugged",
            Kind::Wifi => "Not connected",
            Kind::Usb | Kind::Lagg => "No link",
        }
        .to_owned(),
        (State::Off, _) => "Off".to_owned(),
        (State::NotSetUp, _) => "Not set up; Wi-Fi sets it up".to_owned(),
        (State::NoDriver(_), _) => "FreeBSD has no driver for it".to_owned(),
    };
    format!("{kind} · {state}")
}

/// The helper run that turns a card on or off: the Wi-Fi switch's for a wlan.
fn switched(card: &Card, on: bool) -> Root {
    match (card.kind, on) {
        (Kind::Wifi, true) => Root::WifiOn,
        (Kind::Wifi, false) => Root::WifiOff,
        _ => Root::Switch { card: card.name.clone(), on },
    }
}

/// A row per card, and New lagg when two cards or more can be a lagg's; what was asked for.
pub(super) fn list(ui: &mut Ui, cards: &Cards, root: Option<&Root>) -> Option<Action> {
    let mut action = None;
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        if cards.cards.is_empty() {
            ui.label(RichText::new("No network cards.").color(DIM));
        }
        for card in &cards.cards {
            if let Some(asked) = row(ui, card, root) {
                action = Some(asked);
            }
        }
        if cards.ports_for(&cards.free_lagg()).nth(1).is_some()
            && button(ui, Id::new("new-lagg"), "New lagg", FAINT, TEXT, root.is_none()).clicked()
        {
            action = Some(Action::NewLagg);
        }
        follow_focus(ui);
    });
    action
}

/// A card's row: its name and state, and its switch; a click or Enter opens its settings.
fn row(ui: &mut Ui, card: &Card, root: Option<&Root>) -> Option<Action> {
    let opens = opens(card);
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), HEADER));
    let response = ui.interact(rect, row_id(&card.name), sense(opens && root.is_none()));
    let hovered = response.hovered() && opens;
    ui.painter().rect_filled(rect, RADIUS, if hovered { SURFACE_HOVER } else { SURFACE });
    let switchable = switchable(card);
    let width = rect.width() - TEXT_LEFT - if switchable { 72.0 } else { 16.0 };
    let title = match &card.state {
        State::NoDriver(model) => model.as_str(),
        _ => card.name.as_str(),
    };
    let name = one_line(ui, title, theme::semibold(15.0), TEXT, width);
    ui.painter().galley(pos2(rect.left() + TEXT_LEFT, rect.top() + 9.0), name, TEXT);
    let color = if card.state == State::Connected && card.part_of.is_none() { TEAL } else { DIM };
    let line = one_line(ui, &caption(card), theme::body(13.0), color, width);
    ui.painter().galley(pos2(rect.left() + TEXT_LEFT, rect.top() + 31.0), line, color);
    if response.has_focus() {
        ui.painter().rect_stroke(rect.shrink(1.0), RADIUS, Stroke::new(1.5, ORANGE), StrokeKind::Inside);
    }
    let mut action = response.clicked().then(|| Action::Open(card.name.clone()));
    if switchable {
        let on = card.state != State::Off;
        let at = egui::Rect::from_center_size(pos2(rect.right() - 38.0, rect.center().y), vec2(44.0, 24.0));
        // A child of its own, so the switch takes no room in the list's layout.
        let mut child = ui.new_child(UiBuilder::new().max_rect(at).id(row_id(&card.name).with("switch")));
        if switch(&mut child, &card.name, on, root.is_none()).clicked() {
            action = Some(Action::Root(switched(card, !on)));
        }
    }
    action
}

/// A card's settings: Back and its name, its state, a lagg's mode and ports, Automatic or
/// Manual, and Apply (Create for a new lagg, and Remove for a lagg); what was asked for.
pub(super) fn page(ui: &mut Ui, cards: &Cards, page: &mut Page, root: Option<&Root>) -> Option<Action> {
    let mut action = None;
    let applying = matches!(root, Some(Root::Apply { .. }));
    let id = row_id(&page.card).with("page");
    let new = page.lagg.as_ref().is_some_and(|l| l.new);
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.horizontal(|ui| {
            if small_button(ui, id.with("back"), "Back", true).clicked() && !applying {
                action = Some(Action::Back);
            }
            ui.label(RichText::new(&page.card).font(theme::semibold(17.0)).color(TEXT));
        });
        if new {
            ui.label(RichText::new("New lagg").font(theme::body(13.0)).color(DIM));
        } else {
            let Some(card) = cards.cards.iter().find(|c| c.name == page.card) else {
                message(ui, &format!("{} isn't there any more.", page.card));
                return;
            };
            ui.label(RichText::new(caption(card)).font(theme::body(13.0)).color(DIM));
            if !opens(card) {
                let why = if card.kind == Kind::Wifi && card.state == State::Off {
                    "Wi-Fi is off: switch it on to change its settings."
                } else {
                    "This card's settings can't be changed now."
                };
                message(ui, why);
                return;
            }
            if card.settings.addressing == Addressing::ByHand {
                let line = format!("{}=\"{}\"", ifconfig_var(&card.name), card.settings.value);
                message(ui, &format!("Set up by hand in rc.conf:\n{line}\nThis window leaves it as it is."));
                return;
            }
        }
        if let Some(lagg) = &mut page.lagg {
            lagg_form(ui, cards, &page.card, lagg, id);
        }
        let picked = page.lagg.as_ref().is_none_or(|l| l.picked);
        ui.horizontal(|ui| {
            for (manual, key, label) in [(false, "automatic", "Automatic (DHCP)"), (true, "manual", "Manual")] {
                if with_id(ui, id.with(key), |ui| ui.radio(picked && page.manual == manual, label)).clicked() {
                    page.manual = manual;
                    if let Some(lagg) = &mut page.lagg {
                        lagg.picked = true;
                    }
                }
            }
        });
        let mut submitted = false;
        if page.manual {
            let address_hint = page.address_hint.clone().unwrap_or_else(|| "192.168.1.5".to_owned());
            for (label, hint, text) in [
                ("Address", address_hint.as_str(), &mut page.address),
                ("Prefix", "24, or 255.255.255.0", &mut page.prefix),
                ("Gateway", "None, or 192.168.1.1", &mut page.gateway),
                ("DNS", "Up to three: 1.1.1.1, 9.9.9.9", &mut page.dns),
            ] {
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(vec2(LABEL, FIELD), Layout::left_to_right(Align::Center), |ui| {
                        ui.set_min_size(vec2(LABEL, FIELD));
                        ui.label(RichText::new(label).font(theme::body(13.0)).color(DIM));
                    });
                    let field = text_field(text, hint, false).margin(Margin::symmetric(8, 5));
                    let field = with_id(ui, id.with(label), |ui| ui.add(field));
                    submitted |= field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                });
            }
            if !page.gateway.trim().is_empty()
                && let Some(warning) = cards.takes_over(&page.card)
            {
                ui.label(RichText::new(warning).font(theme::body(13.0)).color(ORANGE));
            }
        } else if let Some(gateway) = cards.gateway(&page.card) {
            ui.label(RichText::new(format!("Gateway {gateway}, from DHCP")).font(theme::body(13.0)).color(DIM));
        }
        if let Some(error) = &page.error {
            ui.label(RichText::new(error.as_str()).font(theme::body(13.0)).color(RED));
        }
        let idle = root.is_none();
        right_row(ui, |ui| {
            let label = match (applying, new) {
                (true, _) => "Applying…",
                (false, true) => "Create",
                (false, false) => "Apply",
            };
            if button(ui, id.with("apply"), label, ORANGE, BACKGROUND, idle).clicked() || (submitted && idle) {
                match page.apply() {
                    Ok(root) => action = Some(Action::Root(root)),
                    Err(why) => page.error = Some(why),
                }
            }
            if page.lagg.is_some() && !new && button(ui, id.with("remove"), "Remove", FAINT, TEXT, idle).clicked() {
                action = Some(Action::Root(page.remove()));
            }
        });
        follow_focus(ui);
    });
    action
}

/// A lagg's mode, and the cards to be its ports ticked in order, on its page.
fn lagg_form(ui: &mut Ui, cards: &Cards, name: &str, lagg: &mut LaggForm, id: Id) {
    let note = |ui: &mut Ui, text: &str| ui.label(RichText::new(text).font(theme::body(13.0)).color(DIM));
    note(ui, "Mode");
    for (word, label) in MODES {
        if with_id(ui, id.with(word), |ui| ui.radio(lagg.proto == Some(word), label)).clicked() {
            lagg.proto = Some(word);
        }
    }
    note(ui, "Cards, in the order ticked");
    for card in cards.ports_for(name) {
        let at = lagg.ports.iter().position(|p| *p == card.name);
        let label = at.map_or_else(|| card.name.clone(), |n| format!("{} ({})", card.name, n + 1));
        let mut ticked = at.is_some();
        if with_id(ui, id.with(("port", &card.name)), |ui| checkbox(ui, &mut ticked, &label, true)).changed() {
            match at {
                Some(n) => {
                    lagg.ports.remove(n);
                }
                None => lagg.ports.push(card.name.clone()),
            }
        }
    }
    note(ui, "Failover needs every card on the same network.");
    let ports: Vec<&Card> = lagg.ports.iter().filter_map(|p| cards.cards.iter().find(|c| c.name == *p)).collect();
    let wired: Vec<&str> = ports.iter().filter(|c| c.kind != Kind::Wifi).map(|c| c.name.as_str()).collect();
    if let Some(wlan) = ports.iter().find(|c| c.kind == Kind::Wifi)
        && !wired.is_empty()
    {
        let takes = if wired.len() == 1 { "takes" } else { "take" };
        note(ui, &format!("{} {takes} {}'s MAC address, which a lagg with Wi-Fi needs.", wired.join(", "), wlan.name));
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{draw, draw_tall, key, press, text, themed, wifi_window};
    use super::super::{Ended, Network, Section};
    use super::*;
    use crate::cards::{Settings, parse_settings};
    use crate::iface::Route;
    use std::net::Ipv4Addr;
    use std::time::Duration;

    fn card(name: &str, kind: Kind, state: State, inet: &[([u8; 4], u32)], value: &str) -> Card {
        Card {
            name: name.into(),
            kind,
            state,
            inet: inet.iter().map(|(a, l)| (Ipv4Addr::from(*a), *l)).collect(),
            settings: parse_settings(value),
            dns: Vec::new(),
            part_of: None,
            members: Vec::new(),
        }
    }

    /// lagg0 of re1 and wlan1, connected.
    fn lagg() -> Card {
        let value = "laggproto failover laggport re1 laggport wlan1 DHCP";
        let mut lagg0 = card("lagg0", Kind::Lagg, State::Connected, &[([192, 168, 1, 30], 24)], value);
        lagg0.members = vec!["re1".into(), "wlan1".into()];
        lagg0
    }

    /// em0 manual and connected with the computer's gateway, re0 automatic and unplugged, ue0
    /// off, wlan0 joined, a bridge member, a card without a driver, a lagg.
    fn cards() -> Cards {
        let mut em0 = card("em0", Kind::Wired, State::Connected, &[([192, 168, 1, 5], 24)], "inet 192.168.1.5/24");
        em0.dns = vec![Ipv4Addr::new(192, 0, 2, 53)];
        let mut em1 = card("em1", Kind::Wired, State::Connected, &[], "up");
        em1.part_of = Some("vm-public".into());
        let unset = Settings { addressing: Addressing::Unset, off: false, value: String::new() };
        let none3 =
            Card { settings: unset, ..card("none3", Kind::Wired, State::NoDriver("Realtek RTL8125".into()), &[], "") };
        Cards {
            cards: vec![
                em0,
                card("re0", Kind::Wired, State::NoLink, &[], "DHCP"),
                card("ue0", Kind::Usb, State::Off, &[], "DHCP NOAUTO"),
                card("wlan0", Kind::Wifi, State::Connected, &[([192, 168, 1, 20], 24)], "WPA DHCP"),
                card("em2", Kind::Wired, State::Connected, &[], "DHCP description office"),
                em1,
                none3,
                lagg(),
            ],
            defaultrouter: Some(Ipv4Addr::new(192, 168, 1, 1)),
            route: Some(Route { iface: "em0".into(), gateway: Some(Ipv4Addr::new(192, 168, 1, 1)) }),
        }
    }

    /// The window on its Interfaces section, with `cards`.
    fn window() -> Network {
        let mut network = wifi_window();
        network.section = Section::Interfaces;
        network.cards = Some(Ok(cards()));
        network.cards_reader.at = f64::MAX;
        network
    }

    /// The window with `name`'s settings open.
    fn opened(name: &str) -> Network {
        let mut network = window();
        network.page = Page::open(&cards(), name);
        network
    }

    /// What clicking the text `label` asks for, in a window tall enough for a lagg's page.
    fn click(ctx: &egui::Context, network: &mut Network, label: &str) -> Vec<Action> {
        let (texts, _) = draw_tall(ctx, network, Vec::new());
        let at = text(&texts, label).unwrap_or_else(|| panic!("no {label} in {texts:?}")).center();
        draw_tall(ctx, network, press(at, true));
        draw_tall(ctx, network, press(at, false)).1
    }

    #[test]
    fn each_card_has_a_row_with_its_state() {
        let (texts, _) = draw(&themed(), &mut window(), Vec::new());

        let rows = [
            "Wired · Connected, 192.168.1.5/24",
            "Wired · Cable unplugged",
            "USB · Off",
            "Wi-Fi · Connected, 192.168.1.20/24",
            "Wired · Part of vm-public",
            "Wired · FreeBSD has no driver for it",
            "Lagg of re1, wlan1 · Connected, 192.168.1.30/24",
        ];
        let missing: Vec<_> = rows.iter().filter(|r| text(&texts, r).is_none()).collect();
        assert!(missing.is_empty(), "{missing:?} not in {texts:?}");
    }

    #[test]
    fn a_card_without_a_driver_is_named_by_its_model() {
        let (texts, _) = draw(&themed(), &mut window(), Vec::new());

        assert!(text(&texts, "Realtek RTL8125").is_some(), "{texts:?}");
    }

    #[test]
    fn tab_goes_from_a_cards_row_to_its_switch() {
        let ctx = themed();
        let mut network = window();
        draw(&ctx, &mut network, Vec::new());
        // em0's row, then its switch.
        for _ in 0..2 {
            draw(&ctx, &mut network, vec![key(egui::Key::Tab)]);
        }

        let (_, actions) = draw(&ctx, &mut network, vec![key(egui::Key::Space)]);

        assert!(
            matches!(&actions[..], [Action::Root(Root::Switch { card, on: false })] if card == "em0"),
            "{actions:?}"
        );
    }

    #[test]
    fn a_cards_switch_turns_it_on_and_a_wlans_is_the_wifi_switch() {
        let cards = cards();

        assert_eq!(
            [switched(&cards.cards[2], true), switched(&cards.cards[3], false)],
            [Root::Switch { card: "ue0".into(), on: true }, Root::WifiOff]
        );
    }

    #[test]
    fn enter_on_a_row_opens_its_settings() {
        let ctx = themed();
        let mut network = window();
        draw(&ctx, &mut network, Vec::new());
        draw(&ctx, &mut network, vec![key(egui::Key::Tab)]);

        let (_, actions) = draw(&ctx, &mut network, vec![key(egui::Key::Enter)]);

        assert!(matches!(&actions[..], [Action::Open(card)] if card == "em0"), "{actions:?}");
    }

    /// Only wlan1, Wi-Fi turned off: its wlan destroyed, settings Manual.
    fn wifi_off() -> Cards {
        let wlan1 = card("wlan1", Kind::Wifi, State::Off, &[], "WPA inet 10.0.0.5/24 NOAUTO");
        Cards { cards: vec![wlan1], defaultrouter: None, route: None }
    }

    #[test]
    fn wifi_turned_off_has_no_settings_to_open() {
        assert!(Page::open(&wifi_off(), "wlan1").is_none());
    }

    #[test]
    fn wifi_turned_off_keeps_its_switch() {
        let ctx = themed();
        let mut network = window();
        network.cards = Some(Ok(wifi_off()));
        draw(&ctx, &mut network, Vec::new());
        draw(&ctx, &mut network, vec![key(egui::Key::Tab)]);

        let (_, actions) = draw(&ctx, &mut network, vec![key(egui::Key::Space)]);

        assert!(matches!(actions[..], [Action::Root(Root::WifiOn)]), "{actions:?}");
    }

    #[test]
    fn settings_open_when_wifi_goes_off_say_so_and_have_no_apply() {
        let mut network = window();
        let mut on = wifi_off();
        on.cards[0].state = State::Connected;
        network.page = Page::open(&on, "wlan1");
        network.cards = Some(Ok(wifi_off()));

        let (texts, _) = draw(&themed(), &mut network, Vec::new());

        let said = text(&texts, "Wi-Fi is off: switch it on to change its settings.").is_some();
        assert!(said && text(&texts, "Apply").is_none(), "{texts:?}");
    }

    /// The window with a new lagg's page open.
    fn new_lagg() -> Network {
        let mut network = window();
        network.page = Some(Page::new_lagg(&cards()));
        network
    }

    /// The open page's lagg settings.
    fn lagg_form(network: &mut Network) -> &mut LaggForm {
        network.page.as_mut().and_then(|p| p.lagg.as_mut()).expect("a lagg's page")
    }

    /// A new lagg's page with failover, re0 then wlan0, and Automatic picked.
    fn picked() -> Network {
        let mut network = new_lagg();
        let lagg = lagg_form(&mut network);
        lagg.proto = Some("failover");
        lagg.ports = vec!["re0".into(), "wlan0".into()];
        lagg.picked = true;
        network
    }

    #[test]
    fn new_lagg_below_the_cards_opens_a_new_laggs_page() {
        let actions = click(&themed(), &mut window(), "New lagg");

        assert!(matches!(actions[..], [Action::NewLagg]), "{actions:?}");
    }

    #[test]
    fn a_new_laggs_page_offers_the_cards_that_can_be_ports() {
        let (texts, _) = draw_tall(&themed(), &mut new_lagg(), Vec::new());

        let offered: Vec<_> =
            ["em0", "re0", "ue0", "wlan0", "em2", "em1"].into_iter().filter(|n| text(&texts, n).is_some()).collect();
        assert_eq!(offered, ["em0", "re0", "ue0", "wlan0"], "{texts:?}");
    }

    #[test]
    fn the_cards_ticked_are_the_ports_in_that_order() {
        let ctx = themed();
        let mut network = new_lagg();
        click(&ctx, &mut network, "wlan0");
        click(&ctx, &mut network, "re0");

        assert_eq!(lagg_form(&mut network).ports, ["wlan0", "re0"]);
    }

    #[test]
    fn create_with_nothing_picked_says_what_to_pick_and_runs_nothing() {
        let ctx = themed();
        let mut network = new_lagg();

        let actions = click(&ctx, &mut network, "Create");
        let (texts, _) = draw_tall(&ctx, &mut network, Vec::new());

        assert!(actions.is_empty() && text(&texts, "Pick a mode.").is_some(), "{actions:?} {texts:?}");
    }

    #[test]
    fn create_runs_lagg_with_the_mode_the_ports_and_dhcp() {
        let actions = click(&themed(), &mut picked(), "Create");

        let want = ["lagg", "lagg1", "failover", "re0,wlan0", "dhcp"];
        assert!(matches!(&actions[..], [Action::Root(Root::Apply { args, .. })] if args == &want), "{actions:?}");
    }

    #[test]
    fn create_on_manual_runs_lagg_with_the_address() {
        let mut network = picked();
        if let Some(page) = &mut network.page {
            page.manual = true;
            page.address = "10.0.0.5".into();
            page.prefix = "24".into();
        }

        let actions = click(&themed(), &mut network, "Create");

        let want = ["lagg", "lagg1", "failover", "re0,wlan0", "manual", "10.0.0.5/24", "-"];
        assert!(matches!(&actions[..], [Action::Root(Root::Apply { args, .. })] if args == &want), "{actions:?}");
    }

    #[test]
    fn a_wired_port_beside_a_wlan_says_it_takes_the_wlans_mac_address() {
        let (texts, _) = draw_tall(&themed(), &mut picked(), Vec::new());

        assert!(text(&texts, "re0 takes wlan0's MAC address, which a lagg with Wi-Fi needs.").is_some(), "{texts:?}");
    }

    #[test]
    fn a_laggs_page_starts_from_its_mode_and_ports() {
        let lagg = Page::open(&cards(), "lagg0").and_then(|p| p.lagg).expect("lagg0's page");

        assert_eq!((lagg.proto, lagg.ports, lagg.picked), (Some("failover"), vec!["re1".into(), "wlan1".into()], true));
    }

    #[test]
    fn remove_on_a_laggs_page_runs_lagg_remove() {
        let actions = click(&themed(), &mut opened("lagg0"), "Remove");

        let want = ["lagg", "lagg0", "remove"];
        assert!(matches!(&actions[..], [Action::Root(Root::Apply { args, .. })] if args == &want), "{actions:?}");
    }

    /// Ends `network`'s helper run `root` as done, with the keyboard in use, then draws a frame
    /// with the cards as they were and two with `after` as the cards read again; what has the
    /// focus.
    fn done_with_the_keyboard(network: &mut Network, root: Root, after: fn() -> Result<Cards, String>) -> Option<Id> {
        let ctx = themed();
        network.ctx = ctx.clone();
        network.keyboard = true;
        network.cards_reader.read = after;
        network.root = Some(root);
        network.root_finished(&ctx, Ended::Done, 0.0);
        draw(&ctx, network, Vec::new());
        network.cards = Some(after());
        draw(&ctx, network, Vec::new());
        draw(&ctx, network, Vec::new());
        ctx.memory(|m| m.focused())
    }

    #[test]
    fn after_create_the_keyboard_is_on_the_new_laggs_row() {
        let mut network = picked();
        let create = network.page.as_ref().map(Page::apply).expect("a page").expect("all picked");

        let after =
            || Ok(Cards { cards: vec![Card { name: "lagg1".into(), ..lagg() }], defaultrouter: None, route: None });
        assert_eq!(done_with_the_keyboard(&mut network, create, after), Some(row_id("lagg1")));
    }

    #[test]
    fn after_remove_with_its_ports_unticked_the_keyboard_is_on_its_first_ports_row() {
        let mut network = opened("lagg0");
        if let Some(lagg) = network.page.as_mut().and_then(|p| p.lagg.as_mut()) {
            lagg.ports.clear();
        }
        let remove = network.page.as_ref().map(Page::remove).expect("lagg0's page");

        let after = || {
            let re1 = card("re1", Kind::Wired, State::Connected, &[], "DHCP");
            Ok(Cards { cards: vec![re1], defaultrouter: None, route: None })
        };
        assert_eq!(done_with_the_keyboard(&mut network, remove, after), Some(row_id("re1")));
    }

    #[test]
    fn a_lagg_has_no_switch() {
        let ctx = themed();
        let mut network = window();
        network.cards = Some(Ok(Cards { cards: vec![lagg()], defaultrouter: None, route: None }));
        draw(&ctx, &mut network, Vec::new());
        draw(&ctx, &mut network, vec![key(egui::Key::Tab)]);

        let (_, actions) = draw(&ctx, &mut network, vec![key(egui::Key::Space)]);

        assert!(!actions.iter().any(|a| matches!(a, Action::Root(_))), "{actions:?}");
    }

    #[test]
    fn a_bridge_member_has_no_settings_to_open() {
        assert!(Page::open(&cards(), "em1").is_none());
    }

    #[test]
    fn a_manual_cards_settings_are_filled_in() {
        let page = Page::open(&cards(), "em0").unwrap();

        assert_eq!(
            [page.address.as_str(), &page.prefix, &page.gateway, &page.dns],
            ["192.168.1.5", "24", "192.168.1.1", "192.0.2.53"]
        );
    }

    /// re0 with an address, gateway and DNS server from DHCP.
    fn from_dhcp() -> Cards {
        let mut cards = cards();
        let re0 = &mut cards.cards[1];
        re0.state = State::Connected;
        re0.inet = vec![(Ipv4Addr::new(10, 1, 9, 5), 24)];
        re0.dns = vec![Ipv4Addr::new(10, 1, 9, 1)];
        cards.route = Some(Route { iface: "re0".into(), gateway: Some(Ipv4Addr::new(10, 1, 9, 1)) });
        cards
    }

    #[test]
    fn an_automatic_cards_manual_settings_start_from_dhcps_but_its_address() {
        let page = Page::open(&from_dhcp(), "re0").unwrap();

        assert_eq!([page.address.as_str(), &page.prefix, &page.gateway, &page.dns], ["", "24", "10.1.9.1", "10.1.9.1"]);
    }

    #[test]
    fn the_address_hint_is_dhcps_address() {
        let mut network = window();
        network.cards = Some(Ok(from_dhcp()));
        network.page = Page::open(&from_dhcp(), "re0");
        if let Some(page) = &mut network.page {
            page.manual = true;
        }

        let (texts, _) = draw(&themed(), &mut network, Vec::new());

        assert!(text(&texts, "10.1.9.5 (from DHCP)").is_some(), "{texts:?}");
    }

    #[test]
    fn apply_on_automatic_runs_iface_dhcp() {
        let ctx = themed();
        let mut network = opened("re0");

        let actions = click(&ctx, &mut network, "Apply");

        assert!(
            matches!(&actions[..], [Action::Root(Root::Apply { args, .. })] if args == &["iface", "re0", "dhcp"]),
            "{actions:?}"
        );
    }

    #[test]
    fn apply_on_manual_runs_iface_manual_with_what_was_typed() {
        let ctx = themed();
        let mut network = opened("em0");

        let actions = click(&ctx, &mut network, "Apply");

        let want = ["iface", "em0", "manual", "192.168.1.5/24", "192.168.1.1", "192.0.2.53"];
        assert!(matches!(&actions[..], [Action::Root(Root::Apply { args, .. })] if args == &want), "{actions:?}");
    }

    #[test]
    fn apply_with_a_bad_address_says_why_and_runs_nothing() {
        let ctx = themed();
        let mut network = opened("em0");
        if let Some(page) = &mut network.page {
            page.address = "192.168.1.256".into();
        }

        let actions = click(&ctx, &mut network, "Apply");
        let (texts, _) = draw(&ctx, &mut network, Vec::new());

        let why = "\"192.168.1.256\" isn't an IPv4 address for the address (like 192.168.1.5).";
        assert!(actions.is_empty() && text(&texts, why).is_some(), "{actions:?} {texts:?}");
    }

    #[test]
    fn a_gateway_typed_on_another_card_says_it_moves() {
        let mut network = opened("re0");
        if let Some(page) = &mut network.page {
            page.manual = true;
            page.gateway = "10.0.0.1".into();
        }

        let (texts, _) = draw(&themed(), &mut network, Vec::new());

        assert!(text(&texts, "em0 has the gateway 192.168.1.1. Applying moves it to re0.").is_some(), "{texts:?}");
    }

    #[test]
    fn a_card_set_up_by_hand_shows_its_line_and_no_apply() {
        let (texts, _) = draw(&themed(), &mut opened("em2"), Vec::new());

        let line =
            "Set up by hand in rc.conf:\nifconfig_em2=\"DHCP description office\"\nThis window leaves it as it is.";
        assert!(text(&texts, line).is_some() && text(&texts, "Apply").is_none(), "{texts:?}");
    }

    /// Runs Apply on `network`'s open page through a helper that ends as `helper` says, and
    /// takes its end as the window does.
    fn applied(network: &mut Network, helper: fn(Root) -> Ended) {
        let ctx = themed();
        network.ctx = ctx.clone();
        network.cards_reader.read = || Ok(cards());
        network.root_helper = helper;
        click(&ctx, network, "Apply").into_iter().for_each(|action| network.act(action));
        let ended = network.root_done.recv_timeout(Duration::from_secs(5)).expect("the helper's end");
        network.root_finished(&ctx, ended, 0.0);
    }

    #[test]
    fn settings_that_applied_close() {
        let mut network = opened("re0");

        applied(&mut network, |_| Ended::Done);

        assert!(network.page.is_none());
    }

    #[test]
    fn settings_that_failed_stay_open_with_the_helpers_reason() {
        let mut network = opened("re0");

        applied(&mut network, |_| Ended::Failed("Couldn't start re0 (service netif start re0).".into()));

        let error = network.page.as_ref().and_then(|p| p.error.as_deref());
        assert_eq!(error, Some("Couldn't start re0 (service netif start re0)."));
    }

    #[test]
    fn a_dismissed_password_box_leaves_the_settings_open() {
        let mut network = opened("re0");

        applied(&mut network, |_| Ended::Dismissed);

        assert!(network.page.as_ref().is_some_and(|p| p.error.is_none()));
    }

    #[test]
    fn the_cards_are_read_again_after_the_helper() {
        let mut network = opened("re0");

        applied(&mut network, |_| Ended::Done);

        assert!(network.cards_reader.reading);
    }

    #[test]
    fn escape_on_the_settings_goes_back_to_the_cards_row() {
        let ctx = themed();
        let mut network = window();
        draw(&ctx, &mut network, Vec::new());
        draw(&ctx, &mut network, vec![key(egui::Key::Tab)]);
        let (_, actions) = draw(&ctx, &mut network, vec![key(egui::Key::Enter)]);
        actions.into_iter().for_each(|action| network.act(action));
        draw(&ctx, &mut network, Vec::new());

        draw(&ctx, &mut network, vec![key(egui::Key::Escape)]);
        let (_, actions) = draw(&ctx, &mut network, vec![key(egui::Key::Enter)]);

        assert!(matches!(&actions[..], [Action::Open(card)] if card == "em0"), "{actions:?}");
    }

    #[test]
    fn the_interfaces_tab_shows_the_section() {
        let ctx = themed();
        let mut network = wifi_window();

        let actions = click(&ctx, &mut network, "Interfaces");

        assert!(matches!(actions[..], [Action::Section(Section::Interfaces)]), "{actions:?}");
    }

    #[test]
    fn the_section_reads_the_cards_off_the_windows_thread() {
        let ctx = themed();
        let mut network = wifi_window();
        network.section = Section::Interfaces;
        network.cards_reader.read = || Ok(cards());

        draw(&ctx, &mut network, Vec::new());
        let read = network.cards_reader.done.recv_timeout(Duration::from_secs(5));

        assert!(read.is_ok_and(|cards| cards.is_ok()));
    }
}
