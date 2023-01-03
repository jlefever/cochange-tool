mod tagging;

use std::time::Instant;
use std::{fs::read_to_string, path::PathBuf};

use anyhow::Context;
use clap_verbosity_flag::{InfoLevel, Verbosity};
use tree_sitter::Language;

use crate::tagging::TagGenerator;

#[macro_use]
extern crate derive_builder;

#[derive(Debug, clap::Parser)]
#[clap(author, version, about, long_about = None)]
struct Cli {
    /// Name of source file to load
    #[clap(value_parser)]
    filename: PathBuf,

    #[clap(flatten, help_heading = "LOG OPTIONS")]
    verbose: Verbosity<InfoLevel>,
}

fn main() -> anyhow::Result<()> {
    let cli = <Cli as clap::Parser>::parse();
    env_logger::Builder::new().filter_level(cli.verbose.log_level_filter()).init();

    extern "C" {
        fn tree_sitter_java() -> Language;
    }

    let language = unsafe { tree_sitter_java() };

    let start = Instant::now();
    let source_code = read_to_string(cli.filename).context("failed to read source file")?;
    let read_elapsed = start.elapsed();

    let java_query = include_str!("../queries/java/tags.scm");

    let start = Instant::now();
    let mut generator = TagGenerator::new(language, java_query)?;
    let generation_elapsed = start.elapsed();

    log::info!(
        "Read file in {} ms. Generated tags in {} ms.",
        read_elapsed.as_millis(),
        generation_elapsed.as_millis()
    );

    for tag in generator.generate_tags(source_code)? {
        println!("{tag:#?}");
    }

    Ok(())
}
