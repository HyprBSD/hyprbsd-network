//! Scanning, saved networks, joining and forgetting, against a model of wpa_supplicant.

mod fake;

use fake::{Air, Fake, NoEvents};
use hyprbsd_network::wifi::{self, Joined, Key, Network, Security, Ssid, WifiError};
use hyprbsd_network::wpa::{Control, WpaError};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const QUICK: wifi::Wait = wifi::Wait {
    handshake: Duration::from_millis(200),
    address: Duration::from_millis(200),
    poll: Duration::from_millis(1),
    rescan: Duration::from_secs(30),
    stop: None,
};

fn ssid(name: &[u8]) -> Ssid {
    Ssid::new(name).unwrap()
}

/// Joins through the model, with its events.
fn join(fake: &mut Fake, name: &[u8], key: Key, hidden: bool) -> Result<Joined, WifiError> {
    let mut events = fake.events();
    wifi::join(fake, &mut events, &ssid(name), key, hidden, &QUICK)
}

/// Joins a saved network through the model, with its events.
fn connect(fake: &mut Fake, id: u32) -> Result<Joined, WifiError> {
    let mut events = fake.events();
    wifi::connect(fake, &mut events, id, &QUICK)
}

/// The pre-shared key for a passphrase and name, in hex.
fn psk_hex(passphrase: &str, name: &[u8]) -> String {
    fake::hex(&wifi::psk(passphrase.as_bytes(), &ssid(name)))
}

