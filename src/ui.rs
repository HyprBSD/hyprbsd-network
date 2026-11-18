//! The window: a card per Wi-Fi network, and Other network for one that hides its name; the
//! network cards, each with its settings, in a section of their own.

mod interfaces;

use crate::cards::{self, Cards};
use crate::instance::Window;
use crate::paths::HELPER;
use crate::radio::{self, Radio};
use crate::theme::{self, BACKGROUND, DIM, FAINT, ORANGE, RADIUS, RED, SURFACE, SURFACE_HOVER, TEAL, TEXT};
use crate::trace::trace;
use crate::view::{self, Row};
use crate::wifi::{Security, Ssid, Status, Strength, Wait, WifiError};
use crate::worker::{self, Request, Update, Wifi};
use eframe::egui::text::{LayoutJob, TextFormat, TextWrapping};
use eframe::egui::{
    self, Align, Color32, CornerRadius, CursorIcon, FontId, Galley, Id, Layout, Margin, Rect, RichText, Sense, Stroke,
    StrokeKind, Ui, UiBuilder, UserAttentionType, Vec2, ViewportCommand, pos2, vec2,
};
use std::mem;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

/// A card's height without what opens under it.
const HEADER: f32 = 56.0;
/// Where a card's text starts, after the signal bars.
const TEXT_LEFT: f32 = 54.0;
const BUTTON: Vec2 = vec2(104.0, 34.0);
/// The window's smallest size.
const SMALLEST: Vec2 = vec2(400.0, 360.0);
/// pkexec's exit status for a dismissed password box.
const PKEXEC_DISMISSED: i32 = 126;
/// pkexec's exit status when polkit says no, or finds no agent to ask for a password.
const PKEXEC_REFUSED: i32 = 127;
/// How often, in seconds, Wi-Fi is read again while wpa_supplicant can't be reached.
const RADIO_TICK: f64 = 3.0;
/// How long, in seconds, the window says Wi-Fi is turning on after the helper's `wifi on` ends, or
/// after rc.conf first reads on, while wpa_supplicant can't be reached; then it says why.
const STARTING: f64 = 10.0;
/// How often, in seconds, the cards are read again while their section is shown.
const CARDS_TICK: f64 = 5.0;

/// Set to stop a join, which then undoes itself.
static STOP: AtomicBool = AtomicBool::new(false);

/// The window's title, app ID and sizes, drawn without vsync.
fn options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("HyprBSD Network")
            .with_app_id("hyprbsd-network")
            .with_inner_size([520.0, 640.0])
            .with_min_inner_size(SMALLEST),
        glow_options: eframe::egui_glow::GlowConfiguration { vsync: false, ..Default::default() },
        ..Default::default()
    }
}

/// Opens the window; a second start knocking on `window` asks it to come forward.
pub fn run(window: &Window) -> Result<(), eframe::Error> {
    eframe::run_native(
        "HyprBSD Network",
        options(),
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            let ctx = cc.egui_ctx.clone();
            window.on_knock(move || {
                ctx.send_viewport_cmd(ViewportCommand::RequestUserAttention(UserAttentionType::Informational));
            })?;
            Ok(Box::new(Network::start(&cc.egui_ctx)))
        }),
    )
}

/// A card open: a network's, or Other network's.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Card {
    Network(Ssid),
    Other,
}

/// What's typed in the open card.
#[derive(Debug, Default)]
struct Form {
    name: String,
    password: String,
    show: bool,
    /// The saved password didn't work, so a new one is asked for.
    retype: bool,
    error: Option<String>,
    focused: bool,
}

/// What the worker is doing for the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Doing {
    Rescan,
    Join,
    Forget,
    Autoconnect,
    Disconnect,
}

/// What the root helper is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Root {
    WifiOn,
    WifiOff,
    SetUp,
    /// A card turned on or off.
    Switch {
        card: String,
        on: bool,
    },
    /// A card's or a lagg's settings applied: the helper's `iface` or `lagg` arguments.
    Apply {
        /// The card whose row the keyboard goes back to when it's done.
        card: String,
        args: Vec<String>,
    },
}

impl Root {
    fn args(&self) -> Vec<&str> {
        match self {
            Root::WifiOn => vec!["wifi", "on"],
            Root::WifiOff => vec!["wifi", "off"],
            Root::SetUp => vec!["wifi-setup"],
            Root::Switch { card, on } => vec!["iface-switch", card, if *on { "on" } else { "off" }],
            Root::Apply { args, .. } => args.iter().map(String::as_str).collect(),
        }
    }
}

/// How a run of the root helper ended.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ended {
    Done,
    /// The password box was dismissed.
    Dismissed,
    /// Why it failed, in words.
    Failed(String),
}

/// A read of the system on a thread of its own, so the window doesn't wait for it.
struct Reader<T> {
    read: fn() -> T,
    /// When, by egui's clock, a read was last asked for.
    at: f64,
    /// A read hasn't come back.
    reading: bool,
    /// The read running may have started before the system changed.
    stale: bool,
    done: Receiver<T>,
    send: Sender<T>,
}

impl<T: Send + 'static> Reader<T> {
    fn new(read: fn() -> T) -> Self {
        let (send, done) = mpsc::channel();
        Reader { read, at: 0.0, reading: false, stale: false, done, send }
    }

    /// Reads `what` on a thread, at `now` by egui's clock, unless a read is running.
    fn load(&mut self, ctx: &egui::Context, now: f64, what: &'static str) {
        self.at = now;
        if self.reading {
            return;
        }
        self.reading = true;
        let (send, wake, read) = (self.send.clone(), ctx.clone(), self.read);
        thread::spawn(move || {
            let started = Instant::now();
            let value = read();
            trace(|| format!("read {what} in {:.3} s", started.elapsed().as_secs_f64()));
            let _ = send.send(value);
            wake.request_repaint();
        });
    }

    /// Reads `what` again at `now`, after the system changed: a read already running may have
    /// started before the change, so its value is dropped and another read is due at once.
    fn reload(&mut self, ctx: &egui::Context, now: f64, what: &'static str) {
        self.stale = self.reading;
        self.load(ctx, now, what);
    }

    /// A read that has come back.
    fn take(&mut self) -> Option<T> {
        let value = self.done.try_recv().ok()?;
        self.came_back(value)
    }

    /// A read came back with `value`: the value, unless it may have started before the system
    /// changed.
    fn came_back(&mut self, value: T) -> Option<T> {
        self.reading = false;
        if mem::take(&mut self.stale) {
            self.at = f64::NEG_INFINITY;
            return None;
        }
        Some(value)
    }

    /// Whether `every` seconds have passed at `now` since a read was last asked for.
    fn due(&self, now: f64, every: f64) -> bool {
        now - self.at >= every
    }
}

/// A request the worker is carrying out, and the card it's for.
#[derive(Debug)]
struct Busy {
    card: Option<Card>,
    doing: Doing,
}

/// What a card asked for.
#[derive(Debug)]
enum Action {
    Toggle(Card),
    Ask(Request, Option<Card>, Doing),
    Stop,
    Root(Root),
    Section(Section),
    /// Opens a card's settings.
    Open(String),
    /// Opens a new lagg's page.
    NewLagg,
    /// Closes them.
    Back,
}

/// What the window shows under its title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Wifi,
    Interfaces,
}

pub struct Network {
    requests: Sender<Request>,
    updates: Receiver<Update>,
    /// Wi-Fi's state, or why it can't be read; None until the worker first says.
    wifi: Option<Result<Wifi, String>>,
    open: Option<(Card, Form)>,
    busy: Option<Busy>,
    /// Why the last Rescan failed.
    notice: Option<String>,
    closing: bool,
    /// The keyboard was used last, not the pointer.
    keyboard: bool,
    /// wpa_supplicant can't be reached.
    down: bool,
    /// Wi-Fi as rc.conf and the kernel have it, while wpa_supplicant can't be reached; None
    /// until read.
    radio: Option<Radio>,
    radio_reader: Reader<Radio>,
    /// By egui's clock, the last frame the helper's `wifi on` ran, or else when rc.conf first
    /// read Wi-Fi on while wpa_supplicant can't be reached.
    starting: Option<f64>,
    section: Section,
    /// The network cards, or why they can't be read; None until read.
    cards: Option<Result<Cards, String>>,
    cards_reader: Reader<Result<Cards, String>>,
    /// The settings of a card, as typed, while they're open.
    page: Option<interfaces::Page>,
    /// The widget that had the focus when a request or a helper run started; disabled while it
    /// runs, it loses the focus, and gets it back once enabled.
    refocus: Option<egui::Id>,
    /// What the root helper is doing.
    root: Option<Root>,
    /// How the root helper runs, on a thread of its own.
    root_helper: fn(Root) -> Ended,
    /// How the root helper's runs ended, a thread's for each run.
    root_done: Receiver<Ended>,
    root_send: Sender<Ended>,
    ctx: egui::Context,
}

