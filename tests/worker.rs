//! The window's worker against a model of wpa_supplicant behind a control socket.

mod fake;

use fake::{Air, Fake, NoEvents, Scratch};
use hyprbsd_network::wifi::{self, Ssid, WifiError};
use hyprbsd_network::worker::{self, Request, Update, Wifi};
use hyprbsd_network::wpa::{Control, EventSource, Socket, WpaError};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const LONG: Duration = Duration::from_secs(5);

fn quick(stop: Option<&'static AtomicBool>) -> wifi::Wait {
    wifi::Wait {
        handshake: Duration::from_millis(300),
        address: Duration::from_millis(300),
        poll: Duration::from_millis(5),
        rescan: LONG,
        stop,
    }
}

/// How many scans the model was asked for.
fn scans(harness: &Harness) -> usize {
    harness.model().log.iter().filter(|l| *l == "SCAN").count()
}

fn ssid(name: &[u8]) -> Ssid {
    Ssid::new(name).unwrap()
}

fn psk_hex(passphrase: &str, name: &[u8]) -> String {
    fake::hex(&wifi::psk(passphrase.as_bytes(), &ssid(name)))
}

/// A worker on the model's socket, or on a socket nothing serves yet.
struct Harness {
    scratch: Scratch,
    model: Option<Arc<Mutex<Fake>>>,
    requests: Option<Sender<Request>>,
    updates: Receiver<Update>,
    thread: JoinHandle<()>,
}

impl Harness {
    fn new(fake: Option<Fake>, wait: wifi::Wait) -> Self {
        let scratch = Scratch::new("worker");
        let server = scratch.path("wlan0");
        let model = fake.map(|f| fake::serve(&server, f));
        let dir = scratch.0.clone();
        let open = move || Ok((Socket::connect(&server, &dir)?, Socket::connect(&server, &dir)?.attach()?));
        Harness::on(scratch, model, open, wait)
    }

    /// A worker that connects with `open`.
    fn on<C, E>(
        scratch: Scratch,
        model: Option<Arc<Mutex<Fake>>>,
        open: impl FnMut() -> Result<(C, E), WpaError> + Send + 'static,
        wait: wifi::Wait,
    ) -> Self
    where
        C: Control + 'static,
        E: EventSource + 'static,
    {
        let (requests, from_window) = mpsc::channel();
        let (to_window, updates) = mpsc::channel();
        let thread = thread::spawn(move || {
            worker::run(open, from_window, move |update| drop(to_window.send(update)), wait);
        });
        Harness { scratch, model, requests: Some(requests), updates, thread }
    }

    fn start(fake: Fake) -> Self {
        Harness::new(Some(fake), quick(None))
    }

    fn server(&self) -> PathBuf {
        self.scratch.path("wlan0")
    }

    fn model(&self) -> std::sync::MutexGuard<'_, Fake> {
        self.model.as_ref().unwrap().lock().unwrap()
    }

    fn send(&self, request: Request) {
        self.requests.as_ref().unwrap().send(request).unwrap();
    }

    fn next(&self) -> Update {
        self.updates.recv_timeout(LONG).unwrap()
    }

    /// The next state read, or why it couldn't be.
    fn state(&self) -> Result<Wifi, WifiError> {
        loop {
            if let Update::State(state) = self.next() {
                return state;
            }
        }
    }

    /// The next state read, after it has been read once more since the last request ended.
    fn settled(&self) -> Wifi {
        self.done().unwrap();
        self.state().unwrap()
    }

    /// How the next request ended.
    fn done(&self) -> Result<(), WifiError> {
        loop {
            if let Update::Done(result) = self.next() {
                return result;
            }
        }
    }
}

fn names(wifi: &Wifi) -> Vec<Vec<u8>> {
    wifi.rows.iter().map(|r| r.ssid.as_bytes().to_vec()).collect()
}

fn home() -> Fake {
    let psk = psk_hex("correct horse", b"Home");
    Fake::new().with_air(Air::wpa2(b"Home", -50, &psk)).with_air(Air::open(b"Cafe", -70))
}

/// A model joined to the saved network Home.
fn joined_home() -> Fake {
    let psk = psk_hex("correct horse", b"Home");
    let mut fake = home().with_record(b"Home", &psk, false);
    fake.current = Some(0);
    fake
}

mod start {
    use super::*;

