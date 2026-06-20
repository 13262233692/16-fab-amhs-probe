use anyhow::Result;
use clap::Parser;

mod cli;
mod digraph;
mod event;
mod parser;
mod probe;
mod scanner;

use cli::Args;
use probe::run_probe;

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args = Args::parse();
    run_probe(args)
}