impl Network {
    fn start(ctx: &egui::Context) -> Self {
        let (requests, from_window) = mpsc::channel();
        let (to_window, updates) = mpsc::channel();
        let wake = ctx.clone();
        let wait = Wait { stop: Some(&STOP), ..Wait::default() };
        thread::spawn(move || {
            let send = move |update| {
                let _ = to_window.send(update);
                wake.request_repaint();
            };
            worker::run(worker::open, from_window, send, wait);
        });
        let (root_send, root_done) = mpsc::channel();
        Network {
            requests,
            updates,
            wifi: None,
            open: None,
            busy: None,
            notice: None,
            closing: false,
            keyboard: false,
            down: false,
            radio: None,
            radio_reader: Reader::new(radio::read),
            starting: None,
            section: Section::Wifi,
            cards: None,
            cards_reader: Reader::new(|| cards::read().map_err(|e| view::as_sentence(&e.to_string()))),
            page: None,
            refocus: None,
            root: None,
            root_helper: run_root,
            root_done,
            root_send,
            ctx: ctx.clone(),
        }
    }

    /// Reads Wi-Fi from rc.conf and the kernel on a thread, at `now` by egui's clock, unless a
    /// read is running.
    fn load_radio(&mut self, now: f64) {
        self.radio_reader.load(&self.ctx, now, "Wi-Fi");
    }

    /// A read of Wi-Fi came back; it shows while wpa_supplicant still can't be reached.
    fn read_radio_done(&mut self, radio: Radio) {
        if self.down {
            self.radio = Some(radio);
        }
    }

    /// While the cards' section is shown, reads them when it opens and again every tick.
    fn refresh_cards(&mut self, ctx: &egui::Context) {
        if self.section != Section::Interfaces {
            return;
        }
        let now = ctx.input(|i| i.time);
        if self.cards.is_none() || self.cards_reader.due(now, CARDS_TICK) {
            self.cards_reader.load(ctx, now, "the cards");
        }
        ctx.request_repaint_after(Duration::from_secs_f64(CARDS_TICK));
    }

    /// While wpa_supplicant can't be reached, reads Wi-Fi again every tick, so an adapter
    /// plugged in or taken out shows.
    fn refresh_radio(&mut self, ctx: &egui::Context) {
        if !self.down {
            return;
        }
        let now = ctx.input(|i| i.time);
        if self.radio_reader.due(now, RADIO_TICK) {
            self.load_radio(now);
        }
        ctx.request_repaint_after(Duration::from_secs_f64(RADIO_TICK));
    }

    /// Notes, at `now`, whether Wi-Fi is turning on; not while it's being turned off.
    fn watch_start(&mut self, now: f64) {
        self.starting = match (&self.root, self.down, self.radio) {
            (Some(Root::WifiOn), ..) => Some(now),
            (Some(Root::WifiOff), ..) => None,
            (_, true, Some(Radio::On)) => self.starting.or(Some(now)),
            _ => None,
        };
    }

    /// Whether Wi-Fi is turning on, for less than `STARTING` so far.
    fn turning_on(&self, now: f64) -> bool {
        self.starting.is_some_and(|since| now - since < STARTING)
    }

    /// Wi-Fi's state from the worker; while wpa_supplicant can't be reached, Wi-Fi as rc.conf
    /// and the kernel have it too.
    fn state(&mut self, state: Result<Wifi, WifiError>, now: f64) {
        self.down = matches!(&state, Err(e) if worker::gone(e));
        if self.down {
            self.load_radio(now);
        } else {
            self.radio = None;
        }
        self.wifi = Some(state.map_err(|e| view::sentence(&e)));
    }

    /// Reads the worker's updates, the root helper's notices and the reads of Wi-Fi; closes the
    /// window once a request it waited for has ended.
    fn poll(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        while let Some(radio) = self.radio_reader.take() {
            self.read_radio_done(radio);
        }
        while let Some(cards) = self.cards_reader.take() {
            self.cards = Some(cards);
        }
        while let Ok(ended) = self.root_done.try_recv() {
            self.root_finished(ctx, ended, now);
        }
        loop {
            match self.updates.try_recv() {
                Ok(Update::State(state)) => {
                    trace(|| match &state {
                        Ok(wifi) => format!("state: {}", wifi.status.state),
                        Err(e) => format!("state: {e}"),
                    });
                    self.state(state, now);
                }
                Ok(Update::Done(result)) => self.done(ctx, result),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.wifi =
                        Some(Err("The window stopped talking to wpa_supplicant; close it and open it again.".into()));
                    self.busy = None;
                    break;
                }
            }
        }
        if self.closing && self.busy.is_none() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    /// The root helper ended: a card's settings that applied close; why they didn't shows on
    /// them, and why anything else failed as the notice. Wi-Fi is read again while
    /// wpa_supplicant can't be reached, and the cards while they're shown.
    fn root_finished(&mut self, ctx: &egui::Context, ended: Ended, now: f64) {
        trace(|| format!("the helper ended: {ended:?}"));
        let root = self.root.take();
        self.notice = None;
        match (root, ended, &mut self.page) {
            (Some(Root::Apply { card, .. }), Ended::Done, _) => self.close_page_to(ctx, &card),
            (Some(Root::Apply { .. }), Ended::Failed(why), Some(page)) => page.error = Some(why),
            // The helper has put Wi-Fi on in rc.conf.
            (Some(Root::WifiOn), Ended::Done, _) if self.down => self.radio = Some(Radio::On),
            (_, Ended::Failed(why), _) => self.notice = Some(why),
            _ => {}
        }
        if self.down {
            self.radio_reader.reload(ctx, now, "Wi-Fi");
        }
        if self.section == Section::Interfaces {
            self.cards_reader.reload(ctx, now, "the cards");
        }
    }

    /// A request ended: a join or forget that worked closes its card; an error shows in it.
    fn done(&mut self, ctx: &egui::Context, result: Result<(), WifiError>) {
        let Some(busy) = self.busy.take() else { return };
        if busy.doing == Doing::Rescan {
            self.notice = result.err().map(|e| view::sentence(&e));
            return;
        }
        let Some((card, form)) = self.open.as_mut().filter(|(card, _)| busy.card.as_ref() == Some(card)) else {
            return;
        };
        match result {
            Ok(()) if busy.doing == Doing::Autoconnect => {}
            Ok(()) => self.close(ctx),
            Err(e) => {
                if matches!(e, WifiError::WrongPassword(_) | WifiError::NotJoined(_)) && *card != Card::Other {
                    form.retype = true;
                    form.focused = false;
                }
                form.error = Some(view::sentence(&e));
            }
        }
    }

    fn act(&mut self, action: Action) {
        match action {
            Action::Toggle(card) if self.open.as_ref().is_some_and(|(open, _)| *open == card) => self.open = None,
            Action::Toggle(card) => self.open = Some((card, Form::default())),
            Action::Ask(request, card, doing) if self.idle() => {
                STOP.store(false, Ordering::Relaxed);
                if let Some((open, form)) = &mut self.open
                    && card.as_ref() == Some(open)
                {
                    form.error = None;
                }
                if self.requests.send(request).is_ok() {
                    self.busy = Some(Busy { card, doing });
                    self.refocus = self.ctx.memory(|m| m.focused());
                }
            }
            Action::Ask(..) => {}
            Action::Stop => STOP.store(true, Ordering::Relaxed),
            Action::Root(root) if self.idle() => {
                trace(|| format!("the helper: {}", root.args().join(" ")));
                self.root = Some(root.clone());
                self.notice = None;
                if let Some(page) = &mut self.page {
                    page.error = None;
                }
                self.refocus = self.ctx.memory(|m| m.focused());
                let (send, wake, helper) = (self.root_send.clone(), self.ctx.clone(), self.root_helper);
                thread::spawn(move || {
                    let _ = send.send(helper(root));
                    wake.request_repaint();
                });
            }
            Action::Root(_) => {}
            Action::Section(section) => {
                self.section = section;
                self.notice = None;
            }
            Action::Open(card) => {
                if let Some(Ok(cards)) = &self.cards {
                    self.page = interfaces::Page::open(cards, &card);
                }
            }
            Action::NewLagg => {
                if let Some(Ok(cards)) = &self.cards {
                    self.page = Some(interfaces::Page::new_lagg(cards));
                }
            }
            Action::Back => {
                let ctx = self.ctx.clone();
                self.close_page(&ctx);
            }
        }
    }

