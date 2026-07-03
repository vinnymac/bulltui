//! bulltui: a terminal UI for BullMQ with feature parity with bull-board.
//!
//! The binary is a thin wrapper around this library so the application state
//! ([`app::App`]) and rendering ([`ui`]) can be driven directly from tests with
//! ratatui's `TestBackend`.

pub mod app;
pub mod boot;
pub mod cli;
pub mod clipboard;
pub mod events;
pub mod format;
pub mod fuzzy;
pub mod fx;
pub mod keymap;
pub mod state;
pub mod theme;
pub mod ui;
