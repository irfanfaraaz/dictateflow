//! wispertype — macOS dictation app entry point.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    wispertype_lib::run();
}