    /// With the keyboard in use and nothing focused, focuses `refocus` once it's enabled again;
    /// forgets it once the focus has gone elsewhere.
    fn give_focus_back(&mut self, ctx: &egui::Context) {
        let Some(id) = self.refocus else { return };
        if !self.keyboard {
            self.refocus = None;
            return;
        }
        match ctx.memory(|m| m.focused()) {
            Some(focused) if focused != id => self.refocus = None,
            Some(_) => {}
            None if ctx.read_response(id).is_some_and(|r| r.enabled() && r.sense.is_focusable()) => {
                ctx.memory_mut(|m| m.request_focus(id));
                self.refocus = None;
            }
            None => {}
        }
    }

    /// Whether neither the worker nor the root helper is running anything for the window.
    fn idle(&self) -> bool {
        self.busy.is_none() && self.root.is_none()
    }

    /// Where the Wi-Fi switch stands: on while wpa_supplicant answers, else as rc.conf has it;
    /// none without Wi-Fi set up.
    fn switched_on(&self) -> Option<bool> {
        match (&self.wifi, self.radio) {
            (Some(Ok(_)), _) | (_, Some(Radio::On)) => Some(true),
            (_, Some(Radio::Off)) => Some(false),
            _ => None,
        }
    }

    fn header(ui: &mut Ui) -> egui::Response {
        let mut title = LayoutJob::default();
        for (text, color) in
            [("Hypr", TEXT.gamma_multiply(0.55)), ("BSD", ORANGE.gamma_multiply(0.85)), (" Network", TEXT)]
        {
            title.append(text, 0.0, TextFormat::simple(theme::semibold(22.0), color));
        }
        ui.label(title)
    }

    /// "Wi-Fi", its switch and Rescan; while joined, that rescanning can drop the connection.
    fn wifi_heading(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let wifi = self.wifi.as_ref().and_then(|w| w.as_ref().ok());
        let idle = self.idle();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Wi-Fi").font(theme::semibold(17.0)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(on) = self.switched_on()
                    && with_id(ui, Id::new("wifi-switch"), |ui| switch(ui, "Wi-Fi", on, idle)).clicked()
                {
                    actions.push(Action::Root(if on { Root::WifiOff } else { Root::WifiOn }));
                }
                let Some(wifi) = wifi else { return };
                let scanning = wifi.scanning || self.busy.as_ref().is_some_and(|b| b.doing == Doing::Rescan);
                let label = if scanning { "Scanning…" } else { "Rescan" };
                if button(ui, Id::new("rescan"), label, FAINT, TEXT, idle && !scanning).clicked() {
                    actions.push(Action::Ask(Request::Rescan, None, Doing::Rescan));
                }
            });
        });
        if let Some(wifi) = wifi.filter(|w| w.status.joined()) {
            let warning = "Rescanning can drop the connection for a moment.";
            let line = wifi.age.map_or_else(|| warning.to_owned(), |age| format!("{} {warning}", view::scanned(age)));
            ui.label(RichText::new(line).font(theme::body(13.0)).color(DIM));
        }
        if let Some(notice) = &self.notice {
            ui.label(RichText::new(notice).font(theme::body(13.0)).color(RED));
        }
    }

    fn list(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let turning_on = self.turning_on(ui.input(|i| i.time));
        let Network { wifi, open, busy, down, radio, root, .. } = self;
        let wifi = match wifi {
            Some(Ok(wifi)) => &*wifi,
            Some(Err(_)) if turning_on => {
                message(ui, "Turning Wi-Fi on…");
                return;
            }
            Some(Err(why)) if !*down || radio.is_some() => {
                match radio {
                    Some(Radio::NoAdapter) => message(ui, "No Wi-Fi adapter."),
                    Some(Radio::NotSetUp) => {
                        message(ui, "Wi-Fi isn't set up on this computer.");
                        let label = if *root == Some(Root::SetUp) { "Setting up…" } else { "Set up Wi-Fi" };
                        if right_button(ui, Id::new("set-up"), label, ORANGE, BACKGROUND, root.is_none()).clicked() {
                            actions.push(Action::Root(Root::SetUp));
                        }
                    }
                    Some(Radio::Off) => message(ui, "Wi-Fi is off."),
                    _ => message(ui, why),
                }
                return;
            }
            // Before the worker first says, or before Wi-Fi is read while wpa_supplicant is down.
            _ => {
                ui.label(RichText::new("Looking for Wi-Fi…").color(DIM));
                return;
            }
        };
        let working = busy.is_some() || root.is_some();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            if wifi.rows.is_empty() {
                ui.label(RichText::new("No networks in range.").color(DIM));
            }
            for row in &wifi.rows {
                let card = Card::Network(row.ssid.clone());
                let form = open.as_mut().filter(|(open, _)| *open == card).map(|(_, form)| form);
                let doing = busy.as_ref().filter(|b| b.card.as_ref() == Some(&card)).map(|b| b.doing);
                let shown = Shown { status: &wifi.status, doing, busy: working };
                actions.extend(network_card(ui, row, &shown, form));
            }
            let form = open.as_mut().filter(|(open, _)| *open == Card::Other).map(|(_, form)| form);
            let doing = busy.as_ref().filter(|b| b.card == Some(Card::Other)).map(|b| b.doing);
            actions.extend(other_card(ui, form, doing, working));
            follow_focus(ui);
        });
    }

    /// Notes whether the keyboard or the pointer was used last. Escape closes the open card,
    /// unless a request is running; with no card open, it closes the window.
    fn input(&mut self, ctx: &egui::Context) {
        ctx.input(|i| {
            if i.pointer.any_pressed() {
                self.keyboard = false;
            }
            if i.events.iter().any(|e| matches!(e, egui::Event::Key { pressed: true, .. })) {
                self.keyboard = true;
            }
        });
        if !ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            return;
        }
        if self.section == Section::Interfaces {
            if self.page.is_none() {
                ctx.send_viewport_cmd(ViewportCommand::Close);
            } else if !matches!(self.root, Some(Root::Apply { .. })) {
                self.close_page(ctx);
            }
        } else if self.open.is_some() && self.busy.is_none() {
            self.close(ctx);
        } else if self.open.is_none() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    /// Closes a card's settings; with the keyboard in use, its row takes the focus.
    fn close_page(&mut self, ctx: &egui::Context) {
        if let Some(card) = self.page.as_ref().map(|p| p.card.clone()) {
            self.close_page_to(ctx, &card);
        }
    }

    /// Closes a card's settings; with the keyboard in use, `card`'s row takes the focus, now or
    /// once the cards read again have it.
    fn close_page_to(&mut self, ctx: &egui::Context, card: &str) {
        if self.page.take().is_some() && self.keyboard {
            let row = interfaces::row_id(card);
            ctx.memory_mut(|m| m.request_focus(row));
            self.refocus = Some(row);
        }
    }

    /// Closes the open card; with the keyboard in use, its header takes the focus.
    fn close(&mut self, ctx: &egui::Context) {
        if let Some((card, _)) = self.open.take()
            && self.keyboard
        {
            ctx.memory_mut(|m| m.request_focus(header_id(&card)));
        }
    }

    /// The cards, each with its switch, or the settings of one.
    fn interfaces(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let Network { cards, page, root, notice, .. } = self;
        if let Some(notice) = notice {
            ui.label(RichText::new(notice.as_str()).font(theme::body(13.0)).color(RED));
        }
        match cards {
            None => {
                ui.label(RichText::new("Looking for network cards…").color(DIM));
            }
            Some(Err(why)) => message(ui, why),
            Some(Ok(cards)) => match page {
                Some(page) => actions.extend(interfaces::page(ui, cards, page, root.as_ref())),
                None => actions.extend(interfaces::list(ui, cards, root.as_ref())),
            },
        }
    }

    /// The window's contents; what was asked for in them.
    fn page(&mut self, ui: &mut Ui) -> Vec<Action> {
        let mut actions = Vec::new();
        let frame = egui::Frame::new().fill(BACKGROUND).inner_margin(Margin::symmetric(24, 20));
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            let title = Network::header(ui).rect;
            let title_row = Rect::from_x_y_ranges(ui.max_rect().x_range(), title.y_range());
            ui.add_space(14.0);
            match self.section {
                Section::Wifi => {
                    self.wifi_heading(ui, &mut actions);
                    ui.add_space(6.0);
                    self.list(ui, &mut actions);
                }
                Section::Interfaces => self.interfaces(ui, &mut actions),
            }
            // At the right of the title's row, but added last, so the section's own widgets come
            // first from the keyboard.
            let tabs = [(Section::Wifi, "Wi-Fi"), (Section::Interfaces, "Interfaces")];
            let width =
                tabs.iter().map(|(_, label)| small_button_width(ui, label)).sum::<f32>() + ui.spacing().item_spacing.x;
            let row = Rect::from_x_y_ranges((title_row.right() - width)..=title_row.right(), title_row.y_range());
            ui.scope_builder(UiBuilder::new().max_rect(row).layout(Layout::left_to_right(Align::Center)), |ui| {
                for (section, label) in tabs {
                    let shown = self.section == section;
                    if small_button(ui, Id::new(("tab", label)), label, shown).clicked() && !shown {
                        actions.push(Action::Section(section));
                    }
                }
            });
        });
        actions
    }
}

