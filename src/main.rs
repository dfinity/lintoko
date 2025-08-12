use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use glob::glob_with;
use std::path::PathBuf;
use std::{collections::BTreeSet, path::Path};
use tracing::{debug, level_filters::LevelFilter};

/// Lint Motoko code according to a set of rules
#[derive(Parser, Debug)]
#[command(name = "lintoko")]
#[command(about = "A CLI tool for linting Motoko code")]
#[command(version = "0.1.0")]
struct Args {
    /// Files, directories, or globs of Motoko files to lint
    #[arg(value_name = "INPUTS")]
    inputs: Vec<String>,

    /// Verbose output
    #[arg(short, long)]
    verbose: bool,

    /// Directories containing extra rules
    #[arg(short, long, value_name = "DIRECTORY")]
    rules: Vec<PathBuf>,
}

/// Expands passed input parameters (skips hidden directories, unless explicitly referenced)
/// - If the input references a file, just match that file
/// - If the input references a directory, expand to all `.mo` files nested underneath it
/// - Otherwise interprets the input as a glob
fn expand_input(input: &String) -> Vec<PathBuf> {
    let path = Path::new(input);
    let match_options = glob::MatchOptions {
        require_literal_leading_dot: true,
        ..glob::MatchOptions::new()
    };
    if path.is_dir() {
        debug!("directory input: {}", input);
        let g = format!("{input}/**/*.mo");
        glob_with(&g, match_options)
            .expect("Invalid glob")
            .filter_map(Result::ok)
            .collect()
    } else if path.is_file() {
        debug!("file input: {}", input);
        vec![PathBuf::from(input.to_string())]
    } else {
        debug!("glob input: {}", input);
        glob_with(input, match_options)
            .expect("Invalid glob")
            .filter_map(Result::ok)
            .collect()
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
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

    let inputs = if args.inputs.is_empty() {
        vec!["**/*.mo".to_string()]
    } else {
        args.inputs
    };
    // Collecting into a Set here to guarantee we only lint every file once.
    let all_files: BTreeSet<PathBuf> = inputs.iter().flat_map(expand_input).collect();
    if all_files.is_empty() {
        bail!("Input patterns did not match any files")
    }

    let mut rules = lintoko::default_rules();
    for dir in &args.rules {
        debug!("Loading rules from: {}", dir.display());
        rules.extend(lintoko::load_rules_from_directory(dir)?);
    }

    let mut error_count = 0;
    for input in all_files {
        debug!("Linting file: {}", input.display());

        let file_content = std::fs::read_to_string(&input)
            .with_context(|| anyhow!("Failed to read file at '{}'", input.display()))?;

        error_count += lintoko::lint_file(
            input.to_string_lossy().as_ref(),
            &file_content,
            &rules,
            std::io::stderr(),
        )?;
    }

    if error_count > 0 {
        bail!("Found {error_count} errors")
    } else {
        Ok(())
    }
}
