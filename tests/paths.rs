//! Where a build puts hyprbsd-network's helper and finds Noto's fonts: under the PREFIX and LOCALBASE it
//! was given (HYPRBSD_NETWORK_PREFIX, HYPRBSD_NETWORK_LOCALBASE), /usr/local without them.

use hyprbsd_network::paths;

const PREFIX: &str = match option_env!("HYPRBSD_NETWORK_PREFIX") {
    Some(p) => p,
    None => "/usr/local",
};
const LOCALBASE: &str = match option_env!("HYPRBSD_NETWORK_LOCALBASE") {
    Some(p) => p,
    None => "/usr/local",
};

#[test]
fn the_helper_should_be_under_the_prefix() {
    assert_eq!(paths::HELPER, format!("{PREFIX}/libexec/hyprbsd-network-helper"));
}

#[test]
fn noto_should_be_under_localbase() {
    assert_eq!(paths::FONTS, format!("{LOCALBASE}/share/fonts/noto"));
}
