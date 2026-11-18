//! One window at a time.

mod fake;

use fake::Scratch;
use hyprbsd_network::instance::{self, Instance};
use std::os::unix::net::UnixListener;
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn the_first_start_is_the_window() {
    let scratch = Scratch::new("instance");

    let claimed = instance::claim(&scratch.path("window")).unwrap();

    assert!(matches!(claimed, Instance::First(_)));
}

#[test]
fn a_second_start_finds_it_running() {
    let scratch = Scratch::new("instance");
    let _first = instance::claim(&scratch.path("window")).unwrap();

    let second = instance::claim(&scratch.path("window")).unwrap();

    assert!(matches!(second, Instance::Running));
}

#[test]
fn a_second_start_asks_the_window_to_show_itself() {
    let scratch = Scratch::new("instance");
    let Instance::First(window) = instance::claim(&scratch.path("window")).unwrap() else { panic!("not first") };
    let (knocked, knocks) = mpsc::channel();
    window
        .on_knock(move || {
            let _ = knocked.send(());
        })
        .unwrap();

    instance::claim(&scratch.path("window")).unwrap();

    assert_eq!(knocks.recv_timeout(Duration::from_secs(5)), Ok(()));
}

#[test]
fn a_window_closed_lets_the_next_start_be_the_window() {
    let scratch = Scratch::new("instance");
    drop(instance::claim(&scratch.path("window")).unwrap());

    let next = instance::claim(&scratch.path("window")).unwrap();

    assert!(matches!(next, Instance::First(_)));
}

#[test]
fn a_window_listening_for_knocks_removes_its_socket_when_it_closes() {
    let scratch = Scratch::new("instance");
    let Instance::First(window) = instance::claim(&scratch.path("window")).unwrap() else { panic!("not first") };
    window.on_knock(|| {}).unwrap();

    drop(window);

    assert!(!scratch.path("window").exists());
}

#[test]
fn a_socket_left_behind_is_taken_over() {
    let scratch = Scratch::new("instance");
    drop(UnixListener::bind(scratch.path("window")).unwrap());

    let next = instance::claim(&scratch.path("window")).unwrap();

    assert!(matches!(next, Instance::First(_)));
}
