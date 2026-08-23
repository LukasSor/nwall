
mod app;
mod consts;
mod ipc_util;
mod theme;
mod ui;
mod widgets;

use adw::prelude::*;
use anyhow::Result;
use gtk::glib;
use gtk::prelude::ApplicationExtManual;
use nwall_ipc::{client_request, Request};

use crate::consts::APP_ID;

fn main() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .build();

    app.connect_activate(|app| {
        if let Some(win) = app.windows().into_iter().next() {
            win.present();
            return;
        }
        if let Err(e) = ensure_daemon() {
            eprintln!("warn: {e:#}");
        }
        ui::build(app);
    });

    app.run()
}

fn ensure_daemon() -> Result<()> {
    if client_request(&Request::Ping).is_ok() {
        return Ok(());
    }
    let _ = std::process::Command::new("nwall").args(["daemon"]).status();
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if client_request(&Request::Ping).is_ok() {
            return Ok(());
        }
    }
    anyhow::bail!("could not start nwalld")
}
