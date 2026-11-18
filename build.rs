//! The bases paths.rs builds on: HYPRBSD_NETWORK_PREFIX and HYPRBSD_NETWORK_LOCALBASE, /usr/local
//! without them, as NETWORK_PREFIX and NETWORK_LOCALBASE.

fn main() {
    for (given, base) in
        [("HYPRBSD_NETWORK_PREFIX", "NETWORK_PREFIX"), ("HYPRBSD_NETWORK_LOCALBASE", "NETWORK_LOCALBASE")]
    {
        println!("cargo::rerun-if-env-changed={given}");
        let path = std::env::var(given).unwrap_or_else(|_| "/usr/local".to_owned());
        println!("cargo::rustc-env={base}={path}");
    }
}
