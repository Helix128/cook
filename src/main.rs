mod config;
mod scanner;
mod builder;
mod resolver;
mod backends;
mod lockfile;
mod cache;
mod registry;

use clap::{Parser, Subcommand};
use colored::*;

#[derive(Parser)]
#[command(name = "cook", version = "0.1.0", about = "C++ build tool")]
struct Cli {
    #[arg(short = 'r', long = "release", global = true)]
    release: bool,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    New { name: String },
    Build,
    Clean,
    Lock,
    Add {
        name: String,
        #[arg(required = false)]
        url: Option<String>,
    },
    Run,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::New { name }) => {
            if let Err(e) = builder::new_project(&name) {
                eprintln!("{} {}", "ERROR:".red(), e);
            }
        }
        Some(Commands::Build) | None => {
            if let Err(e) = builder::build_project(cli.release) {
                eprintln!("{} {}", "FAILED:".red(), e);
            }
        }
        Some(Commands::Clean) => {
            if let Err(e) = builder::clean_project() {
                eprintln!("{} {}", "FAILED:".red(), e);
            }
        }
        Some(Commands::Lock) => {
            if let Err(e) = builder::lock_project() {
                eprintln!("{} {}", "FAILED:".red(), e);
            }
        }
        Some(Commands::Add { name, url }) => {
            if let Err(e) = builder::add_dependency(&name, url.as_deref()) {
                eprintln!("{} {}", "FAILED:".red(), e);
            }
        }
        Some(Commands::Run) => {
            if let Err(e) = builder::run_project(cli.release) {
                eprintln!("{} {}", "FAILED:".red(), e);
            }
        }
    }
}