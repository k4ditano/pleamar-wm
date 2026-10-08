//! The long-lived Windows session needs neither a console nor the renderer.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
#[path = "../window_rules.rs"]
mod window_rules;

#[cfg(windows)]
#[path = "../layout.rs"]
mod layout;
#[cfg(windows)]
#[allow(dead_code)]
#[path = "../windows_backend.rs"]
mod windows_backend;

#[cfg(windows)]
fn main() {
    std::process::exit(windows_backend::run_session(std::env::args().skip(1).collect()));
}

#[cfg(not(windows))]
fn main() {
    eprintln!("pleamar-wm-host is only used by the Windows desktop companion");
    std::process::exit(1);
}