impl eframe::App for Network {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        self.refresh_radio(&ctx);
        self.watch_start(ctx.input(|i| i.time));
        self.refresh_cards(&ctx);
        if self.busy.is_some() && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            STOP.store(true, Ordering::Relaxed);
            self.closing = true;
        }
        self.input(&ctx);
        let actions = self.page(ui);
        self.give_focus_back(&ctx);
        for action in actions {
            self.act(action);
        }
    }
}

/// What a card needs to know besides its network.
struct Shown<'a> {
    status: &'a Status,
    /// What the worker is doing for this card.
    doing: Option<Doing>,
    /// Whether the worker or the root helper is doing anything.
    busy: bool,
}

/// What a card's header shows.
struct Header<'a> {
    name: &'a str,
    caption: &'a str,
    caption_color: Color32,
    /// Bars for the signal; none for a network out of range or Other network.
    strength: Option<Strength>,
    lit: Color32,
    secured: bool,
    /// Whether a click or Enter opens the card.
    opens: bool,
}

/// A card: its header, and under it, while it's open, an error, Stop during a join, or `body`.
fn card(
    ui: &mut Ui,
    card: &Card,
    header: &Header,
    form: Option<&mut Form>,
    joining: bool,
    body: impl FnOnce(&mut Ui, &mut Form) -> Option<Action>,
) -> Option<Action> {
    let mut action = None;
    let is_open = form.is_some();
    let mut prepared = egui::Frame::new().corner_radius(RADIUS).begin(ui);
    {
        let ui = &mut prepared.content_ui;
        if card_header(ui, card, header).clicked() {
            action = Some(Action::Toggle(card.clone()));
        }
        if let Some(form) = form {
            form_body(ui, |ui| {
                if let Some(error) = &form.error {
                    ui.label(RichText::new(error).font(theme::body(13.0)).color(RED));
                }
                if joining {
                    if right_button(ui, header_id(card).with("stop"), "Stop", FAINT, TEXT, true).clicked() {
                        action = Some(Action::Stop);
                    }
                } else if let Some(asked) = body(ui, form) {
                    action = Some(asked);
                }
            });
        }
    }
    let hovered = prepared.content_ui.ui_contains_pointer() && header.opens;
    prepared.frame.fill = if is_open || hovered { SURFACE_HOVER } else { SURFACE };
    prepared.end(ui);
    action
}

/// The id of a card's header, the same in every frame.
fn header_id(card: &Card) -> Id {
    Id::new(("card", card))
}

/// A card's header: signal bars, the name and its caption, a padlock; the response to a click.
fn card_header(ui: &mut Ui, card: &Card, header: &Header) -> egui::Response {
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), HEADER));
    let response = ui.interact(rect, header_id(card), sense(header.opens));
    signal_bars(ui, rect, header.strength, header.lit);
    if header.secured {
        padlock(ui, pos2(rect.right() - 24.0, rect.center().y));
    }
    let width = rect.right() - if header.secured { 40.0 } else { 16.0 } - rect.left() - TEXT_LEFT;
    let painter = ui.painter();
    let name = one_line(ui, header.name, theme::semibold(15.0), TEXT, width);
    painter.galley(pos2(rect.left() + TEXT_LEFT, rect.top() + 9.0), name, TEXT);
    let caption = one_line(ui, header.caption, theme::body(13.0), header.caption_color, width);
    painter.galley(pos2(rect.left() + TEXT_LEFT, rect.top() + 31.0), caption, header.caption_color);
    if response.has_focus() {
        ui.painter().rect_stroke(rect.shrink(1.0), RADIUS, Stroke::new(1.5, ORANGE), StrokeKind::Inside);
    }
    response
}

/// A network's card; open, what can be done with it.
fn network_card(ui: &mut Ui, row: &Row, shown: &Shown, form: Option<&mut Form>) -> Option<Action> {
    let joinable = row.security.is_some_and(Security::joinable);
    let (caption, caption_color) = match shown.doing {
        Some(Doing::Join) => ("Connecting…".to_owned(), ORANGE),
        Some(Doing::Forget) => ("Forgetting…".to_owned(), DIM),
        Some(Doing::Autoconnect) => ("Saving…".to_owned(), DIM),
        Some(Doing::Disconnect) => ("Disconnecting…".to_owned(), DIM),
        _ if row.connected => (view::caption(row, shown.status.address.as_deref()), TEAL),
        _ => (view::caption(row, None), DIM),
    };
    let header = Header {
        name: &row.ssid.display(),
        caption: &caption,
        caption_color,
        strength: row.signal.map(Strength::from_dbm),
        lit: if row.connected { TEAL } else { TEXT },
        secured: row.security.is_some_and(|s| s != Security::Open),
        opens: !shown.busy && (joinable || !row.saved.is_empty()),
    };
    let this = Card::Network(row.ssid.clone());
    let joining = shown.doing == Some(Doing::Join);
    card(ui, &this, &header, form, joining, |ui, form| network_actions(ui, row, shown.busy, form, &this))
}

/// What can be done with a network: its password if one is needed, Connect automatically, then
/// Forget and Connect, or Disconnect for the network joined, on a row of their own.
fn network_actions(ui: &mut Ui, row: &Row, busy: bool, form: &mut Form, this: &Card) -> Option<Action> {
    let submitted = view::password_hint(row, form.retype).is_some_and(|hint| password_field(ui, form, hint));
    let mut action = None;
    if !row.saved.is_empty() {
        let mut on = row.autoconnect;
        let id = header_id(this).with("autoconnect");
        if with_id(ui, id, |ui| checkbox(ui, &mut on, "Connect automatically", !busy)).changed() {
            let request = Request::Autoconnect { ssid: row.ssid.clone(), on };
            action = Some(Action::Ask(request, Some(this.clone()), Doing::Autoconnect));
        }
    }
    right_row(ui, |ui| {
        if view::connectable(row)
            && (button(ui, header_id(this).with("connect"), "Connect", ORANGE, BACKGROUND, !busy).clicked()
                || submitted)
        {
            match view::connect(row, &form.password, form.retype) {
                Ok(request) => action = Some(Action::Ask(request, Some(this.clone()), Doing::Join)),
                Err(e) => form.error = Some(view::sentence(&e)),
            }
        }
        if row.connected && button(ui, header_id(this).with("disconnect"), "Disconnect", FAINT, TEXT, !busy).clicked() {
            action = Some(Action::Ask(Request::Disconnect, Some(this.clone()), Doing::Disconnect));
        }
        if !row.saved.is_empty() && button(ui, header_id(this).with("forget"), "Forget", FAINT, TEXT, !busy).clicked() {
            let request = Request::Forget { ssid: row.ssid.clone() };
            action = Some(Action::Ask(request, Some(this.clone()), Doing::Forget));
        }
    });
    action
}

