#![windows_subsystem = "windows"]

mod dialogs;
mod git;
mod github;
mod theme;
mod ui;

fn main() {
    if let Err(error) = ui::run() {
        ui::startup_error(&error.to_string());
        std::process::exit(1);
    }
}
