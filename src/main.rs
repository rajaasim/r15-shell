mod app;
mod chat;
mod config;

use clap::Parser;
use config::Args;

fn main() {
    let args = Args::parse();
    if let Err(error) = app::run(args) {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