/// A wpa_supplicant that answers each request with what `.0` gives for it.
struct Answers(fn(&str) -> &'static str);

impl Control for Answers {
    fn request(&mut self, command: &str) -> Result<String, WpaError> {
        Ok((self.0)(command).to_owned())
    }
}

/// Each record's name and whether it's joined automatically.
fn enabled(records: &[fake::Record]) -> Vec<(Vec<u8>, bool)> {
    records.iter().map(|r| (r.ssid.clone(), !r.disabled)).collect()
}

mod networks {
    use super::*;

    #[test]
    fn lists_each_name_once_strongest_first() {
        let mut fake = Fake::new()
            .with_air(Air::wpa2(b"Home", -70, "k"))
            .with_air(Air::open(b"Cafe", -50))
            .with_air(Air::wpa2(b"Home", -40, "k"));

        let found = wifi::networks(&mut fake).unwrap();

        assert_eq!(
            found,
            [
                Network { ssid: ssid(b"Home"), security: Security::Wpa2, signal: -40 },
                Network { ssid: ssid(b"Cafe"), security: Security::Open, signal: -50 },
            ]
        );
    }

    #[test]
    fn leaves_out_hidden_networks() {
        let mut fake = Fake::new().with_air(Air { hidden: true, ..Air::wpa2(b"Secret", -40, "k") });

        let found = wifi::networks(&mut fake).unwrap();

        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn reads_every_network_of_a_busy_scan() {
        let mut fake = Fake::new();
        for n in 0..120 {
            fake = fake.with_air(Air::wpa2(format!("A network with a long name {n:03}").as_bytes(), -60, "k"));
        }

        let found = wifi::networks(&mut fake).unwrap();

        assert_eq!(found.len(), 120);
    }

    #[test]
    fn leaves_out_a_name_of_zero_bytes() {
        let mut fake = Fake::new().with_air(Air::open(b"\0\0\0\0", -40));

        let found = wifi::networks(&mut fake).unwrap();

        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn keeps_a_name_that_isnt_utf8_as_its_bytes() {
        let mut fake = Fake::new().with_air(Air::open(b"caf\xe9 \"1\"\n", -50));

        let found = wifi::networks(&mut fake).unwrap();

        assert_eq!(found[0].ssid.as_bytes(), b"caf\xe9 \"1\"\n");
    }
}

mod security_of {
    use super::*;

    fn home(securities: &[Security]) -> Vec<Network> {
        securities.iter().map(|&security| Network { ssid: ssid(b"Home"), security, signal: -50 }).collect()
    }

    #[test]
    fn a_name_both_open_and_secured_is_secured() {
        assert_eq!(wifi::security_of(&home(&[Security::Open, Security::Wpa2]), &ssid(b"Home")), Some(Security::Wpa2));
    }

    #[test]
    fn a_name_both_open_and_wpa3_only_is_wpa3_only() {
        assert_eq!(wifi::security_of(&home(&[Security::Open, Security::Wpa3]), &ssid(b"Home")), Some(Security::Wpa3));
    }
}

/// 35 saved networks whose names are 32 bytes, each escaped to 128 characters: more than one
/// LIST_NETWORKS reply holds.
fn many_long_names(fake: Fake) -> Fake {
    (0..35).fold(fake, |fake, n| {
        let mut name = [0xff; 32];
        name[0] = b'A' + n;
        fake.with_record(&name, "", false)
    })
}

mod saved {
    use super::*;

    #[test]
    fn reads_more_networks_than_one_reply_holds() {
        let mut fake = many_long_names(Fake::new());

        let saved = wifi::saved(&mut fake).unwrap();

        assert_eq!(saved.len(), 35);
    }

    #[test]
    fn a_join_keeps_networks_beyond_one_reply_joined_automatically() {
        let mut fake = many_long_names(Fake::new().with_air(Air::open(b"Cafe", -40)));

        join(&mut fake, b"Cafe", Key::Open, false).unwrap();

        assert!(fake.file.unwrap().iter().all(|r| !r.disabled));
    }

    #[test]
    fn rejects_a_reply_without_the_heading() {
        let err = wifi::saved(&mut Answers(|_| "0\tHome\tany\t\n")).unwrap_err();

        assert!(
            matches!(err, WifiError::Wpa(WpaError::BadReply { ref command, .. }) if command == "LIST_NETWORKS"),
            "{err:?}"
        );
    }
}

mod join {
    use super::*;

    fn home() -> Fake {
        Fake::new().with_air(Air::wpa2(b"Home", -50, &psk_hex("correct horse", b"Home")))
    }

    #[test]
    fn saves_the_network_with_the_hashed_key() {
        let mut fake = home();

        join(&mut fake, b"Home", Key::Passphrase(b"correct horse"), false).unwrap();

        let saved = fake.file.unwrap();
        assert_eq!(
            (saved[0].ssid.as_slice(), saved[0].psk.as_str()),
            (&b"Home"[..], psk_hex("correct horse", b"Home").as_str())
        );
    }

    #[test]
    fn never_sends_the_passphrase() {
        let mut fake = home();

        join(&mut fake, b"Home", Key::Passphrase(b"correct horse"), false).unwrap();

        assert!(!fake.log.iter().any(|c| c.contains("correct") || c.contains('"')), "{:?}", fake.log);
    }

    #[test]
    fn gives_the_address_dhcp_gave() {
        let mut fake = home();

        let joined = join(&mut fake, b"Home", Key::Passphrase(b"correct horse"), false).unwrap();

        assert_eq!(joined, Joined { id: 0, address: Some("192.168.1.20".into()) });
    }

    #[test]
    fn succeeds_without_an_address_when_dhcp_hasnt_answered() {
        let mut fake = home();
        fake.dhcp_after = None;

        let joined = join(&mut fake, b"Home", Key::Passphrase(b"correct horse"), false).unwrap();

        assert_eq!(joined.address, None);
    }

    #[test]
    fn keeps_whether_other_networks_join_automatically() {
        let mut fake =
            home().with_record(b"Work", "", false).with_record(b"Library", "", true).with_record(b"Phone", "", false);

        join(&mut fake, b"Home", Key::Passphrase(b"correct horse"), false).unwrap();

        assert_eq!(
            enabled(&fake.file.unwrap()),
            [
                (b"Work".to_vec(), true),
                (b"Library".to_vec(), false),
                (b"Phone".to_vec(), true),
                (b"Home".to_vec(), true)
            ]
        );
    }

    #[test]
    fn with_a_wrong_password_unreported_says_so_in_words() {
        let mut fake = home();
        fake.reports_wrong_key = false;

        let err = join(&mut fake, b"Home", Key::Passphrase(b"wrong horse"), false).unwrap_err();

        assert_eq!(err.to_string(), "couldn't join \"Home\": the password is wrong, or the network is out of reach");
    }

    #[test]
    fn with_a_wrong_password_reported_says_the_password_is_wrong() {
        let mut fake = home();

        let err = join(&mut fake, b"Home", Key::Passphrase(b"wrong horse"), false).unwrap_err();

        assert_eq!(err.to_string(), "couldn't join \"Home\": the password is wrong");
    }

    #[test]
    fn with_a_wrong_password_reported_stops_before_the_wait_is_up() {
        let mut fake = home();
        let slow = wifi::Wait { handshake: Duration::from_secs(20), ..QUICK };
        let mut events = fake.events();
        let start = std::time::Instant::now();

        let _ = wifi::join(&mut fake, &mut events, &ssid(b"Home"), Key::Passphrase(b"wrong horse"), false, &slow);

        assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
    }

    #[test]
    fn ignores_a_wrong_key_reported_for_another_network() {
        let mut fake = home();
        fake.events.lock().unwrap().push_back(
            "CTRL-EVENT-SSID-TEMP-DISABLED id=7 ssid=\"Elsewhere\" auth_failures=1 duration=10 reason=WRONG_KEY".into(),
        );

        let joined = join(&mut fake, b"Home", Key::Passphrase(b"correct horse"), false);

        assert!(joined.is_ok(), "{joined:?}");
    }

    #[test]
    fn that_fails_leaves_the_configuration_as_it_was_and_saves_nothing() {
        let mut fake = home().with_record(b"Work", "", false).with_record(b"Library", "", true);
        let before = fake.records.clone();

        let _ = join(&mut fake, b"Home", Key::Passphrase(b"wrong horse"), false);

        let saved = fake.log.iter().any(|c| c == "SAVE_CONFIG");
        assert!(fake.records == before && !saved, "{:?} {:?}", fake.records, fake.log);
    }

    #[test]
    fn lost_while_waiting_for_an_address_keeps_the_saved_network_and_saves_nothing() {
        let mut fake = home().with_record(b"Home", &psk_hex("old horse", b"Home"), false);
        fake.dhcp_after = None;
        fake.drops = true;
        let before = fake.file.clone();

        let err = join(&mut fake, b"Home", Key::Passphrase(b"correct horse"), false).unwrap_err();

        assert!(matches!(err, WifiError::NotJoined(_)) && fake.file == before, "{err:?} {:?}", fake.log);
    }

    #[test]
    fn stopped_as_it_completes_still_stops() {
        static STOP: AtomicBool = AtomicBool::new(false);
        struct StopOnStatus(Fake);
        impl Control for StopOnStatus {
            fn request(&mut self, command: &str) -> Result<String, WpaError> {
                let reply = self.0.request(command)?;
                if command == "STATUS" {
                    STOP.store(true, Ordering::Relaxed);
                }
                Ok(reply)
            }
        }
        let mut fake = home().with_record(b"Work", "", false);
        fake.join_polls = 1;
        fake.dhcp_after = Some(0);
        let before = fake.records.clone();
        let mut events = fake.events();
        let mut ctrl = StopOnStatus(fake);
        let wait = wifi::Wait { stop: Some(&STOP), ..QUICK };

        let err = wifi::join(&mut ctrl, &mut events, &ssid(b"Home"), Key::Passphrase(b"correct horse"), false, &wait)
            .unwrap_err();

        assert!(matches!(err, WifiError::Stopped(_)) && ctrl.0.records == before, "{err:?} {:?}", ctrl.0.log);
    }

    #[test]
    fn stopped_leaves_the_configuration_as_it_was() {
        static STOP: AtomicBool = AtomicBool::new(true);
        let mut fake = home().with_record(b"Work", "", false).with_record(b"Library", "", true);
        fake.reports_wrong_key = false;
        let before = fake.records.clone();
        let mut events = fake.events();
        let wait = wifi::Wait { stop: Some(&STOP), ..QUICK };

        let err = wifi::join(&mut fake, &mut events, &ssid(b"Home"), Key::Passphrase(b"wrong horse"), false, &wait)
            .unwrap_err();

        assert!(matches!(err, WifiError::Stopped(_)) && fake.records == before, "{err:?} {:?}", fake.log);
    }

    #[test]
    fn sends_a_hostile_name_only_in_hex_and_saves_its_bytes() {
        let name = b"x\"; reboot; $(id) <b>&\n\xff";
        let mut fake = Fake::new().with_air(Air::open(name, -50));

        join(&mut fake, name, Key::Open, false).unwrap();

        let sent = fake.log.contains(&format!("SET_NETWORK 0 ssid {}", fake::hex(name)));
        let saved = fake.file.as_deref().unwrap()[0].ssid == name;
        assert!(sent && saved, "{:?} {:?}", fake.log, fake.file);
    }

    #[test]
    fn joins_an_open_network_without_a_key() {
        let mut fake = Fake::new().with_air(Air::open(b"Cafe", -50));

        join(&mut fake, b"Cafe", Key::Open, false).unwrap();

        assert_eq!(fake.file.unwrap()[0].key_mgmt, "NONE");
    }

    #[test]
    fn joins_a_hidden_network_by_asking_for_its_name() {
        let psk = psk_hex("correct horse", b"Secret");
        let mut fake = Fake::new().with_air(Air { hidden: true, ..Air::wpa2(b"Secret", -50, &psk) });

        join(&mut fake, b"Secret", Key::Passphrase(b"correct horse"), true).unwrap();

        assert!(fake.file.unwrap()[0].scan_ssid);
    }

    #[test]
    fn without_a_key_keeps_a_saved_record_of_the_same_name() {
        let key = psk_hex("correct horse", b"Home");
        let mut fake = Fake::new().with_air(Air::open(b"Home", -30)).with_record(b"Home", &key, false);

        join(&mut fake, b"Home", Key::Open, false).unwrap();

        let kinds: Vec<String> = fake.file.unwrap().iter().map(|r| r.key_mgmt.clone()).collect();
        assert_eq!(kinds, ["WPA-PSK", "NONE"]);
    }

    #[test]
    fn replaces_an_older_record_of_the_same_name() {
        let mut fake = home().with_record(b"Home", &psk_hex("old horse", b"Home"), false);

        join(&mut fake, b"Home", Key::Passphrase(b"correct horse"), false).unwrap();

        let homes: Vec<String> =
            fake.file.unwrap().iter().filter(|r| r.ssid == b"Home").map(|r| r.psk.clone()).collect();
        assert_eq!(homes, [psk_hex("correct horse", b"Home")]);
    }

    #[test]
    fn replacing_records_not_joined_automatically_keeps_the_new_one_that_way() {
        let mut fake = home().with_record(b"Home", &psk_hex("old horse", b"Home"), true);

        join(&mut fake, b"Home", Key::Passphrase(b"correct horse"), false).unwrap();

        assert_eq!((enabled(&fake.file.clone().unwrap()), fake.current), (vec![(b"Home".to_vec(), false)], Some(1)));
    }

    #[test]
    fn replacing_records_one_joined_automatically_joins_the_new_one_automatically() {
        let old = psk_hex("old horse", b"Home");
        let mut fake = home().with_record(b"Home", &old, true).with_record(b"Home", &old, false);

        join(&mut fake, b"Home", Key::Passphrase(b"correct horse"), false).unwrap();

        assert_eq!(enabled(&fake.file.unwrap()), [(b"Home".to_vec(), true)]);
    }

    #[test]
    fn refuses_a_short_passphrase_before_asking_wpa_supplicant_anything() {
        let mut fake = home();

        let err = join(&mut fake, b"Home", Key::Passphrase(b"short"), false).unwrap_err();

        assert!(matches!(err, WifiError::Passphrase) && fake.log.is_empty(), "{err:?} {:?}", fake.log);
    }

    #[test]
    fn says_wpa_supplicant_refused_a_new_network() {
        let mut refuses =
            Answers(|c| if c.starts_with("LIST_NETWORKS") { "network id / ssid / bssid / flags\n" } else { "FAIL\n" });

        let err = wifi::join(&mut refuses, &mut NoEvents, &ssid(b"Home"), Key::Open, false, &QUICK).unwrap_err();

        assert!(
            matches!(err, WifiError::Wpa(WpaError::Refused { ref command, .. }) if command == "ADD_NETWORK"),
            "{err:?}"
        );
    }

    #[test]
    fn says_so_when_wpa_supplicant_cant_save() {
        let mut fake = home();
        fake.save_fails = true;

        let err = join(&mut fake, b"Home", Key::Passphrase(b"correct horse"), false).unwrap_err();

        assert!(matches!(err, WifiError::Unsaved(ref name) if name == "Home"), "{err:?}");
    }
}

mod scan_results {
    use super::*;

    #[test]
    fn are_as_old_as_the_newest_sighting() {
        let mut fake = Fake::new()
            .with_air(Air { age: 700, ..Air::open(b"Cafe", -60) })
            .with_air(Air { age: 650, ..Air::wpa2(b"Home", -40, "k") });

        assert_eq!(wifi::scan_results(&mut fake).unwrap().age, Some(650));
    }

    #[test]
    fn without_networks_have_no_age() {
        assert_eq!(wifi::scan_results(&mut Fake::new()).unwrap().age, None);
    }
}

mod disconnect {
    use super::*;

    #[test]
    fn leaves_the_network_joined() {
        let psk = psk_hex("correct horse", b"Home");
        let mut fake = Fake::new().with_air(Air::wpa2(b"Home", -50, &psk)).with_record(b"Home", &psk, false);
        fake.current = Some(0);

        wifi::disconnect(&mut fake).unwrap();

        assert_eq!(fake.current, None);
    }
}

mod connect {
    use super::*;

    #[test]
    fn joins_a_saved_network() {
        let psk = psk_hex("correct horse", b"Home");
        let mut fake = Fake::new().with_air(Air::wpa2(b"Home", -50, &psk)).with_record(b"Home", &psk, false);

        let joined = connect(&mut fake, 0).unwrap();

        assert_eq!(joined.id, 0);
    }

    #[test]
    fn keeps_whether_other_networks_join_automatically() {
        let psk = psk_hex("correct horse", b"Home");
        let mut fake = Fake::new()
            .with_air(Air::wpa2(b"Home", -50, &psk))
            .with_record(b"Work", "", false)
            .with_record(b"Library", "", true)
            .with_record(b"Home", &psk, false);

        connect(&mut fake, 2).unwrap();

        assert_eq!(
            enabled(&fake.records),
            [(b"Work".to_vec(), true), (b"Library".to_vec(), false), (b"Home".to_vec(), true)]
        );
    }

    #[test]
    fn stays_joined_to_a_network_not_joined_automatically_and_keeps_it_that_way() {
        let psk = psk_hex("correct horse", b"Away");
        let mut fake = Fake::new().with_air(Air::wpa2(b"Away", -50, &psk)).with_record(b"Away", &psk, true);

        connect(&mut fake, 0).unwrap();

        assert_eq!((fake.current, enabled(&fake.records)), (Some(0), vec![(b"Away".to_vec(), false)]));
    }

    #[test]
    fn that_fails_leaves_a_network_not_joined_automatically_as_it_was() {
        let mut fake = Fake::new().with_record(b"Work", "", false).with_record(b"Away", "", true);
        let before = fake.records.clone();

        let _ = connect(&mut fake, 1);

        assert_eq!(fake.records, before);
    }

    #[test]
    fn an_unknown_id_says_so() {
        let mut fake = Fake::new();

        let err = connect(&mut fake, 7).unwrap_err();

        assert!(matches!(err, WifiError::NoId(7)), "{err:?}");
    }
}

mod forget {
    use super::*;

    #[test]
    fn removes_every_record_of_the_name_and_saves() {
        let mut fake =
            Fake::new().with_record(b"Home", "", false).with_record(b"Work", "", false).with_record(b"Home", "", true);

        let removed = wifi::forget(&mut fake, &ssid(b"Home")).unwrap();

        assert_eq!((removed, enabled(&fake.file.unwrap())), (2, vec![(b"Work".to_vec(), true)]));
    }

    #[test]
    fn a_name_not_saved_says_so() {
        let mut fake = Fake::new().with_record(b"Work", "", false);

        let err = wifi::forget(&mut fake, &ssid(b"Home")).unwrap_err();

        assert_eq!(err.to_string(), "\"Home\" isn't a saved network");
    }

    #[test]
    fn says_so_when_wpa_supplicant_cant_save() {
        let mut fake = Fake::new().with_record(b"Home", "", false);
        fake.save_fails = true;

        let err = wifi::forget(&mut fake, &ssid(b"Home")).unwrap_err();

        assert!(matches!(err, WifiError::NotForgotten(ref name) if name == "Home"), "{err:?}");
    }

    #[test]
    fn matches_the_name_byte_for_byte() {
        let mut fake = Fake::new().with_record(b"caf\xe9", "", false).with_record("café".as_bytes(), "", false);

        wifi::forget(&mut fake, &ssid(b"caf\xe9")).unwrap();

        assert_eq!(enabled(&fake.file.unwrap()), [("café".as_bytes().to_vec(), true)]);
    }
}

mod set_autoconnect {
    use super::*;

    #[test]
    fn off_saves_every_record_of_the_name_not_joined_automatically() {
        let mut fake =
            Fake::new().with_record(b"Home", "", false).with_record(b"Work", "", false).with_record(b"Home", "", false);

        wifi::set_autoconnect(&mut fake, &ssid(b"Home"), false).unwrap();

        assert_eq!(
            enabled(&fake.file.unwrap()),
            [(b"Home".to_vec(), false), (b"Work".to_vec(), true), (b"Home".to_vec(), false)]
        );
    }

    #[test]
    fn off_leaves_the_network_joined() {
        let psk = psk_hex("correct horse", b"Home");
        let mut fake = Fake::new().with_air(Air::wpa2(b"Home", -50, &psk)).with_record(b"Home", &psk, false);
        connect(&mut fake, 0).unwrap();

        wifi::set_autoconnect(&mut fake, &ssid(b"Home"), false).unwrap();

        assert_eq!(fake.current, Some(0));
    }

    #[test]
    fn on_saves_the_network_joined_automatically_without_joining_it_now() {
        let mut fake = Fake::new().with_record(b"Home", "", true);

        wifi::set_autoconnect(&mut fake, &ssid(b"Home"), true).unwrap();

        let no_connect = fake.log.contains(&"ENABLE_NETWORK 0 no-connect".to_owned());
        assert_eq!(
            (enabled(fake.file.as_deref().unwrap()), no_connect),
            (vec![(b"Home".to_vec(), true)], true),
            "{:?}",
            fake.log
        );
    }

    #[test]
    fn a_name_not_saved_says_so() {
        let mut fake = Fake::new().with_record(b"Work", "", false);

        let err = wifi::set_autoconnect(&mut fake, &ssid(b"Home"), false).unwrap_err();

        assert_eq!(err.to_string(), "\"Home\" isn't a saved network");
    }

    #[test]
    fn says_so_when_wpa_supplicant_cant_save() {
        let mut fake = Fake::new().with_record(b"Home", "", false);
        fake.save_fails = true;

        let err = wifi::set_autoconnect(&mut fake, &ssid(b"Home"), false).unwrap_err();

        assert!(matches!(err, WifiError::NotKept(ref name) if name == "Home"), "{err:?}");
    }
}

mod status {
    use super::*;

    #[test]
    fn reads_the_network_in_use() {
        let psk = psk_hex("correct horse", b"Home");
        let mut fake = Fake::new().with_air(Air::wpa2(b"Home", -50, &psk)).with_record(b"Home", &psk, false);
        connect(&mut fake, 0).unwrap();

        let status = wifi::status(&mut fake).unwrap();

        assert_eq!((status.joined(), status.ssid, status.id), (true, Some(ssid(b"Home")), Some(0)));
    }

    #[test]
    fn reads_the_signal_of_the_network_in_use() {
        let psk = psk_hex("correct horse", b"Home");
        let mut fake = Fake::new().with_air(Air::wpa2(b"Home", -48, &psk)).with_record(b"Home", &psk, false);
        connect(&mut fake, 0).unwrap();

        let signal = wifi::signal(&mut fake).unwrap();

        assert_eq!(signal, Some(-48));
    }

    #[test]
    fn has_no_signal_when_nothing_is_joined() {
        let mut fake = Fake::new();

        let signal = wifi::signal(&mut fake).unwrap();

        assert_eq!(signal, None);
    }

    #[test]
    fn rejects_a_reply_without_a_state() {
        let err = wifi::status(&mut Answers(|_| "bssid=02:00:00:00:00:01\n")).unwrap_err();

        assert!(
            matches!(err, WifiError::Wpa(WpaError::BadReply { ref command, .. }) if command == "STATUS"),
            "{err:?}"
        );
    }
}