    #[test]
    fn sends_the_networks_in_range() {
        let harness = Harness::start(home());

        let wifi = harness.state().unwrap();

        assert_eq!(names(&wifi), [b"Home".to_vec(), b"Cafe".to_vec()]);
    }

    #[test]
    fn scans_again_every_rescan_while_nothing_is_joined() {
        let harness = Harness::new(Some(home()), wifi::Wait { rescan: Duration::from_millis(100), ..quick(None) });
        harness.state().unwrap();

        thread::sleep(Duration::from_millis(700));

        assert!(scans(&harness) >= 3, "{:?}", harness.model().log);
    }

    #[test]
    fn doesnt_scan_again_while_joined() {
        let harness =
            Harness::new(Some(joined_home()), wifi::Wait { rescan: Duration::from_millis(100), ..quick(None) });
        harness.state().unwrap();

        thread::sleep(Duration::from_millis(700));

        assert_eq!(scans(&harness), 0, "{:?}", harness.model().log);
    }

    /// A worker that scanned with nothing joined and read the results; then Home is joined,
    /// with no event, before a rescan comes due.
    fn joined_after_a_scan() -> Harness {
        let mut fake = joined_home();
        fake.current = None;
        let harness = Harness::new(Some(fake), wifi::Wait { rescan: Duration::from_millis(500), ..quick(None) });
        // The state, then the state with the first scan's results.
        harness.state().unwrap();
        harness.state().unwrap();
        harness.model().current = Some(0);
        harness
    }

    #[test]
    fn doesnt_scan_once_joined_before_the_state_is_read_again() {
        let harness = joined_after_a_scan();

        thread::sleep(Duration::from_secs(1));

        let scanned = harness.model().scans_while_joined;
        assert_eq!(scanned, 0, "{:?}", harness.model().log);
    }

    #[test]
    fn reads_the_state_once_when_a_rescan_comes_due_while_joined() {
        let harness = joined_after_a_scan();
        let before = harness.model().log.len();

        thread::sleep(Duration::from_secs(1));

        let reads = harness.model().log[before..].iter().filter(|l| *l == "STATUS").count();
        assert_eq!(reads, 1, "{:?}", harness.model().log);
    }

    #[test]
    fn doesnt_scan_while_joined_when_the_networks_cant_be_read() {
        let mut fake = joined_home();
        fake.bss_fails = true;
        let harness = Harness::start(fake);
        harness.state().unwrap_err();

        thread::sleep(Duration::from_millis(300));

        let scanned = harness.model().scans_while_joined;
        assert_eq!(scanned, 0, "{:?}", harness.model().log);
    }

    #[test]
    fn scans_once_a_joined_network_drops() {
        let harness = Harness::start(joined_home());
        harness.state().unwrap();
        {
            let mut model = harness.model();
            model.current = None;
            model.events.lock().unwrap().push_back("CTRL-EVENT-DISCONNECTED bssid=02:00:00:00:00:01 reason=4".into());
        }
        // The model's server sends its events after answering any request.
        Socket::connect(&harness.server(), &harness.scratch.0).unwrap().request("PING").unwrap();

        thread::sleep(Duration::from_millis(300));

        assert_eq!(scans(&harness), 1, "{:?}", harness.model().log);
    }

    #[test]
    fn sends_the_state_when_the_scan_is_refused() {
        let mut fake = home();
        fake.scan_fails = true;
        let harness = Harness::start(fake);

        let wifi = harness.state().unwrap();

        assert_eq!(names(&wifi), [b"Home".to_vec(), b"Cafe".to_vec()]);
    }

    #[test]
    fn shows_the_joined_network_connected() {
        let harness = Harness::start(joined_home());

        let wifi = harness.state().unwrap();

        assert!(wifi.rows[0].connected, "{:?}", wifi.rows);
    }
}

mod rescan {
    use super::*;

    #[test]
    fn scans_while_joined() {
        let harness = Harness::start(joined_home());
        harness.state().unwrap();

        harness.send(Request::Rescan);
        harness.done().unwrap();

        assert!(harness.model().log.contains(&"SCAN".to_owned()), "{:?}", harness.model().log);
    }

    #[test]
    fn is_scanning_until_the_results_come() {
        let harness = Harness::start(joined_home());
        harness.state().unwrap();

        harness.send(Request::Rescan);
        harness.done().unwrap();
        let mut seen = Vec::new();
        let deadline = Instant::now() + LONG;
        while Instant::now() < deadline {
            seen.push(harness.state().unwrap().scanning);
            if seen.ends_with(&[true, false]) {
                break;
            }
        }

        assert!(seen.ends_with(&[true, false]), "{seen:?}");
    }
}

