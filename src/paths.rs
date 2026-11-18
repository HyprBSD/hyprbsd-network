//! hyprbsd-network's helper under the build's PREFIX, and Noto's fonts under its LOCALBASE (build.rs).

/// The root helper, run through pkexec.
pub const HELPER: &str = concat!(env!("NETWORK_PREFIX"), "/libexec/hyprbsd-network-helper");

/// Noto Sans, from x11-fonts/noto-sans.
pub const FONTS: &str = concat!(env!("NETWORK_LOCALBASE"), "/share/fonts/noto");
