mod config;
mod scanner;
mod builder;
mod resolver;
mod backends;
mod lockfile;
mod cache;
mod registry;
mod reporter;

use clap::{Parser, Subcommand};
use reporter::Reporter;

#[derive(Parser)]
#[command(name = "cook", version = "0.1.0", about = "C++ build tool")]
struct Cli {
    #[arg(short = 'r', long = "release", global = true)]
    release: bool,
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count, global = true)]
    verbose: u8,
    #[arg(long = "quiet", global = true, conflicts_with = "verbose")]
    quiet: bool,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
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
    let reporter = Reporter::new(cli.verbose, cli.quiet);
    reporter.debug(format!("starting command: {:?}", cli.command));

    match cli.command {
        Some(Commands::New { name }) => {
            if let Err(e) = builder::new_project(&name, &reporter) {
                reporter.error(e.to_string());
            }
        }
        Some(Commands::Build) | None => {
            if let Err(e) = builder::build_project(cli.release, &reporter) {
                reporter.error(e.to_string());
            }
        }
        Some(Commands::Clean) => {
            if let Err(e) = builder::clean_project(&reporter) {
                reporter.error(e.to_string());
            }
        }
        Some(Commands::Lock) => {
            if let Err(e) = builder::lock_project(&reporter) {
                reporter.error(e.to_string());
            }
        }
        Some(Commands::Add { name, url }) => {
            if let Err(e) = builder::add_dependency(&name, url.as_deref(), &reporter) {
                reporter.error(e.to_string());
            }
        }
        Some(Commands::Run) => {
            if let Err(e) = builder::run_project(cli.release, &reporter) {
                reporter.error(e.to_string());
            }
        }
    }
}