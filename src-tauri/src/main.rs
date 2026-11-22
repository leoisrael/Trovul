// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // The same exe also runs as the input service and its helper (input_service.rs).
    match std::env::args().nth(1).as_deref() {
        Some("--input-service") => trovul_lib::run_input_service(),
        Some("--input-helper") => trovul_lib::run_input_helper(),
        _ => trovul_lib::run(),
    }
}
