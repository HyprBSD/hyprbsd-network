//! Wi-Fi as rc.conf and the kernel have it, from `sysctl -n net.wlan.devices` and `sysrc -a`.

use hyprbsd_network::radio::{self, Radio};

const SET_UP: &str = "wlans_iwlwifi0: wlan0\nifconfig_wlan0: WPA DHCP\n";

#[test]
fn without_an_adapter_there_is_none() {
    assert_eq!(radio::parse("\n", SET_UP), Radio::NoAdapter);
}

#[test]
fn an_adapter_without_a_wlan_isnt_set_up() {
    assert_eq!(radio::parse("iwlwifi0\n", "ifconfig_DEFAULT: DHCP\n"), Radio::NotSetUp);
}

#[test]
fn a_wlan_without_wpa_supplicant_isnt_set_up() {
    assert_eq!(
        radio::parse("iwlwifi0\n", "wlans_iwlwifi0: wlan0\nifconfig_wlan0: inet 10.0.0.2/24\n"),
        Radio::NotSetUp
    );
}

#[test]
fn a_wlan_with_wpa_supplicant_is_on() {
    assert_eq!(radio::parse("iwlwifi0\n", SET_UP), Radio::On);
}

#[test]
fn noauto_is_off() {
    assert_eq!(radio::parse("iwlwifi0\n", "wlans_iwlwifi0: wlan0\nifconfig_wlan0: WPA DHCP NOAUTO\n"), Radio::Off);
}

#[test]
fn words_count_in_any_case() {
    assert_eq!(radio::parse("iwlwifi0\n", "wlans_iwlwifi0: wlan0\nifconfig_wlan0: wpa noauto dhcp\n"), Radio::Off);
}

#[test]
fn one_wlan_on_is_on() {
    let rc = "wlans_iwlwifi0: wlan0 wlan1\nifconfig_wlan0: WPA DHCP NOAUTO\nifconfig_wlan1: WPA DHCP\n";

    assert_eq!(radio::parse("iwlwifi0\n", rc), Radio::On);
}

#[test]
fn noauto_as_part_of_a_word_doesnt_count() {
    assert_eq!(radio::parse("iwlwifi0\n", "wlans_iwlwifi0: wlan0\nifconfig_wlan0: WPA DHCP xNOAUTO\n"), Radio::On);
}
