//! The rows of the window's Wi-Fi list.

use hyprbsd_network::view::{self, Row};
use hyprbsd_network::wifi::{Network, Saved, Security, Ssid, Status};

fn ssid(name: &[u8]) -> Ssid {
    Ssid::new(name).unwrap()
}

fn air(name: &[u8], security: Security, signal: i32) -> Network {
    Network { ssid: ssid(name), security, signal }
}

fn saved(id: u32, name: &[u8], disabled: bool) -> Saved {
    Saved { id, ssid: ssid(name), current: false, disabled }
}

fn idle() -> Status {
    Status { state: "DISCONNECTED".into(), ssid: None, id: None, address: None }
}

fn joined(name: &[u8], id: u32) -> Status {
    Status { state: "COMPLETED".into(), ssid: Some(ssid(name)), id: Some(id), address: None }
}

fn names(rows: &[Row]) -> Vec<&[u8]> {
    rows.iter().map(|r| r.ssid.as_bytes()).collect()
}

mod rows {
    use super::*;

    #[test]
    fn lists_networks_in_range_strongest_first_with_no_saved_ids() {
        let networks = [air(b"Home", Security::Wpa2, -40), air(b"Cafe", Security::Open, -60)];

        let rows = view::rows(&networks, &[], &idle());

        assert_eq!(
            rows.iter().map(|r| (r.ssid.as_bytes(), r.saved.as_slice())).collect::<Vec<_>>(),
            [(b"Home".as_slice(), &[][..]), (b"Cafe".as_slice(), &[][..])]
        );
    }

    #[test]
    fn puts_the_connected_network_first_and_marks_only_it_connected() {
        let networks = [air(b"Cafe", Security::Open, -40), air(b"Home", Security::Wpa2, -70)];

        let rows = view::rows(&networks, &[saved(0, b"Home", false)], &joined(b"Home", 0));

        assert_eq!(
            rows.iter().map(|r| (r.ssid.as_bytes(), r.connected)).collect::<Vec<_>>(),
            [(b"Home".as_slice(), true), (b"Cafe".as_slice(), false)]
        );
    }

    #[test]
    fn isnt_connected_mid_handshake() {
        let status = Status { state: "4WAY_HANDSHAKE".into(), ..joined(b"Home", 0) };

        let rows = view::rows(&[air(b"Home", Security::Wpa2, -50)], &[saved(0, b"Home", false)], &status);

        assert!(!rows[0].connected);
    }

    #[test]
    fn has_one_row_for_a_name_both_open_and_secured_and_it_is_secured() {
        let networks = [air(b"Home", Security::Open, -40), air(b"Home", Security::Wpa2, -60)];

        let rows = view::rows(&networks, &[], &idle());

        assert_eq!(
            rows.iter().map(|r| (r.ssid.as_bytes(), r.security)).collect::<Vec<_>>(),
            [(b"Home".as_slice(), Some(Security::Wpa2))]
        );
    }

    #[test]
    fn gives_a_name_its_strongest_signal() {
        let networks = [air(b"Home", Security::Open, -40), air(b"Home", Security::Wpa2, -60)];

        let rows = view::rows(&networks, &[], &idle());

        assert_eq!(rows[0].signal, Some(-40));
    }

    #[test]
    fn lists_saved_networks_out_of_range_last() {
        let rows = view::rows(&[air(b"Cafe", Security::Open, -60)], &[saved(0, b"Away", false)], &idle());

        assert_eq!(names(&rows), [b"Cafe".as_slice(), b"Away"]);
    }

    #[test]
    fn gives_a_saved_network_out_of_range_no_signal_or_security() {
        let rows = view::rows(&[], &[saved(0, b"Away", false)], &idle());

        assert_eq!((rows[0].signal, rows[0].security), (None, None));
    }

    #[test]
    fn lists_saved_networks_out_of_range_by_name() {
        let rows = view::rows(&[], &[saved(0, b"Work", false), saved(1, b"Away", false)], &idle());

        assert_eq!(names(&rows), [b"Away".as_slice(), b"Work"]);
    }

    #[test]
    fn has_one_row_for_a_name_saved_twice() {
        let rows = view::rows(&[], &[saved(0, b"Home", false), saved(3, b"Home", true)], &idle());

        assert_eq!(rows.iter().map(|r| r.saved.clone()).collect::<Vec<_>>(), [vec![0, 3]]);
    }

    #[test]
    fn joins_a_name_automatically_if_any_of_its_records_is() {
        let rows = view::rows(&[], &[saved(0, b"Home", true), saved(1, b"Home", false)], &idle());

        assert!(rows[0].autoconnect);
    }