/// The Other network card: a name and a password for a network that hides its name.
fn other_card(ui: &mut Ui, form: Option<&mut Form>, doing: Option<Doing>, busy: bool) -> Option<Action> {
    let joining = doing == Some(Doing::Join);
    let (caption, caption_color) =
        if joining { ("Connecting…", ORANGE) } else { ("A network that hides its name", DIM) };
    let header = Header {
        name: "Other network…",
        caption,
        caption_color,
        strength: None,
        lit: TEXT,
        secured: false,
        opens: !busy,
    };
    card(ui, &Card::Other, &header, form, joining, |ui, form| {
        let name = ui.add(text_field(&mut form.name, "Network name", false));
        if !form.focused {
            name.request_focus();
            form.focused = true;
        }
        let submitted = password_field(ui, form, "Password, if it has one");
        let connect = header_id(&Card::Other).with("connect");
        if !(right_button(ui, connect, "Connect", ORANGE, BACKGROUND, !busy).clicked() || submitted) {
            return None;
        }
        match view::connect_hidden(&form.name, &form.password) {
            Ok(request) => Some(Action::Ask(request, Some(Card::Other), Doing::Join)),
            Err(e) => {
                form.error = Some(view::sentence(&e));
                None
            }
        }
    })
}

/// The open part of a card, under the name, with room on the right.
fn form_body(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new().inner_margin(Margin { left: TEXT_LEFT as i8, right: 16, top: 0, bottom: 14 }).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 10.0;
        add(ui);
    });
}

/// A password field and its Show password box; true when Enter was pressed in it.
fn password_field(ui: &mut Ui, form: &mut Form, hint: &str) -> bool {
    let field = ui.add(text_field(&mut form.password, hint, !form.show));
    if !form.focused {
        field.request_focus();
        form.focused = true;
    }
    checkbox(ui, &mut form.show, "Show password", true);
    field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

fn text_field<'a>(text: &'a mut String, hint: &'a str, hidden: bool) -> egui::TextEdit<'a> {
    egui::TextEdit::singleline(text)
        .hint_text(hint)
        .password(hidden)
        .font(theme::body(15.0))
        .margin(Margin::symmetric(10, 8))
        .desired_width(f32::INFINITY)
}

/// A card's message in place of the list: why Wi-Fi can't be shown.
fn message(ui: &mut Ui, text: &str) {
    egui::Frame::new().fill(SURFACE).corner_radius(RADIUS).inner_margin(Margin::same(16)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(text).color(TEXT));
    });
}

fn sense(clickable: bool) -> Sense {
    if clickable { Sense::click() } else { Sense::hover() }
}

/// Four bars, as many lit as the signal's strength; none out of range.
fn signal_bars(ui: &Ui, rect: Rect, strength: Option<Strength>, lit: Color32) {
    let Some(strength) = strength else { return };
    let lit_bars = match strength {
        Strength::Weak => 1,
        Strength::Fair => 2,
        Strength::Good => 3,
        Strength::Strong => 4,
    };
    let base = pos2(rect.left() + 17.0, rect.center().y + 9.0);
    for bar in 0..4 {
        let height = 6.0 + 4.0 * bar as f32;
        let left = base.x + 6.0 * bar as f32;
        let bar_rect = Rect::from_min_max(pos2(left, base.y - height), pos2(left + 4.0, base.y));
        let color = if bar < lit_bars { lit } else { FAINT };
        ui.painter().rect_filled(bar_rect, 1, color);
    }
}

/// A padlock centred at `at`, for a network that isn't open.
fn padlock(ui: &Ui, at: egui::Pos2) {
    let painter = ui.painter();
    let shackle = Rect::from_center_size(at + vec2(0.0, -4.0), vec2(8.0, 10.0));
    painter.rect_stroke(shackle, CornerRadius::same(4), Stroke::new(1.6, DIM), StrokeKind::Middle);
    let body = Rect::from_center_size(at + vec2(0.0, 2.5), vec2(12.0, 9.0));
    painter.rect_filled(body, 2, DIM);
}

/// A filled button, dimmed when it can't be pressed.
fn button(ui: &mut Ui, id: Id, label: &str, fill: Color32, text: Color32, enabled: bool) -> egui::Response {
    let (fill, text) = if enabled { (fill, text) } else { (FAINT.gamma_multiply(0.6), DIM) };
    let button = egui::Button::new(RichText::new(label).font(theme::semibold(14.0)).color(text))
        .fill(fill)
        .corner_radius(RADIUS)
        .min_size(BUTTON);
    let response = with_id(ui, id, |ui| ui.add_enabled(enabled, button));
    if response.has_focus() {
        ui.painter().rect_stroke(response.rect.expand(2.0), RADIUS, Stroke::new(1.5, ORANGE), StrokeKind::Outside);
    }
    if enabled { response.on_hover_cursor(CursorIcon::PointingHand) } else { response }
}

/// A small button that fits its label, lit or not: a section's tab, or Back.
fn small_button(ui: &mut Ui, id: Id, label: &str, lit: bool) -> egui::Response {
    let text = RichText::new(label).font(theme::semibold(13.0)).color(if lit { TEXT } else { DIM });
    let fill = if lit { SURFACE_HOVER } else { Color32::TRANSPARENT };
    let button = egui::Button::new(text).fill(fill).corner_radius(RADIUS).min_size(vec2(0.0, 28.0));
    let response = with_id(ui, id, |ui| ui.add(button));
    if response.has_focus() {
        ui.painter().rect_stroke(response.rect.expand(2.0), RADIUS, Stroke::new(1.5, ORANGE), StrokeKind::Outside);
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// How wide `small_button` draws `label`.
fn small_button_width(ui: &Ui, label: &str) -> f32 {
    let text = ui.painter().layout_no_wrap(label.to_owned(), theme::semibold(13.0), TEXT);
    text.size().x + 2.0 * ui.spacing().button_padding.x
}

/// An on/off switch named `name`, dimmed when it can't be used; the response to a click.
fn switch(ui: &mut Ui, name: &str, on: bool, enabled: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(44.0, 24.0), sense(enabled));
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, on, name));
    let track = match (enabled, on) {
        (false, _) => FAINT.gamma_multiply(0.6),
        (true, true) => TEAL,
        (true, false) => FAINT,
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(12), track);
    let knob = if on { rect.right() - 12.0 } else { rect.left() + 12.0 };
    painter.circle_filled(pos2(knob, rect.center().y), 9.0, if enabled { TEXT } else { DIM });
    if response.has_focus() {
        painter.rect_stroke(rect.expand(2.0), CornerRadius::same(14), Stroke::new(1.5, ORANGE), StrokeKind::Outside);
    }
    if enabled { response.on_hover_cursor(CursorIcon::PointingHand) } else { response }
}

/// A check box with nearly square corners, dimmed when it can't be used.
fn checkbox(ui: &mut Ui, on: &mut bool, label: &str, enabled: bool) -> egui::Response {
    ui.scope(|ui| {
        let w = &mut ui.visuals_mut().widgets;
        for state in [&mut w.inactive, &mut w.hovered, &mut w.active] {
            state.corner_radius = CornerRadius::same(2);
        }
        ui.add_enabled(enabled, egui::Checkbox::new(on, label))
    })
    .inner
}

/// The root helper through pkexec, doing `root`.
fn root_command(root: &Root) -> Command {
    let mut command = Command::new("pkexec");
    command.arg(HELPER).args(root.args()).stdin(Stdio::null());
    command
}

/// Runs the root helper; how it ended.
fn run_root(root: Root) -> Ended {
    match root_command(&root).output() {
        Ok(out) => root_ended(out.status.code(), &String::from_utf8_lossy(&out.stderr)),
        Err(e) => Ended::Failed(format!("Couldn't run pkexec: {e}.")),
    }
}

/// How the root helper ended, from pkexec's exit status and the helper's errors.
fn root_ended(code: Option<i32>, stderr: &str) -> Ended {
    match code {
        Some(0) => Ended::Done,
        Some(PKEXEC_DISMISSED) => Ended::Dismissed,
        Some(PKEXEC_REFUSED) if stderr.contains("authentication agent") => {
            Ended::Failed("No polkit agent is running to ask for the password.".into())
        }
        Some(PKEXEC_REFUSED) => Ended::Failed("Not allowed: it needs an administrator (a member of wheel).".into()),
        _ => {
            let line = stderr.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("the helper failed");
            Ended::Failed(view::as_sentence(line.strip_prefix("hyprbsd-network-helper: ").unwrap_or(line)))
        }
    }
}

