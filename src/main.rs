mod app;
mod args;
mod canvas;
mod export;
mod terminal;
mod theme;
mod vivid;

use anyhow::Result;
use clap::Parser;

fn main() -> Result<()> {
    app::run(args::Args::parse())
}