    #[test]
    fn doesnt_join_a_name_automatically_if_its_records_are_all_off() {
        let rows = view::rows(&[], &[saved(0, b"Home", true)], &idle());

        assert!(!rows[0].autoconnect);
    }

    #[test]
    fn lists_a_hidden_network_joined_first() {
        let rows =
            view::rows(&[air(b"Cafe", Security::Open, -40)], &[saved(0, b"Secret", false)], &joined(b"Secret", 0));

        assert_eq!(names(&rows), [b"Secret".as_slice(), b"Cafe"]);
    }
}

fn row(name: &[u8], security: Option<Security>, saved: &[u32]) -> Row {
    Row {
        ssid: ssid(name),
        signal: security.map(|_| -50),
        security,
        connected: false,
        saved: saved.to_vec(),
        autoconnect: !saved.is_empty(),
    }
}

mod needs_password {
    use super::*;

    #[test]
    fn a_new_wpa2_network_does() {
        assert!(view::needs_password(&row(b"Home", Some(Security::Wpa2), &[]), false));
    }

    #[test]
    fn an_open_network_doesnt() {
        assert!(!view::needs_password(&row(b"Cafe", Some(Security::Open), &[]), false));
    }

    #[test]
    fn a_saved_wpa2_network_doesnt() {
        assert!(!view::needs_password(&row(b"Home", Some(Security::Wpa2), &[0]), false));
    }

    #[test]
    fn a_saved_wpa2_network_does_once_its_password_is_to_be_typed_again() {
        assert!(view::needs_password(&row(b"Home", Some(Security::Wpa2), &[0]), true));
    }
}

mod connectable {
    use super::*;

    #[test]
    fn a_saved_network_out_of_range_is() {
        assert!(view::connectable(&row(b"Secret", None, &[0])));
    }

    #[test]
    fn a_wpa3_only_network_isnt() {
        assert!(!view::connectable(&row(b"Office", Some(Security::Wpa3), &[])));
    }

    #[test]
    fn the_network_joined_isnt() {
        let joined = Row { connected: true, ..row(b"Home", Some(Security::Wpa2), &[0]) };

        assert!(!view::connectable(&joined));
    }
}

mod connect {
    use super::*;
    use hyprbsd_network::wifi::WifiError;
    use hyprbsd_network::worker::Request;

    #[test]
    fn joins_a_new_wpa2_network_with_the_password() {
        let request = view::connect(&row(b"Home", Some(Security::Wpa2), &[]), "correct horse", false).unwrap();

        assert_eq!(
            request,
            Request::Join { ssid: ssid(b"Home"), password: Some(b"correct horse".to_vec()), hidden: false }
        );
    }

    #[test]
    fn refuses_a_short_password_before_asking_anything() {
        let err = view::connect(&row(b"Home", Some(Security::Wpa2), &[]), "short", false).unwrap_err();

        assert!(matches!(err, WifiError::Passphrase), "{err:?}");
    }

    #[test]
    fn joins_a_new_open_network_without_a_password() {
        let request = view::connect(&row(b"Cafe", Some(Security::Open), &[]), "", false).unwrap();

        assert_eq!(request, Request::Join { ssid: ssid(b"Cafe"), password: None, hidden: false });
    }

    #[test]
    fn joins_a_saved_network_as_saved_through_its_newest_record() {
        let request = view::connect(&row(b"Home", Some(Security::Wpa2), &[0, 4]), "", false).unwrap();

        assert_eq!(request, Request::Connect { id: 4 });
    }

    #[test]
    fn joins_a_saved_network_with_its_password_typed_again() {
        let request = view::connect(&row(b"Home", Some(Security::Wpa2), &[0]), "new horse 2", true).unwrap();

        assert_eq!(
            request,
            Request::Join { ssid: ssid(b"Home"), password: Some(b"new horse 2".to_vec()), hidden: false }
        );
    }

    #[test]
    fn joins_a_saved_network_out_of_range_with_a_new_password_as_hidden() {
        let request = view::connect(&row(b"Secret", None, &[0]), "new horse 2", true).unwrap();

        assert_eq!(
            request,
            Request::Join { ssid: ssid(b"Secret"), password: Some(b"new horse 2".to_vec()), hidden: true }
        );
    }

    #[test]
    fn joins_a_saved_network_out_of_range_as_saved_without_a_new_password() {
        let request = view::connect(&row(b"Secret", None, &[0]), "", true).unwrap();

        assert_eq!(request, Request::Connect { id: 0 });
    }
}

