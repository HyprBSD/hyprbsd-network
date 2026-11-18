//! wpa::Socket and wpa::Events against a fake wpa_supplicant control socket.

mod fake;

use fake::Scratch;
use hyprbsd_network::wpa::{self, Control, Socket, WpaError};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixDatagram;
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// A fake wpa_supplicant socket at `path`: each request gets `answer`'s replies; None closes the socket.
fn serve(path: &Path, answer: impl Fn(&str) -> Option<Vec<String>> + Send + 'static) {
    let sock = UnixDatagram::bind(path).unwrap();
    thread::spawn(move || {
        let mut buf = [0; 4096];
        while let Ok((n, from)) = sock.recv_from(&mut buf) {
            let Some(replies) = answer(&String::from_utf8_lossy(&buf[..n])) else { return };
            let Some(to) = from.as_pathname() else { continue };
            for reply in replies {
                let _ = sock.send_to(reply.as_bytes(), to);
            }
        }
    });
}

fn connect(scratch: &Scratch) -> Socket {
    Socket::connect(&scratch.path("wlan0"), &scratch.0).unwrap()
}

#[test]
fn request_returns_the_reply() {
    let scratch = Scratch::new("socket");
    serve(&scratch.path("wlan0"), |cmd| Some(vec![if cmd == "PING" { "PONG\n".into() } else { "FAIL\n".into() }]));

    let reply = connect(&scratch).request("PING").unwrap();

    assert_eq!(reply, "PONG\n");
}

#[test]
fn request_skips_events_before_the_reply() {
    let scratch = Scratch::new("socket");
    serve(&scratch.path("wlan0"), |_| Some(vec!["<3>CTRL-EVENT-SCAN-RESULTS ".into(), "OK\n".into()]));

    let reply = connect(&scratch).request("SCAN").unwrap();

    assert_eq!(reply, "OK\n");
}

#[test]
fn request_times_out_without_a_reply() {
    let scratch = Scratch::new("socket");
    serve(&scratch.path("wlan0"), |_| Some(vec![]));
    let mut socket = connect(&scratch);
    socket.set_timeout(Duration::from_millis(50));

    let err = socket.request("STATUS").unwrap_err();

    assert!(matches!(err, WpaError::Timeout(ref c) if c == "STATUS"), "{err:?}");
}

#[test]
fn a_reply_after_a_timeout_doesnt_answer_the_next_request() {
    let scratch = Scratch::new("socket");
    let server = UnixDatagram::bind(scratch.path("wlan0")).unwrap();
    let (release, released) = mpsc::channel::<()>();
    thread::spawn(move || {
        let mut buf = [0; 256];
        let Ok((_, first)) = server.recv_from(&mut buf) else { return };
        let _ = released.recv();
        if let Some(to) = first.as_pathname() {
            let _ = server.send_to(b"OK\n", to);
        }
        let Ok((_, second)) = server.recv_from(&mut buf) else { return };
        if let Some(to) = second.as_pathname() {
            let _ = server.send_to(b"FAIL\n", to);
        }
    });
    let mut socket = connect(&scratch);
    socket.set_timeout(Duration::from_millis(50));
    let _ = socket.ok("SET_NETWORK 1 ssid 486f6d65");
    release.send(()).unwrap();
    thread::sleep(Duration::from_millis(50));

    let removed = socket.ok("REMOVE_NETWORK 1");

    assert!(matches!(removed, Err(WpaError::Refused { .. })), "{removed:?}");
}

#[test]
fn a_socket_that_timed_out_and_cant_be_renewed_answers_nothing_more() {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipped: root can bind in any directory");
        return;
    }
    let scratch = Scratch::new("socket");
    let server = UnixDatagram::bind(scratch.path("wlan0")).unwrap();
    server.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    let (release, released) = mpsc::channel::<()>();
    thread::spawn(move || {
        let mut buf = [0; 256];
        let Ok((_, first)) = server.recv_from(&mut buf) else { return };
        let _ = released.recv();
        if let Some(to) = first.as_pathname() {
            let _ = server.send_to(b"OK\n", to);
        }
        if let Ok((_, second)) = server.recv_from(&mut buf)
            && let Some(to) = second.as_pathname()
        {
            let _ = server.send_to(b"FAIL\n", to);
        }
    });
    let mut socket = connect(&scratch);
    socket.set_timeout(Duration::from_millis(50));
    fs::set_permissions(&scratch.0, fs::Permissions::from_mode(0o500)).unwrap();
    let _ = socket.ok("SET_NETWORK 1 ssid 486f6d65");
    release.send(()).unwrap();
    thread::sleep(Duration::from_millis(50));

    let removed = socket.ok("REMOVE_NETWORK 1");

    assert!(removed.is_err(), "{removed:?}");
}

#[test]
fn connect_without_a_socket_says_wifi_isnt_set_up() {
    let scratch = Scratch::new("socket");

    let err = Socket::connect(&scratch.path("wlan0"), &scratch.0).unwrap_err();

    assert!(matches!(err, WpaError::NoSocket(_)), "{err:?}");
}

#[test]
fn connect_to_a_socket_nobody_listens_on_says_not_running() {
    let scratch = Scratch::new("socket");
    drop(UnixDatagram::bind(scratch.path("wlan0")).unwrap());

    let err = Socket::connect(&scratch.path("wlan0"), &scratch.0).unwrap_err();

    assert!(matches!(err, WpaError::NotRunning(_)), "{err:?}");
}