mod join {
    use super::*;

    #[test]
    fn saves_the_network() {
        let harness = Harness::start(home());
        harness.state().unwrap();

        harness.send(Request::Join { ssid: ssid(b"Home"), password: Some(b"correct horse".to_vec()), hidden: false });
        harness.done().unwrap();

        let saved: Vec<Vec<u8>> = harness.model().file.clone().unwrap().into_iter().map(|r| r.ssid).collect();
        assert_eq!(saved, [b"Home".to_vec()]);
    }

    #[test]
    fn then_shows_it_connected() {
        let harness = Harness::start(home());
        harness.state().unwrap();

        harness.send(Request::Join { ssid: ssid(b"Home"), password: Some(b"correct horse".to_vec()), hidden: false });
        let wifi = harness.settled();

        assert_eq!((wifi.rows[0].ssid.as_bytes(), wifi.rows[0].connected), (b"Home".as_slice(), true));
    }

    #[test]
    fn joins_an_open_network_without_a_password() {
        let harness = Harness::start(home());
        harness.state().unwrap();

        harness.send(Request::Join { ssid: ssid(b"Cafe"), password: None, hidden: false });

        assert!(harness.done().is_ok());
    }

    #[test]
    fn stopped_leaves_the_configuration_as_it_was() {
        static STOP: AtomicBool = AtomicBool::new(false);
        let slow = wifi::Wait { handshake: LONG, ..quick(Some(&STOP)) };
        let harness = Harness::new(Some(Fake::new().with_record(b"Work", "", false)), slow);
        harness.state().unwrap();
        let before = harness.model().records.clone();

        harness.send(Request::Join { ssid: ssid(b"Away"), password: None, hidden: true });
        thread::sleep(Duration::from_millis(100));
        STOP.store(true, Ordering::Relaxed);
        let err = harness.done().unwrap_err();

        assert_eq!((matches!(err, WifiError::Stopped(_)), harness.model().records.clone()), (true, before));
    }
}

mod disconnect {
    use super::*;

    #[test]
    fn leaves_the_network_then_scans() {
        let harness = Harness::start(joined_home());
        harness.state().unwrap();

        harness.send(Request::Disconnect);
        harness.done().unwrap();

        let model = harness.model();
        let after = model.log.iter().position(|c| c == "DISCONNECT").map_or(&[][..], |i| &model.log[i..]);
        assert!(model.current.is_none() && after.contains(&"SCAN".to_owned()), "{:?}", model.log);
    }
}

#[test]
fn sends_how_old_the_scan_is() {
    let harness = Harness::start(Fake::new().with_air(Air { age: 300, ..Air::open(b"Cafe", -70) }));

    let wifi = harness.state().unwrap();

    assert_eq!(wifi.age, Some(300));
}

mod saved {
    use super::*;

    #[test]
    fn connect_joins_a_saved_network() {
        let psk = psk_hex("correct horse", b"Home");
        let harness = Harness::start(home().with_record(b"Home", &psk, false));
        harness.state().unwrap();

        harness.send(Request::Connect { id: 0 });
        harness.done().unwrap();

        assert_eq!(harness.model().current, Some(0));
    }

    #[test]
    fn forget_removes_it() {
        let harness = Harness::start(joined_home());
        harness.state().unwrap();

        harness.send(Request::Forget { ssid: ssid(b"Home") });
        harness.done().unwrap();

        assert_eq!(harness.model().file, Some(Vec::new()));
    }

    #[test]
    fn autoconnect_off_is_saved() {
        let harness = Harness::start(joined_home());
        harness.state().unwrap();

        harness.send(Request::Autoconnect { ssid: ssid(b"Home"), on: false });
        let wifi = harness.settled();

        assert!(!wifi.rows[0].autoconnect, "{:?}", wifi.rows);
    }
}

mod without_wpa_supplicant {
    use super::*;

    #[test]
    fn says_wifi_isnt_set_up() {
        let harness = Harness::new(None, quick(None));

        let err = harness.state().unwrap_err();

        assert!(err.to_string().starts_with("Wi-Fi isn't set up"), "{err}");
    }