/// Scrolls to what the keyboard has just moved to.
fn follow_focus(ui: &Ui) {
    if let Some(focused) = ui.memory(|m| m.focused()).and_then(|id| ui.ctx().read_response(id))
        && focused.gained_focus()
    {
        focused.scroll_to_me(None);
    }
}

/// A button at the right of its row.
fn right_button(ui: &mut Ui, id: Id, label: &str, fill: Color32, text: Color32, enabled: bool) -> egui::Response {
    right_row(ui, |ui| button(ui, id, label, fill, text, enabled))
}

/// What `add` adds, in a scope with the id `id`, so the widget's id stays the same when widgets
/// above it come and go.
fn with_id<R>(ui: &mut Ui, id: Id, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope_builder(UiBuilder::new().id(id), add).inner
}

/// A row of buttons, the first added at the right.
fn right_row<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let row = vec2(ui.available_width(), BUTTON.y);
    ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), add).inner
}

/// Text cut to one line of the width, with an ellipsis where it's cut.
fn one_line(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap =
        TextWrapping { max_width: width.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    ui.painter().layout_job(job)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wpa::WpaError;
    use eframe::egui::epaint::Shape;
    use eframe::egui::{Event, Modifiers, PointerButton, Pos2, RawInput};
    use std::sync::Mutex;

    /// A window listing `above`, then `row` with its card open, disconnected.
    fn open(above: Vec<Row>, row: Row) -> Network {
        let (requests, _) = mpsc::channel();
        let (_, updates) = mpsc::channel();
        let status = Status { state: "DISCONNECTED".into(), ssid: None, id: None, address: None };
        let card = Card::Network(row.ssid.clone());
        let mut rows = above;
        rows.push(row);
        let wifi = Wifi { status, rows, scanning: false, age: None };
        let (root_send, root_done) = mpsc::channel();
        Network {
            requests,
            updates,
            wifi: Some(Ok(wifi)),
            open: Some((card, Form::default())),
            busy: None,
            notice: None,
            closing: false,
            keyboard: false,
            down: false,
            radio: None,
            radio_reader: Reader::new(|| panic!("a test read this machine's Wi-Fi")),
            starting: None,
            section: Section::Wifi,
            cards: None,
            cards_reader: Reader::new(|| panic!("a test read this machine's cards")),
            page: None,
            refocus: None,
            root: None,
            root_helper: |_| panic!("a test ran the root helper"),
            root_done,
            root_send,
            ctx: egui::Context::default(),
        }
    }

    /// A window with Home saved, out of range, and no card open.
    pub(super) fn wifi_window() -> Network {
        let mut network = open(Vec::new(), saved(None));
        network.open = None;
        network
    }

    fn saved(security: Option<Security>) -> Row {
        Row {
            ssid: Ssid::new(b"Home").unwrap(),
            signal: security.map(|_| -40),
            security,
            connected: false,
            saved: vec![0],
            autoconnect: true,
        }
    }

    /// An open network in range, not saved.
    fn in_range(name: &str) -> Row {
        Row {
            ssid: Ssid::new(name.as_bytes()).unwrap(),
            signal: Some(-60),
            security: Some(Security::Open),
            connected: false,
            saved: Vec::new(),
            autoconnect: false,
        }
    }

    /// Draws the window at its smallest size, given `events`: each text drawn, where, and what
    /// was asked for.
    pub(super) fn draw(
        ctx: &egui::Context,
        network: &mut Network,
        events: Vec<Event>,
    ) -> (Vec<(String, Rect)>, Vec<Action>) {
        draw_input(ctx, network, RawInput { events, ..smallest() })
    }

    /// Draws a frame as `draw` does, the window as narrow but tall enough for a lagg's page.
    pub(super) fn draw_tall(
        ctx: &egui::Context,
        network: &mut Network,
        events: Vec<Event>,
    ) -> (Vec<(String, Rect)>, Vec<Action>) {
        draw_input(ctx, network, RawInput { events, ..tall() })
    }

    /// A frame's input for the window at its smallest size.
    fn smallest() -> RawInput {
        RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SMALLEST)), ..Default::default() }
    }

    /// A frame's input for the window as narrow as it goes, but tall enough for a lagg's page.
    fn tall() -> RawInput {
        RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(SMALLEST.x, 900.0))), ..Default::default() }
    }

    /// Draws a frame of the window: each text drawn, where, and what was asked for.
    fn draw_input(ctx: &egui::Context, network: &mut Network, input: RawInput) -> (Vec<(String, Rect)>, Vec<Action>) {
        let (shapes, actions) = frame(ctx, network, input);
        let texts = shapes
            .into_iter()
            .filter_map(|shape| match shape {
                Shape::Text(text) => {
                    Some((text.galley.text().to_owned(), text.galley.rect.translate(text.pos.to_vec2())))
                }
                _ => None,
            })
            .collect();
        (texts, actions)
    }

    /// Draws a frame of the window, as it does each frame but for the worker's updates: every
    /// shape drawn, and what was asked for.
    fn frame(ctx: &egui::Context, network: &mut Network, input: RawInput) -> (Vec<Shape>, Vec<Action>) {
        let mut actions = Vec::new();
        let mut output = ctx.run_ui(input, |ui| {
            network.refresh_radio(ui.ctx());
            network.watch_start(ui.input(|i| i.time));
            network.refresh_cards(ui.ctx());
            network.input(ui.ctx());
            actions = network.page(ui);
            network.give_focus_back(ui.ctx());
        });
        // Nothing draws the textures.
        output.textures_delta.clear();
        let mut shapes = Vec::new();
        for clipped in output.shapes {
            flatten(clipped.shape, &mut shapes);
        }
        (shapes, actions)
    }

    fn flatten(shape: Shape, out: &mut Vec<Shape>) {
        match shape {
            Shape::Vec(shapes) => shapes.into_iter().for_each(|shape| flatten(shape, out)),
            shape => out.push(shape),
        }
    }

    pub(super) fn text(texts: &[(String, Rect)], label: &str) -> Option<Rect> {
        texts.iter().find(|(text, _)| text == label).map(|(_, rect)| *rect)
    }

    pub(super) fn press(at: Pos2, pressed: bool) -> Vec<Event> {
        let button =
            Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        vec![Event::PointerMoved(at), button]
    }

    pub(super) fn key(key: egui::Key) -> Event {
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }
    }

    pub(super) fn themed() -> egui::Context {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        ctx
    }

    /// A join of Home running.
    fn joining() -> Option<Busy> {
        Some(Busy { card: Some(Card::Network(Ssid::new(b"Home").unwrap())), doing: Doing::Join })
    }

    #[test]
    fn a_click_on_connect_automatically_turns_it_off_at_the_smallest_size() {
        let ctx = themed();
        let mut network = open(Vec::new(), saved(Some(Security::Wpa2)));
        let (texts, _) = draw(&ctx, &mut network, Vec::new());
        let at = text(&texts, "Connect automatically").unwrap().center();

        draw(&ctx, &mut network, press(at, true));
        let (_, actions) = draw(&ctx, &mut network, press(at, false));

        assert!(matches!(actions[..], [Action::Ask(Request::Autoconnect { on: false, .. }, ..)]), "{actions:?}");
    }

    #[test]
    fn tab_after_connect_automatically_goes_on_to_disconnect_whether_it_saved_or_not() {
        for saved in [true, false] {
            let ctx = themed();
            let mut network = joined(0);
            let (requests, _asked) = mpsc::channel();
            network.requests = requests;
            network.ctx = ctx.clone();
            draw(&ctx, &mut network, Vec::new());
            // The switch, Rescan, Home's card, then its checkbox.
            for _ in 0..4 {
                draw(&ctx, &mut network, vec![key(egui::Key::Tab)]);
            }
            let (_, actions) = draw(&ctx, &mut network, vec![key(egui::Key::Space)]);
            actions.into_iter().for_each(|action| network.act(action));
            draw(&ctx, &mut network, Vec::new());
            network.done(&ctx, if saved { Ok(()) } else { Err(WifiError::NotKept("Home".into())) });
            draw(&ctx, &mut network, Vec::new());

            draw(&ctx, &mut network, vec![key(egui::Key::Tab)]);
            let (_, actions) = draw(&ctx, &mut network, vec![key(egui::Key::Space)]);

            assert!(matches!(actions[..], [Action::Ask(Request::Disconnect, ..)]), "saved {saved}: {actions:?}");
        }
    }

    #[test]
    fn tab_after_the_switch_goes_on_to_rescan_once_the_helper_ends() {
        let ctx = themed();
        let mut network = joined(0);
        network.open = None;
        network.ctx = ctx.clone();
        network.root_helper = |_| Ended::Done;
        draw(&ctx, &mut network, Vec::new());
        draw(&ctx, &mut network, vec![key(egui::Key::Tab)]);
        let (_, actions) = draw(&ctx, &mut network, vec![key(egui::Key::Space)]);
        actions.into_iter().for_each(|action| network.act(action));
        for _ in 0..3 {
            draw(&ctx, &mut network, Vec::new());
        }
        let ended = network.root_done.recv_timeout(Duration::from_secs(5)).expect("the helper's end");
        network.root_finished(&ctx, ended, 0.0);
        draw(&ctx, &mut network, Vec::new());

        draw(&ctx, &mut network, vec![key(egui::Key::Tab)]);
        let (_, actions) = draw(&ctx, &mut network, vec![key(egui::Key::Space)]);

        assert!(matches!(actions[..], [Action::Ask(Request::Rescan, ..)]), "{actions:?}");
    }

    #[test]
    fn tab_after_rescan_goes_on_from_it_once_the_scan_ends() {
        let ctx = themed();
        let mut network = joined(0);
        network.open = None;
        let (requests, _asked) = mpsc::channel();
        network.requests = requests;
        network.ctx = ctx.clone();
        draw(&ctx, &mut network, Vec::new());
        // The switch, then Rescan.
        for _ in 0..2 {
            draw(&ctx, &mut network, vec![key(egui::Key::Tab)]);
        }
        let (_, actions) = draw(&ctx, &mut network, vec![key(egui::Key::Space)]);
        actions.into_iter().for_each(|action| network.act(action));
        draw(&ctx, &mut network, Vec::new());
        network.done(&ctx, Ok(()));
        for scanning in [true, true, false] {
            if let Some(Ok(wifi)) = &mut network.wifi {
                wifi.scanning = scanning;
            }
            draw(&ctx, &mut network, Vec::new());
        }

        draw(&ctx, &mut network, vec![key(egui::Key::Tab)]);
        let (_, actions) = draw(&ctx, &mut network, vec![key(egui::Key::Space)]);

        assert!(matches!(actions[..], [Action::Toggle(Card::Network(_))]), "{actions:?}");
    }

    #[test]
    fn the_list_follows_the_keyboard_to_a_button_out_of_sight() {
        let ctx = themed();
        let cafes = (1..=6).map(|n| in_range(&format!("Cafe {n}"))).collect();
        let mut network = open(cafes, saved(None));
        draw(&ctx, &mut network, Vec::new());

        // The switch, Rescan, the six cafes, Home's card, its checkbox, then its Connect.
        for _ in 0..11 {
            draw(&ctx, &mut network, vec![key(egui::Key::Tab)]);
        }
        // Half a second of frames, for the scroll's animation.
        for _ in 0..30 {
            draw(&ctx, &mut network, Vec::new());
        }
        let (texts, _) = draw(&ctx, &mut network, Vec::new());

        // A widget out of sight isn't drawn.
        assert!(text(&texts, "Connect").is_some_and(|connect| connect.max.y <= SMALLEST.y), "{texts:?}");
    }

    #[test]
    fn escape_leaves_the_keyboard_on_the_card_it_closes() {
        let ctx = themed();
        let mut network = open(Vec::new(), saved(Some(Security::Wpa2)));
        draw(&ctx, &mut network, Vec::new());

        draw(&ctx, &mut network, vec![key(egui::Key::Escape)]);
        let (_, actions) = draw(&ctx, &mut network, vec![key(egui::Key::Space)]);

        assert!(matches!(actions[..], [Action::Toggle(Card::Network(_))]), "{actions:?}");
    }

    #[test]
    fn a_saved_network_out_of_range_that_fails_to_join_asks_for_a_new_password() {
        let ctx = themed();
        let mut network = open(Vec::new(), saved(None));
        network.busy = joining();
        network.done(&ctx, Err(WifiError::WrongPassword("Home".into())));

        let (texts, _) = draw(&ctx, &mut network, Vec::new());

        assert!(text(&texts, "New password, if it changed").is_some(), "{texts:?}");
    }

    #[test]
    fn a_join_made_from_the_keyboard_leaves_it_on_the_card() {
        let ctx = themed();
        let mut network = open(Vec::new(), saved(Some(Security::Wpa2)));
        draw(&ctx, &mut network, vec![key(egui::Key::Tab)]);
        network.busy = joining();

        network.done(&ctx, Ok(()));
        let (_, actions) = draw(&ctx, &mut network, vec![key(egui::Key::Space)]);

        assert!(matches!(actions[..], [Action::Toggle(Card::Network(_))]), "{actions:?}");
    }

    #[test]
    fn a_join_made_with_the_pointer_leaves_nothing_focused() {
        let ctx = themed();
        let mut network = open(Vec::new(), saved(Some(Security::Wpa2)));
        draw(&ctx, &mut network, press(pos2(5.0, 5.0), true));
        draw(&ctx, &mut network, press(pos2(5.0, 5.0), false));
        network.busy = joining();

        network.done(&ctx, Ok(()));
        draw(&ctx, &mut network, Vec::new());

        assert_eq!(ctx.memory(|m| m.focused()), None);
    }

    /// A window where wpa_supplicant can't be reached, with Wi-Fi as `radio`.
    fn down(radio: Radio) -> Network {
        let mut network = open(Vec::new(), saved(None));
        network.open = None;
        network.wifi = Some(Err("Wi-Fi isn't set up: wpa_supplicant has no control socket.".into()));
        network.down = true;
        network.radio = Some(radio);
        network
    }

    /// What Space asks for after the first Tab.
    fn first_widget(network: &mut Network) -> Vec<Action> {
        let ctx = themed();
        draw(&ctx, network, Vec::new());
        draw(&ctx, network, vec![key(egui::Key::Tab)]);
        draw(&ctx, network, vec![key(egui::Key::Space)]).1
    }

    #[test]
    fn wifi_off_says_so() {
        let (texts, _) = draw(&themed(), &mut down(Radio::Off), Vec::new());

        assert!(text(&texts, "Wi-Fi is off.").is_some(), "{texts:?}");
    }

    #[test]
    fn turning_wifi_on_says_so_while_the_helper_runs() {
        let mut network = down(Radio::Off);
        network.root = Some(Root::WifiOn);

        let (texts, _) = draw(&themed(), &mut network, Vec::new());

        assert!(text(&texts, "Turning Wi-Fi on…").is_some(), "{texts:?}");
    }

    #[test]
    fn wifi_on_before_wpa_supplicant_answers_says_its_turning_on() {
        let (texts, _) = draw(&themed(), &mut down(Radio::On), Vec::new());

        assert!(text(&texts, "Turning Wi-Fi on…").is_some(), "{texts:?}");
    }

    #[test]
    fn wifi_turned_on_says_so_until_wpa_supplicant_answers() {
        let ctx = themed();
        let mut network = down(Radio::Off);
        network.radio_reader.read = || Radio::On;
        network.root = Some(Root::WifiOn);
        draw(&ctx, &mut network, Vec::new());
        network.root_finished(&ctx, Ended::Done, 0.0);

        let (texts, _) = draw(&ctx, &mut network, Vec::new());

        assert!(text(&texts, "Turning Wi-Fi on…").is_some(), "{texts:?}");
    }

    #[test]
    fn wifi_on_with_no_wpa_supplicant_for_a_while_says_why() {
        let ctx = themed();
        let mut network = down(Radio::On);
        network.radio_reader.read = || Radio::On;
        draw_input(&ctx, &mut network, RawInput { time: Some(0.0), ..smallest() });

        let (texts, _) = draw_input(&ctx, &mut network, RawInput { time: Some(STARTING), ..smallest() });

        let why = "Wi-Fi isn't set up: wpa_supplicant has no control socket.";
        assert!(text(&texts, why).is_some(), "{texts:?}");
    }

    #[test]
    fn turning_wifi_off_doesnt_say_its_turning_on() {
        let mut network = down(Radio::On);
        network.root = Some(Root::WifiOff);

        let (texts, _) = draw(&themed(), &mut network, Vec::new());

        assert!(text(&texts, "Turning Wi-Fi on…").is_none(), "{texts:?}");
    }

    #[test]
    fn the_switch_turns_wifi_on_when_its_off() {
        let actions = first_widget(&mut down(Radio::Off));

        assert!(matches!(actions[..], [Action::Root(Root::WifiOn)]), "{actions:?}");
    }

    #[test]
    fn the_switch_turns_wifi_off_when_its_on() {
        let mut network = open(Vec::new(), saved(Some(Security::Wpa2)));
        network.open = None;

        let actions = first_widget(&mut network);

        assert!(matches!(actions[..], [Action::Root(Root::WifiOff)]), "{actions:?}");
    }

    #[test]
    fn wifi_not_set_up_offers_to_set_it_up() {
        let actions = first_widget(&mut down(Radio::NotSetUp));

        assert!(matches!(actions[..], [Action::Root(Root::SetUp)]), "{actions:?}");
    }

    #[test]
    fn no_adapter_says_so_and_has_no_switch() {
        let mut network = down(Radio::NoAdapter);
        let (texts, _) = draw(&themed(), &mut network, Vec::new());

        let actions = first_widget(&mut network);

        assert!(text(&texts, "No Wi-Fi adapter.").is_some() && actions.is_empty(), "{texts:?} {actions:?}");
    }

    #[test]
    fn set_up_runs_the_helpers_wifi_setup_through_pkexec() {
        let command = root_command(&Root::SetUp);

        let words: Vec<&std::ffi::OsStr> = std::iter::once(command.get_program()).chain(command.get_args()).collect();

        assert_eq!(words, ["pkexec", HELPER, "wifi-setup"]);
    }

    mod root_ended {
        use super::super::{Ended, root_ended};

        #[test]
        fn a_password_box_dismissed_is_dismissed() {
            assert_eq!(root_ended(Some(126), ""), Ended::Dismissed);
        }

        #[test]
        fn no_polkit_agent_says_so() {
            let stderr = "Error executing command as another user: No authentication agent found.\n";

            assert_eq!(
                root_ended(Some(127), stderr),
                Ended::Failed("No polkit agent is running to ask for the password.".into())
            );
        }

        #[test]
        fn the_helpers_error_is_why() {
            let stderr = "hyprbsd-network-helper: couldn't start wlan0 (service netif start wlan0).\n";

            assert_eq!(
                root_ended(Some(1), stderr),
                Ended::Failed("Couldn't start wlan0 (service netif start wlan0).".into())
            );
        }
    }

    fn not_set_up() -> Radio {
        Radio::NotSetUp
    }

    #[test]
    fn an_adapter_plugged_in_while_it_is_open_offers_set_up() {
        let ctx = themed();
        let mut network = down(Radio::NoAdapter);
        network.radio_reader.read = not_set_up;
        draw_input(&ctx, &mut network, RawInput { time: Some(0.0), ..smallest() });
        draw_input(&ctx, &mut network, RawInput { time: Some(RADIO_TICK), ..smallest() });
        read_back(&mut network);

        let (texts, _) = draw_input(&ctx, &mut network, RawInput { time: Some(RADIO_TICK), ..smallest() });

        assert!(text(&texts, "Set up Wi-Fi").is_some(), "{texts:?}");
    }

    /// Waits for the read of Wi-Fi the window asked for, and takes it as the window does.
    fn read_back(network: &mut Network) {
        let radio = network.radio_reader.done.recv_timeout(Duration::from_secs(5)).expect("a read of Wi-Fi");
        if let Some(radio) = network.radio_reader.came_back(radio) {
            network.read_radio_done(radio);
        }
    }

    /// A reader with a read running, as if asked for before the system changed.
    fn reading() -> Reader<Radio> {
        let mut reader = Reader::new(|| Radio::On);
        reader.reading = true;
        reader
    }

    #[test]
    fn a_read_running_when_the_system_changes_is_dropped() {
        let mut reader = reading();
        reader.reload(&themed(), 0.0, "Wi-Fi");
        reader.send.send(Radio::Off).unwrap();

        assert_eq!(reader.take(), None);
    }

    #[test]
    fn a_dropped_read_is_due_again_at_once() {
        let mut reader = reading();
        reader.reload(&themed(), 0.0, "Wi-Fi");
        reader.send.send(Radio::Off).unwrap();
        reader.take();

        assert!(reader.due(0.0, RADIO_TICK));
    }

    #[test]
    fn a_read_running_when_the_tick_asks_again_is_kept() {
        let mut reader = reading();
        reader.load(&themed(), RADIO_TICK, "Wi-Fi");
        reader.send.send(Radio::Off).unwrap();

        assert_eq!(reader.take(), Some(Radio::Off));
    }

    #[test]
    fn a_read_of_wifi_from_before_its_turned_on_doesnt_say_its_off() {
        let ctx = themed();
        let mut network = down(Radio::Off);
        network.radio_reader = reading();
        network.root = Some(Root::WifiOn);
        draw(&ctx, &mut network, Vec::new());
        network.root_finished(&ctx, Ended::Done, 0.0);
        network.radio_reader.send.send(Radio::Off).unwrap();
        if let Some(radio) = network.radio_reader.take() {
            network.read_radio_done(radio);
        }

        let (texts, _) = draw(&ctx, &mut network, Vec::new());

        assert!(text(&texts, "Turning Wi-Fi on…").is_some(), "{texts:?}");
    }

    /// wpa_supplicant with no control socket.
    fn gone() -> WifiError {
        WifiError::Wpa(WpaError::NoSocket("/var/run/wpa_supplicant".into()))
    }

    /// The thread `read_where` last ran on.
    static READ_ON: Mutex<Option<thread::ThreadId>> = Mutex::new(None);

    fn read_where() -> Radio {
        *READ_ON.lock().unwrap() = Some(thread::current().id());
        Radio::Off
    }

    #[test]
    fn wifi_is_read_off_the_windows_thread() {
        let mut network = open(Vec::new(), saved(None));
        network.radio_reader.read = read_where;

        network.state(Err(gone()), 0.0);
        read_back(&mut network);

        let read_on = *READ_ON.lock().unwrap();
        assert!(read_on.is_some_and(|id| id != thread::current().id()), "{read_on:?}");
    }

    #[test]
    fn wifi_down_says_looking_until_it_is_read() {
        let mut network = open(Vec::new(), saved(None));
        network.open = None;
        network.radio_reader.read = || Radio::Off;
        network.state(Err(gone()), 0.0);

        let (texts, _) = draw(&themed(), &mut network, Vec::new());

        assert!(text(&texts, "Looking for Wi-Fi…").is_some(), "{texts:?}");
    }

    #[test]
    fn a_read_of_wifi_back_after_wpa_supplicant_answers_again_is_dropped() {
        let mut network = open(Vec::new(), saved(None));
        network.open = None;
        network.radio_reader.read = || Radio::Off;
        network.state(Err(gone()), 0.0);
        network.state(Err(WifiError::NoId(7)), 0.0);
        read_back(&mut network);

        let (texts, _) = draw(&themed(), &mut network, Vec::new());

        assert!(text(&texts, "Wi-Fi is off.").is_none(), "{texts:?}");
    }

    /// A window joined to Home, saved and in range, with its card open; the list `age` seconds old.
    fn joined(age: u32) -> Network {
        let mut network = open(Vec::new(), Row { connected: true, ..saved(Some(Security::Wpa2)) });
        if let Some(Ok(wifi)) = &mut network.wifi {
            let ssid = Some(Ssid::new(b"Home").unwrap());
            wifi.status = Status { state: "COMPLETED".into(), ssid, id: Some(0), address: None };
            wifi.age = Some(age);
        }
        network
    }

    #[test]
    fn joined_says_how_old_the_list_is() {
        let (texts, _) = draw(&themed(), &mut joined(720), Vec::new());

        let line = "Last scanned 12 minutes ago. Rescanning can drop the connection for a moment.";
        assert!(text(&texts, line).is_some(), "{texts:?}");
    }

    #[test]
    fn the_joined_networks_card_disconnects_it() {
        let ctx = themed();
        let mut network = joined(0);
        draw(&ctx, &mut network, Vec::new());

        // The switch, Rescan, Home's card, its checkbox, then Disconnect.
        for _ in 0..5 {
            draw(&ctx, &mut network, vec![key(egui::Key::Tab)]);
        }
        let (_, actions) = draw(&ctx, &mut network, vec![key(egui::Key::Space)]);

        assert!(matches!(actions[..], [Action::Ask(Request::Disconnect, _, Doing::Disconnect)]), "{actions:?}");
    }
}
