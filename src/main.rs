#![windows_subsystem = "windows"]

mod types;
mod image;
mod win;
mod ort;
mod paddle;
mod layout;
mod config;
mod capture;
mod overlay;
mod tray;
mod everything;
mod profile;
mod card;
mod dialogs;
mod searchbox;
mod app;

fn main() {
    app::run();
}

// touch
