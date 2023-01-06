mod tagging;

use std::env::current_dir;
use std::io::{self, Read};
use std::time::Instant;
use std::{fs::read_to_string, path::PathBuf};

use anyhow::Result;
use anyhow::{bail, Context};
use clap_verbosity_flag::{InfoLevel, Verbosity};
use git2::{Oid, Repository};
use tree_sitter::Language;

use crate::tagging::TagGenerator;

#[macro_use]
extern crate derive_builder;

#[derive(Debug, clap::Parser)]
#[clap(author, version, about, long_about = None)]
struct Cli {
    /// Path to the store of co-change data. This is a sqlite3 database.
    #[clap(short, long)]
    store: PathBuf,

    /// Use parents from input rather than looking up the true parent from the
    /// repository. This allows for history simplification. Only use if
    /// --parents was given to git-rev-parse.
    #[clap(short, long, action)]
    parents: bool,

    #[clap(flatten, help_heading = "LOG OPTIONS")]
    verbose: Verbosity<InfoLevel>,
}

#[derive(Debug)]
struct CommitRequest {
    commit: Oid,
    parents: Vec<Oid>,
}

impl CommitRequest {
    fn new(commit: Oid) -> Self {
        Self { commit, parents: Vec::new() }
    }

    fn with_parents(commit: Oid, parents: Vec<Oid>) -> Self {
        Self { commit, parents }
    }

    fn from_vec(hashs: Vec<Oid>) -> Result<Self> {
        match hashs.len() {
            0 => bail!("expected at least one hash"),
            1 => Ok(Self::new(hashs[0])),
            _ => Ok(Self::with_parents(hashs[0], hashs[1..].into())),
        }
    }

    fn has_parents(&self) -> bool {
        !self.parents.is_empty()
    }
}

fn parse_commit_req<S: AsRef<str>>(text: S) -> Result<CommitRequest> {
    let hashs: Result<Vec<Oid>> = text
        .as_ref()
        .split(' ')
        .enumerate()
        .map(|(i, hash)| {
            Oid::from_str(hash).with_context(|| format!("failed to parse hash #{}", i + 1))
        })
        .collect();

    CommitRequest::from_vec(hashs?)
}

fn parse_commit_reqs<S: AsRef<str>>(text: S) -> Result<Vec<CommitRequest>> {
    text.as_ref()
        .lines()
        .enumerate()
        .map(|(line_no, line)| {
            parse_commit_req(line).with_context(|| format!("failed to parse line {}", line_no + 1))
        })
        .collect()
}

fn lookup_parents(repo: Repository, req: CommitRequest) -> Result<CommitRequest> {
    let commit = repo.find_commit(req.commit)?;
    let parents: Vec<Oid> = commit.parent_ids().collect();
    Ok(CommitRequest::with_parents(req.commit, parents))
}

// fn replace_parents(repo: Repository, req: CommitRequest) -> Result<CommitRequest> {
//     if reqs.iter().all(|r| r.has_parents()) {
//         return Ok();
//     }
// }

fn main() -> anyhow::Result<()> {
    let cli = <Cli as clap::Parser>::parse();
    env_logger::Builder::new().filter_level(cli.verbose.log_level_filter()).init();

    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;
    let reqs = parse_commit_reqs(&buffer)?;

    let cwd = current_dir().context("failed to access the current working directory")?;
    let repo = Repository::discover(cwd)
        .context("failed to find git repository in the current directory")?;

    if reqs.iter().all(|r| !r.has_parents()) {}

    println!("{reqs:#?}");

    Ok(())
}

// fn main2() -> anyhow::Result<()> {
//     let cli = <Cli as clap::Parser>::parse();
//     env_logger::Builder::new().filter_level(cli.verbose.log_level_filter()).init();

//     extern "C" {
//         fn tree_sitter_java() -> Language;
//     }

//     let language = unsafe { tree_sitter_java() };

//     let start = Instant::now();
//     let source_code = read_to_string(cli.filename).context("failed to read source file")?;
//     let read_elapsed = start.elapsed();

//     let java_query = include_str!("../queries/java/tags.scm");

//     let start = Instant::now();
//     let mut generator = TagGenerator::new(language, java_query)?;
//     let generation_elapsed = start.elapsed();

//     log::info!(
//         "Read file in {} ms. Generated tags in {} ms.",
//         read_elapsed.as_millis(),
//         generation_elapsed.as_millis()
//     );

//     for tag in generator.generate_tags(source_code)? {
//         println!("{tag:#?}");
//     }

//     Ok(())
// }
