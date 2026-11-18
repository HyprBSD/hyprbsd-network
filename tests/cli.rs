//! The hyprbsd-network command against a model of wpa_supplicant behind a control socket.

mod fake;

use fake::{Air, Fake, Scratch};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// A control directory with the model on wlan0.
struct Wpa {
    dir: Scratch,
    fake: Arc<Mutex<Fake>>,
}

impl Wpa {
    fn new(fake: Fake) -> Self {
        let dir = Scratch::new("cli");
        fs::create_dir(dir.path("run")).unwrap();
        let fake = fake::serve(&dir.path("wlan0"), fake);
        Wpa { dir, fake }
    }

    /// hyprbsd-network with `args`, `input` on standard input.
    fn run(&self, args: &[&str], input: &str) -> Output {
        run_in(&self.dir.0, args, input)
    }

    fn log(&self) -> Vec<String> {
        self.fake.lock().unwrap().log.clone()
    }
}

fn run_in(dir: &PathBuf, args: &[&str], input: &str) -> Output {
    spawn_in(dir, args, input).wait_with_output().unwrap()
}

/// hyprbsd-network started with `args`, `input` on standard input.
fn spawn_in(dir: &PathBuf, args: &[&str], input: &str) -> Child {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hyprbsd-network"))
        .args(args)
        .env("HYPRBSD_NETWORK_CTRL_DIR", dir)
        .env("XDG_RUNTIME_DIR", dir.join("run"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    child
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn psk_hex(passphrase: &str, name: &str) -> String {
    fake::hex(&hyprbsd_network::wifi::psk(
        passphrase.as_bytes(),
        &hyprbsd_network::wifi::Ssid::new(name.as_bytes()).unwrap(),
    ))
}

fn home() -> Fake {
    Fake::new()
        .with_air(Air::wpa2(b"Home", -50, &psk_hex("correct horse", "Home")))
        .with_air(Air::open(b"Cafe", -80))
        .with_air(Air { flags: "[WPA2-SAE-CCMP][ESS]".into(), ..Air::wpa2(b"Neighbour", -60, "x") })
}

mod list {
    use super::*;

    #[test]
    fn prints_the_networks_strongest_first() {
        let wpa = Wpa::new(home().with_record(b"Cafe", "", false));

        let out = wpa.run(&["list"], "");

        assert_eq!(
            stdout(&out),
            "strong  WPA2                 Home\ngood    WPA3 only            Neighbour\nfair    open                 Cafe (saved)\n",
            "{}",
            stderr(&out)
        );
    }

    #[test]
    fn scans_when_nothing_is_joined() {
        let wpa = Wpa::new(home());

        wpa.run(&["list"], "");

        assert!(wpa.log().contains(&"SCAN".to_owned()), "{:?}", wpa.log());
    }

    /// The model joined to Home.
    fn joined() -> Fake {
        let mut fake = home().with_record(b"Home", &psk_hex("correct horse", "Home"), false);
        fake.current = Some(0);
        fake
    }

    #[test]
    fn doesnt_scan_while_joined() {
        let wpa = Wpa::new(joined());

        let out = wpa.run(&["list"], "");

        assert!(
            !wpa.log().contains(&"SCAN".to_owned()) && stdout(&out).contains("Home (connected)"),
            "{:?} {}",
            wpa.log(),
            stdout(&out)
        );
    }

    #[test]
    fn says_while_joined_that_the_list_is_the_last_scans() {
        let wpa = Wpa::new(joined());

        let out = wpa.run(&["list"], "");

        assert!(stderr(&out).contains("list --scan"), "{}", stderr(&out));
    }

    #[test]
    fn scan_scans_while_joined() {
        let wpa = Wpa::new(joined());

        wpa.run(&["list", "--scan"], "");

        assert!(wpa.log().contains(&"SCAN".to_owned()), "{:?}", wpa.log());
    }
}

mod join {
    use super::*;

    #[test]
    fn joins_with_the_password_on_standard_input() {
        let wpa = Wpa::new(home());

        let out = wpa.run(&["join", "Home"], "correct horse\n");

        assert_eq!(stdout(&out), "Joined Home: 192.168.1.20\n", "{}", stderr(&out));
        let file = wpa.fake.lock().unwrap().file.clone().unwrap();
        assert_eq!(file[0].ssid, b"Home");
    }

    #[test]
    fn never_joins_an_open_access_point_of_the_name_with_a_password_given() {
        let key = psk_hex("correct horse", "Home");
        let wpa = Wpa::new(
            Fake::new()
                .with_air(Air::open(b"Home", -30))
                .with_air(Air::wpa2(b"Home", -70, &key))
                .with_record(b"Home", &key, false),
        );

        wpa.run(&["join", "Home"], "correct horse\n");

        let file = wpa.fake.lock().unwrap().file.clone().unwrap();
        assert_eq!(file.iter().map(|r| r.key_mgmt.as_str()).collect::<Vec<_>>(), ["WPA-PSK"]);
    }

    #[test]
    fn refuses_a_password_for_a_network_open_here() {
        let key = psk_hex("correct horse", "Home");
        let wpa = Wpa::new(Fake::new().with_air(Air::open(b"Home", -30)).with_record(b"Home", &key, false));

        let out = wpa.run(&["join", "Home"], "correct horse\n");

        let file = wpa.fake.lock().unwrap().file.clone().unwrap();
        assert_eq!(
            (stderr(&out), file.iter().map(|r| r.key_mgmt.clone()).collect::<Vec<_>>()),
            (
                "hyprbsd-network: \"Home\" is open here, yet a password was given: it may not be the network you \
                 saved (leave the password out to join it)\n"
                    .to_owned(),
                vec!["WPA-PSK".to_owned()]
            )
        );
    }

    #[test]
    fn interrupted_puts_the_other_networks_back() {
        let mut fake = home().with_record(b"Work", "", false);
        fake.reports_wrong_key = false;
        let wpa = Wpa::new(fake);
        let child = spawn_in(&wpa.dir.0, &["join", "Home"], "wrong horse\n");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !wpa.log().iter().any(|c| c.starts_with("SELECT_NETWORK")) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }

        // SAFETY: kill has no preconditions; the pid is the child's, not yet waited for.
        unsafe { libc::kill(libc::pid_t::try_from(child.id()).unwrap(), libc::SIGINT) };
        let out = child.wait_with_output().unwrap();

        let fake = wpa.fake.lock().unwrap();
        let enabled: Vec<(Vec<u8>, bool)> = fake.records.iter().map(|r| (r.ssid.clone(), !r.disabled)).collect();
        assert_eq!(
            (enabled, stderr(&out)),
            (vec![(b"Work".to_vec(), true)], "hyprbsd-network: stopped; \"Home\" isn't joined\n".to_owned())
        );
    }

    #[test]
    fn says_when_the_password_is_wrong() {
        let wpa = Wpa::new(home());

        let out = wpa.run(&["join", "Home"], "wrong horse\n");

        assert_eq!(stderr(&out), "hyprbsd-network: couldn't join \"Home\": the password is wrong\n");
    }

    #[test]
    fn joins_an_open_network_without_a_password() {
        let wpa = Wpa::new(home());

        let out = wpa.run(&["join", "Cafe"], "");

        assert!(out.status.success(), "{}", stderr(&out));
    }

    #[test]
    fn joins_a_saved_network_without_adding_another() {
        let wpa = Wpa::new(home().with_record(b"Home", &psk_hex("correct horse", "Home"), false));

        let out = wpa.run(&["join", "Home"], "");

        assert!(out.status.success() && !wpa.log().contains(&"ADD_NETWORK".to_owned()), "{}", stderr(&out));
    }

    #[test]
    fn joins_a_hidden_network() {
        let psk = psk_hex("correct horse", "Secret");
        let wpa = Wpa::new(Fake::new().with_air(Air { hidden: true, ..Air::wpa2(b"Secret", -50, &psk) }));

        let out = wpa.run(&["join", "--hidden", "Secret"], "correct horse\n");

        assert!(out.status.success(), "{}", stderr(&out));
    }

    #[test]
    fn refuses_a_wpa3_only_network_without_adding_it() {
        let wpa = Wpa::new(home());

        let out = wpa.run(&["join", "Neighbour"], "a password\n");

        assert_eq!(
            (stderr(&out), wpa.log().contains(&"ADD_NETWORK".to_owned())),
            ("hyprbsd-network: \"Neighbour\": FreeBSD's Wi-Fi can't join WPA3-only networks yet\n".to_owned(), false)
        );
    }

    #[test]
    fn says_when_a_network_isnt_in_range() {
        let wpa = Wpa::new(home());

        let out = wpa.run(&["join", "Elsewhere"], "a password\n");

        assert_eq!(
            stderr(&out),
            "hyprbsd-network: \"Elsewhere\" isn't in range (a hidden network: hyprbsd-network join --hidden)\n"
        );
    }

    #[test]
    fn refuses_wpa2_without_a_password() {
        let wpa = Wpa::new(home());

        let out = wpa.run(&["join", "Home"], "");

        assert_eq!(
            stderr(&out),
            "hyprbsd-network: \"Home\" needs a password: join it from the window, or pipe the password to standard input\n"
        );
    }

    #[test]
    fn refuses_a_short_password_in_words() {
        let wpa = Wpa::new(home());

        let out = wpa.run(&["join", "Home"], "short\n");

        assert_eq!(stderr(&out), "hyprbsd-network: a Wi-Fi password has 8 to 63 bytes\n");
    }
}

mod forget {
    use super::*;

    #[test]
    fn forgets_a_saved_network() {
        let wpa = Wpa::new(home().with_record(b"Home", "", false));

        let out = wpa.run(&["forget", "Home"], "");

        assert_eq!(stdout(&out), "Forgot Home\n", "{}", stderr(&out));
    }
}

#[test]
fn without_wpa_supplicant_says_wifi_isnt_set_up() {
    let dir = PathBuf::from(format!("/tmp/hn-cli-{}-none", std::process::id()));
    fs::create_dir_all(dir.join("run")).unwrap();

    let out = run_in(&dir, &["list"], "");

    let _ = fs::remove_dir_all(&dir);
    assert!(stderr(&out).starts_with("hyprbsd-network: Wi-Fi isn't set up"), "{}", stderr(&out));
}

#[test]
fn an_unknown_command_prints_usage() {
    let wpa = Wpa::new(Fake::new());

    let out = wpa.run(&["frobnicate"], "");

    assert_eq!((out.status.code(), stderr(&out).starts_with("usage: hyprbsd-network\n")), (Some(2), true));
}

#[test]
fn without_a_command_tries_the_window() {
    let wpa = Wpa::new(Fake::new());

    let out = Command::new(env!("CARGO_BIN_EXE_hyprbsd-network"))
        .env("HYPRBSD_NETWORK_CTRL_DIR", &wpa.dir.0)
        .env("XDG_RUNTIME_DIR", wpa.dir.path("run"))
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .output()
        .unwrap();

    assert!(stderr(&out).starts_with("hyprbsd-network: couldn't open the window"), "{}", stderr(&out));
}
