//! QDP's Linux app: the computers to stream from, found on the local network
//! or paired with, pairing with the code a host shows, and a stream for each
//! in the window `qdp connect` opens, which the app runs and follows. It is
//! built with GTK 4 and libadwaita, whose look GNOME uses and other desktops
//! show well, and needs the `qdp` program, installed beside it or on the
//! `PATH`.

#[cfg(target_os = "linux")]
mod command;
#[cfg(target_os = "linux")]
mod discovery;
#[cfg(target_os = "linux")]
mod hosts;
#[cfg(target_os = "linux")]
mod key_pin;
#[cfg(target_os = "linux")]
mod rows;
#[cfg(target_os = "linux")]
mod session;
#[cfg(target_os = "linux")]
mod session_events;
#[cfg(target_os = "linux")]
mod settings;
#[cfg(target_os = "linux")]
mod window;

#[cfg(target_os = "linux")]
fn main() -> gtk::glib::ExitCode {
    use adw::prelude::*;

    let application = adw::Application::builder()
        .application_id(window::APP_ID)
        .build();
    application.connect_startup(window::startup);
    application.connect_activate(window::activate);
    application.run()
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("QDP's GTK app runs on Linux; the Mac and Windows apps are their own.");
    std::process::ExitCode::FAILURE
}
