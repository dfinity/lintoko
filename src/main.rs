use clap::Parser;
use std::{path::PathBuf, process};
use tracing::{debug, level_filters::LevelFilter};

/// Lint Motoko code according to a set of rules
#[derive(Parser, Debug)]
#[command(name = "lintoko")]
#[command(about = "A CLI tool for linting Motoko code")]
#[command(version = "0.1.0")]
struct Args {
    /// Input Motoko file to process
    #[arg(value_name = "FILE")]
    input: PathBuf,

    /// Verbose output
    #[arg(short, long)]
    verbose: bool,

    /// Directories containing extra rules
    #[arg(short, long, value_name = "DIRECTORY")]
    rules: Vec<PathBuf>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Initialize tracing subscriber
    let filter = if args.verbose {
        LevelFilter::DEBUG
    } else {
        LevelFilter::INFO
    };

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter.to_string()))
        .with_target(false)
        .without_time()
        .init();

    debug!("Linting file: {}", args.input.display());

    let file_content = std::fs::read_to_string(&args.input)
        .map_err(|e| format!("Failed to read file '{}': {}", args.input.display(), e))?;

    let mut rules = lintoko::default_rules();
    for dir in args.rules {
        debug!("Loading rules from: {}", dir.display());
        rules.extend(lintoko::load_rules_from_directory(&dir)?);
    }

    let error_count = lintoko::lint_file(
        args.input.to_string_lossy().as_ref(),
        &file_content,
        rules,
        std::io::stderr(),
    )?;
    if error_count > 0 {
        eprintln!("Found {error_count} errors");
        process::exit(1)
    } else {
        Ok(())
    }
}
