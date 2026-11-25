mod auth_qr_view;
#[cfg(unix)]
pub mod cardputer;
pub mod channel;
pub mod channel_registry;
pub mod cli;
pub mod config;
#[cfg(unix)]
mod config_io;
pub mod console;
pub mod context;
pub mod gateway;
pub mod hardware;
pub mod http;
#[cfg(unix)]
mod mcp;
pub mod memory;
pub mod messages;
pub mod model;
pub mod model_capabilities;
pub mod model_history;
#[cfg(unix)]
pub mod model_metadata;
pub mod process_output;
pub mod repl_input;
pub mod runtime_reload;
mod secrets;
pub mod session;
pub mod session_history;
pub mod skills;
pub mod steering;
pub mod terminal_qr;
pub mod time_context;
pub mod tools;
pub mod transcript;
pub mod vision;
pub mod weixin;

mod file_ops;

#[cfg(unix)]
mod jobs;
#[cfg(unix)]
mod tasks;