    #[test]
    fn a_request_says_why_it_failed() {
        let harness = Harness::new(None, quick(None));
        harness.state().unwrap_err();

        harness.send(Request::Forget { ssid: ssid(b"Home") });

        assert!(harness.done().unwrap_err().to_string().starts_with("Wi-Fi isn't set up"));
    }

    /// A wpa_supplicant that exited once its sockets were open.
    struct Exited;

    impl Control for Exited {
        fn request(&mut self, _: &str) -> Result<String, WpaError> {
            Err(WpaError::NotRunning(PathBuf::from("wlan0")))
        }
    }

    #[test]
    fn a_request_made_meanwhile_is_answered_when_it_has_gone_again() {
        let mut opens = 0;
        let open = move || {
            opens += 1;
            if opens == 2 { Ok((Exited, NoEvents)) } else { Err(WpaError::NoSocket(PathBuf::from("wlan0"))) }
        };
        let harness = Harness::on(Scratch::new("worker"), None, open, quick(None));
        harness.state().unwrap_err();

        harness.send(Request::Forget { ssid: ssid(b"Home") });
        let err = harness.done().unwrap_err();

        assert!(matches!(err, WifiError::Wpa(WpaError::NotRunning(_))), "{err:?}");
    }

    #[test]
    fn finds_it_once_it_starts() {
        let harness = Harness::new(None, quick(None));
        harness.state().unwrap_err();

        let _model = fake::serve(&harness.server(), home());
        let wifi = loop {
            if let Ok(wifi) = harness.state() {
                break wifi;
            }
        };

        assert_eq!(names(&wifi), [b"Home".to_vec(), b"Cafe".to_vec()]);
    }
}

#[test]
fn ends_when_the_window_closes() {
    let mut harness = Harness::start(home());
    harness.state().unwrap();

    harness.requests = None;
    let deadline = Instant::now() + LONG;
    while !harness.thread.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }

    assert!(harness.thread.is_finished());
}

/// Events the test sends, as wpa_supplicant would to an attached socket.
struct Told(Option<Receiver<String>>);

impl EventSource for Told {
    fn next_event(&mut self, timeout: Duration) -> Result<Option<String>, WpaError> {
        match &self.0 {
            Some(told) => Ok(told.recv_timeout(timeout).ok()),
            None => {
                thread::sleep(timeout);
                Ok(None)
            }
        }
    }
}

/// A worker joined to Home, its events from the test, and the moment wpa_supplicant ends: as
/// wpa_supplicant, its socket then takes requests for a moment but never answers them, and then
/// it's gone.
fn ending() -> (Harness, Sender<String>, Arc<Mutex<Option<Instant>>>) {
    let scratch = Scratch::new("worker");
    let server = scratch.path("wlan0");
    let model = fake::serve(&server, joined_home());
    let (tell, told) = mpsc::channel();
    let mut told = Some(told);
    let ended: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let when = Arc::clone(&ended);
    let dir = scratch.0.clone();
    let open = move || match *when.lock().unwrap() {
        None => Ok((Socket::connect(&server, &dir)?, Told(told.take()))),
        Some(at) if at.elapsed() < Duration::from_millis(300) => {
            thread::sleep(Duration::from_secs(10));
            Err(WpaError::Timeout("ATTACH".into()))
        }
        Some(_) => Err(WpaError::NoSocket(PathBuf::from("wlan0"))),
    };
    let harness = Harness::on(scratch, Some(model), open, quick(None));
    harness.state().unwrap();
    (harness, tell, ended)
}

#[test]
fn says_wpa_supplicant_has_gone_soon_after_it_says_it_is_ending() {
    let (harness, tell, ended) = ending();

    *ended.lock().unwrap() = Some(Instant::now());
    tell.send("CTRL-EVENT-TERMINATING ".to_owned()).unwrap();
    let err = harness.state().unwrap_err();

    assert!(matches!(err, WifiError::Wpa(WpaError::NoSocket(_))), "{err:?}");
}

#[test]
fn answers_a_request_made_as_wpa_supplicant_ends_once_it_has_gone() {
    let (harness, tell, ended) = ending();

    *ended.lock().unwrap() = Some(Instant::now());
    tell.send("CTRL-EVENT-TERMINATING ".to_owned()).unwrap();
    harness.send(Request::Rescan);
    let err = harness.done().unwrap_err();

    assert!(matches!(err, WifiError::Wpa(WpaError::NoSocket(_))), "{err:?}");
}