#[test]
fn connect_to_a_socket_one_cant_reach_says_wheel() {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipped: root can open any socket");
        return;
    }
    let scratch = Scratch::new("socket");
    let ctrl = scratch.path("ctrl");
    fs::create_dir(&ctrl).unwrap();
    serve(&ctrl.join("wlan0"), |_| Some(vec!["PONG\n".into()]));
    fs::set_permissions(&ctrl, fs::Permissions::from_mode(0o000)).unwrap();

    let err = Socket::connect(&ctrl.join("wlan0"), &scratch.0).unwrap_err();

    assert!(matches!(err, WpaError::Denied(_)), "{err:?}");
}

#[test]
fn ok_names_only_the_verb_when_refused() {
    let scratch = Scratch::new("socket");
    serve(&scratch.path("wlan0"), |_| Some(vec!["FAIL\n".into()]));

    let err = connect(&scratch).ok("SET_NETWORK 0 psk 0123456789abcdef").unwrap_err();

    assert_eq!(err.to_string(), "wpa_supplicant refused SET_NETWORK: FAIL");
}

#[test]
fn ok_rejects_a_reply_that_isnt_ok_or_fail() {
    let scratch = Scratch::new("socket");
    serve(&scratch.path("wlan0"), |_| Some(vec!["UNKNOWN COMMAND\n".into()]));

    let err = connect(&scratch).ok("ENABLE_NETWORK 1").unwrap_err();

    assert!(matches!(err, WpaError::BadReply { ref command, .. } if command == "ENABLE_NETWORK"), "{err:?}");
}

#[test]
fn dropping_the_socket_removes_its_file() {
    let scratch = Scratch::new("socket");
    serve(&scratch.path("wlan0"), |_| Some(vec![]));
    let socket = connect(&scratch);

    drop(socket);

    let left: Vec<_> = fs::read_dir(&scratch.0).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(left, ["wlan0"]);
}

#[test]
fn events_arrive_without_their_level() {
    let scratch = Scratch::new("socket");
    serve(&scratch.path("wlan0"), |_| {
        Some(vec![
            "OK\n".into(),
            "PONG\n".into(),
            "<3>CTRL-EVENT-CONNECTED - Connection to 02:00:00:00:00:01 completed".into(),
        ])
    });
    let mut events = connect(&scratch).attach().unwrap();

    let event = events.wait(Duration::from_secs(5)).unwrap();

    assert_eq!(event.as_deref(), Some("CTRL-EVENT-CONNECTED - Connection to 02:00:00:00:00:01 completed"));
}

#[test]
fn interfaces_lists_the_sockets_sorted() {
    let scratch = Scratch::new("socket");
    let _wlan1 = UnixDatagram::bind(scratch.path("wlan1")).unwrap();
    let _wlan0 = UnixDatagram::bind(scratch.path("wlan0")).unwrap();
    fs::write(scratch.path("notes"), "").unwrap();

    let found = wpa::interfaces(&scratch.0).unwrap();

    assert_eq!(found, ["wlan0", "wlan1"]);
}

#[test]
fn interfaces_is_empty_without_the_directory() {
    let scratch = Scratch::new("socket");

    let found = wpa::interfaces(&scratch.path("missing")).unwrap();

    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn events_wait_for_finds_the_named_event_among_others() {
    let scratch = Scratch::new("socket");
    serve(&scratch.path("wlan0"), |_| {
        Some(vec!["OK\n".into(), "<3>CTRL-EVENT-SCAN-STARTED ".into(), "<3>CTRL-EVENT-SCAN-RESULTS ".into()])
    });
    let mut events = connect(&scratch).attach().unwrap();

    let found = events.wait_for("CTRL-EVENT-SCAN-RESULTS", Duration::from_secs(5)).unwrap();

    assert!(found);
}

#[test]
fn events_wait_for_gives_false_when_the_event_doesnt_come() {
    let scratch = Scratch::new("socket");
    serve(&scratch.path("wlan0"), |_| Some(vec!["OK\n".into(), "<3>CTRL-EVENT-SCAN-STARTED ".into()]));
    let mut events = connect(&scratch).attach().unwrap();

    let found = events.wait_for("CTRL-EVENT-SCAN-RESULTS", Duration::from_millis(100)).unwrap();

    assert!(!found);
}

/// A wpa_supplicant socket that, given a request, removes its file and closes without answering.
fn ends_on_a_request(path: &Path) {
    let sock = UnixDatagram::bind(path).unwrap();
    let file = path.to_owned();
    thread::spawn(move || {
        let mut buf = [0; 256];
        let _ = sock.recv_from(&mut buf);
        let _ = fs::remove_file(&file);
    });
}

#[test]
fn a_request_ends_soon_after_wpa_supplicant_goes_without_answering() {
    let scratch = Scratch::new("socket");
    ends_on_a_request(&scratch.path("wlan0"));
    let mut socket = connect(&scratch);
    let started = Instant::now();

    let err = socket.request("STATUS").unwrap_err();

    let waited = started.elapsed();
    assert!(matches!(err, WpaError::NotRunning(_)) && waited < Duration::from_secs(2), "{err:?} after {waited:?}");
}