mod password_hint {
    use super::*;

    #[test]
    fn a_new_wpa2_network_asks_for_its_password() {
        assert_eq!(view::password_hint(&row(b"Home", Some(Security::Wpa2), &[]), false), Some("Password"));
    }

    #[test]
    fn a_saved_network_out_of_range_asks_for_none_at_first() {
        assert_eq!(view::password_hint(&row(b"Secret", None, &[0]), false), None);
    }

    #[test]
    fn a_saved_network_out_of_range_asks_for_a_new_one_once_it_failed() {
        assert_eq!(view::password_hint(&row(b"Secret", None, &[0]), true), Some("New password, if it changed"));
    }
}

mod scanned {
    use super::*;

    #[test]
    fn under_a_minute_is_just_now() {
        assert_eq!(view::scanned(59), "Last scanned just now.");
    }

    #[test]
    fn a_minute() {
        assert_eq!(view::scanned(60), "Last scanned a minute ago.");
    }

    #[test]
    fn minutes() {
        assert_eq!(view::scanned(720), "Last scanned 12 minutes ago.");
    }

    #[test]
    fn an_hour() {
        assert_eq!(view::scanned(3600), "Last scanned an hour ago.");
    }

    #[test]
    fn hours() {
        assert_eq!(view::scanned(7300), "Last scanned 2 hours ago.");
    }

    #[test]
    fn hours_up_to_two_days() {
        assert_eq!(view::scanned(47 * 3600 + 3599), "Last scanned 47 hours ago.");
    }

    #[test]
    fn days_from_two_days() {
        assert_eq!(view::scanned(74 * 3600), "Last scanned 3 days ago.");
    }
}

mod connect_hidden {
    use super::*;
    use hyprbsd_network::wifi::WifiError;
    use hyprbsd_network::worker::Request;

    #[test]
    fn joins_it_by_name_with_the_password() {
        let request = view::connect_hidden("Secret", "correct horse").unwrap();

        assert_eq!(
            request,
            Request::Join { ssid: ssid(b"Secret"), password: Some(b"correct horse".to_vec()), hidden: true }
        );
    }

    #[test]
    fn joins_it_open_without_a_password() {
        let request = view::connect_hidden("Secret", "").unwrap();

        assert_eq!(request, Request::Join { ssid: ssid(b"Secret"), password: None, hidden: true });
    }

    #[test]
    fn refuses_an_empty_name() {
        assert!(matches!(view::connect_hidden("", "correct horse"), Err(WifiError::Name)));
    }

    #[test]
    fn refuses_a_name_of_33_bytes() {
        assert!(matches!(view::connect_hidden(&"n".repeat(33), ""), Err(WifiError::Name)));
    }

    #[test]
    fn refuses_a_short_password() {
        assert!(matches!(view::connect_hidden("Secret", "short"), Err(WifiError::Passphrase)));
    }
}

mod sentence {
    use super::*;
    use hyprbsd_network::wifi::WifiError;

    #[test]
    fn starts_with_a_capital_and_ends_with_a_full_stop() {
        let text = view::sentence(&WifiError::WrongPassword("Home".into()));

        assert_eq!(text, "Couldn't join \"Home\": the password is wrong.");
    }
}

mod caption {
    use super::*;

    #[test]
    fn says_connected_with_the_address() {
        let joined = Row { connected: true, ..row(b"Home", Some(Security::Wpa2), &[0]) };

        assert_eq!(view::caption(&joined, Some("192.168.1.20")), "Connected · 192.168.1.20");
    }

    #[test]
    fn says_connected_before_dhcp_gives_an_address() {
        let joined = Row { connected: true, ..row(b"Home", Some(Security::Wpa2), &[0]) };

        assert_eq!(view::caption(&joined, None), "Connected");
    }

    #[test]
    fn says_a_saved_network_is_out_of_range() {
        assert_eq!(view::caption(&row(b"Away", None, &[0]), None), "Saved · not in range");
    }

    #[test]
    fn says_why_a_network_cant_be_joined() {
        assert_eq!(
            view::caption(&row(b"New", Some(Security::Wpa3), &[]), None),
            "FreeBSD's Wi-Fi can't join WPA3-only networks yet"
        );
    }

    #[test]
    fn gives_the_security_of_a_saved_network() {
        assert_eq!(view::caption(&row(b"Home", Some(Security::Wpa2), &[0]), None), "WPA2 · saved");
    }

    #[test]
    fn says_an_open_network_is_open() {
        assert_eq!(view::caption(&row(b"Cafe", Some(Security::Open), &[]), None), "Open");
    }
}
