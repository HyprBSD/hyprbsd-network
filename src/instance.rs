//! One window at a time: the window listens on a socket that a second start knocks on.

use std::fs;
use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;

/// The open window's socket, removed when it closes.
#[derive(Debug)]
pub struct Window {
    listener: UnixListener,
    path: PathBuf,
}

impl Window {
    /// Calls `knock`, on a thread of its own, each time another start knocks.
    pub fn on_knock(&self, knock: impl Fn() + Send + 'static) -> io::Result<()> {
        let listener = self.listener.try_clone()?;
        thread::spawn(move || {
            for stream in listener.incoming() {
                if stream.is_ok() {
                    knock();
                }
            }
        });
        Ok(())
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Debug)]
pub enum Instance {
    /// This is the window.
    First(Window),
    /// A window is open already, and was asked to show itself.
    Running,
}

/// The window's socket in the user's runtime directory.
pub fn path() -> PathBuf {
    crate::wpa::runtime_dir().join("hyprbsd-network.window")
}

/// The window's socket at `path`, or a knock on the one a window holds there already. A
/// socket nothing listens on is taken over.
pub fn claim(path: &Path) -> io::Result<Instance> {
    let first = |listener| Ok(Instance::First(Window { listener, path: path.to_owned() }));
    match UnixListener::bind(path) {
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => match UnixStream::connect(path) {
            Ok(_) => Ok(Instance::Running),
            Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                fs::remove_file(path)?;
                first(UnixListener::bind(path)?)
            }
            Err(e) => Err(e),
        },
        bound => first(bound?),
    }
}
