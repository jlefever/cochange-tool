#![feature(iterator_try_collect)]

#[macro_use]
extern crate derive_builder;
extern crate derive_new;

mod db;
mod extraction;
mod gtl;
mod ir;
mod parsing;

use std::path::PathBuf;
use std::time::Instant;

use ::time::format_description::well_known::Iso8601;
use ::time::Date;
use ::time::OffsetDateTime;
use ::time::PrimitiveDateTime;
use ::time::Time;
use anyhow::Context;
use clap::App;
use clap::CommandFactory;
use clap_verbosity_flag::InfoLevel;
use clap_verbosity_flag::Verbosity;
use git2::Reference;
use git2::Repository;
use git2::Sort;
use parsing::FileParser;
use rusqlite::Connection;
use tree_sitter::Language;

use crate::db::insert_change;
use crate::db::insert_presence;
use crate::db::insert_ref;
use crate::db::VirtualDb;
use crate::extraction::diff_all_files;
use crate::extraction::get_changes;
use crate::extraction::get_presences;
use crate::extraction::CommitWalk;
use crate::extraction::ExtractionCtx;
use crate::extraction::RefGlobKind;
use crate::ir::*;

/// Iterates through the commit history of a git repository and stores the
/// (co-)change information of each semantic entity encountered. A "semantic
/// entity" is any function, method, class, etc. found inside of a source file.
/// This command iterates through all commits reachable from the given starting
/// commits, skipping those that are excluded by the commit limiting options.
///
/// This information is stored in a SQLite database file. This database file may
/// be re-used by other subcommands of this program.
///
/// The commit selection options are intentionally very similiar to those
/// provided by git-log and git-rev-list, but with a few notable omissions:
///
/// - Arbitrary commit hashs are not accepted as input. Starting commits must be
///   provided as named references ([REFS]).
///
/// - Parent rewriting is not supported. Each commit is diffed with its real
///   parent to determine the (co-)changes of that commit.
///
/// - Set subtraction (i.e. `foo ^bar` or `foo..bar`) is not supported.
#[derive(Debug, clap::Parser)]
#[clap(version, author)]
struct Cli {
    #[clap(flatten)]
    verbose: Verbosity<InfoLevel>,

    /// Starting commits given as named references (e.g. HEAD, branchname, etc.)
    #[clap()]
    refs: Vec<String>,

    /// Use the given path to a git repository instead of the current directory.
    #[clap(help_heading = "I/O", long, short = 'C')]
    repo: Option<PathBuf>,

    /// Path to the database of co-change data.
    #[clap(help_heading = "I/O", long)]
    db: PathBuf,

    /// Limit the number of commits to process (i.e. extract (co-)change
    /// information from).
    ///
    /// This is affected by the order of the commits. Commits are sorted in
    /// reverse chronological order.
    #[clap(
        help_heading = "COMMIT LIMITING",
        display_order = 2,
        long,
        short = 'n',
        value_name = "NUMBER"
    )]
    max_count: Option<usize>,

    /// Only process commits created after a specific date.
    ///
    /// Expected to be ISO 8601. Time portion is optional. Timezone defaults to
    /// UTC.
    ///
    /// May also be a duration. For instance, 1year 6months.
    #[clap(help_heading = "COMMIT LIMITING", display_order = 3, long, value_name = "DATE")]
    since: Option<String>,

    /// Only process commits created before a specific date.
    ///
    /// Expected to be ISO 8601. Time portion is optional. Timezone defaults to
    /// UTC.
    ///
    /// May also be a duration. For instance, 1year 6months.
    #[clap(help_heading = "COMMIT LIMITING", display_order = 4, long, value_name = "DATE")]
    until: Option<String>,

    /// Pretend as if all the refs in `refs/`, along with `HEAD`, are listed on
    /// the command line as [REFS].
    #[clap(help_heading = "COMMIT LIMITING", display_order = 7, long, action)]
    all: bool,

    /// Pretend as if all the refs in `refs/heads` are listed on the command
    /// line as [REFS].
    ///
    /// If <PATTERN> is given, limit branches to ones matching given shell glob.
    /// If pattern lacks ?, *, or [, /* at the end is implied.
    #[clap(help_heading = "COMMIT LIMITING", display_order = 8, long, value_name = "PATTERN")]
    branches: Option<Option<String>>,

    /// Pretend as if all the refs in `refs/tags` are listed on the command line
    /// as [REFS].
    ///
    /// If <PATTERN> is given, limit tags to ones matching given shell glob. If
    /// pattern lacks ?, *, or [, /* at the end is implied.
    #[clap(help_heading = "COMMIT LIMITING", display_order = 9, long, value_name = "PATTERN")]
    tags: Option<Option<String>>,

    /// Pretend as if all the refs in `refs/remotes` are listed on the command
    /// line as [REFS].
    ///
    /// If <PATTERN> is given, limit remote-tracking branches to ones matching
    /// given shell glob. If pattern lacks ?, *, or [, /* at the end is implied.
    #[clap(help_heading = "COMMIT LIMITING", display_order = 10, long, value_name = "PATTERN")]
    remotes: Option<Option<String>>,

    /// Pretend as if all the refs matching shell glob <GLOB_PATTERN> are listed
    /// on the command line as [REFS].
    ///
    /// Leading `refs/`, is automatically prepended if missing. If pattern lacks
    /// ?, *, or [, /* at the end is implied.
    #[clap(
        help_heading = "COMMIT LIMITING",
        display_order = 11,
        long,
        value_name = "GLOB_PATTERN"
    )]
    glob: Option<String>,
    // /// Only commits modifying the given <PATHS> are selected.
    // #[clap(help_heading = "COMMIT LIMITING", display_order = 12, long)]
    // paths: Vec<String>,
}

fn parse_time_input<S: AsRef<str>>(text: S) -> Option<OffsetDateTime> {
    // First, try to parse it as a date and time
    if let Ok(datetime) = OffsetDateTime::parse(text.as_ref(), &Iso8601::PARSING) {
        return Some(datetime);
    }

    // If that doesn't work, try parsing it as just a date
    if let Ok(date) = Date::parse(text.as_ref(), &Iso8601::PARSING) {
        return Some(PrimitiveDateTime::new(date, Time::MIDNIGHT).assume_utc());
    }

    // Finally, try to prase it as a duration and subtract
    if let Ok(duration) = humantime::parse_duration(text.as_ref()) {
        return Some(OffsetDateTime::now_utc() - duration);
    }

    None
}

fn validate_time_input<S: AsRef<str>>(
    app: &mut App,
    input: S,
    argument: &'static str,
) -> OffsetDateTime {
    match parse_time_input(&input) {
        Some(datetime) => datetime,
        None => {
            let msg = format!(
                "The value ('{}') supplied to '{}' is not an ISO 8601 date or a duration.",
                input.as_ref(),
                &argument
            );
            app.error(clap::ErrorKind::ValueValidation, msg).exit();
        }
    }
}

fn validate_ref_input<'r, S: AsRef<str>>(
    app: &mut App,
    repo: &'r Repository,
    input: S,
) -> Reference<'r> {
    match repo.resolve_reference_from_short_name(input.as_ref()) {
        Ok(reference) => reference,
        Err(_) => {
            let msg =
                format!("The given ref ('{}') was not found in this repository", input.as_ref());
            app.error(clap::ErrorKind::ValueValidation, msg).exit();
        }
    }
}

fn get_lead_refs(cmd: &mut App, cli: &Cli, repo: &Repository) -> anyhow::Result<Vec<Ref>> {
    if cli.all {
        return Ok(repo.references()?.map(|r| gtl::to_ref(&r.unwrap())).try_collect::<Vec<_>>()?);
    }

    let mut lead_refs = Vec::new();

    for ref_name in &cli.refs {
        lead_refs.push(gtl::to_ref(&validate_ref_input(cmd, &repo, ref_name))?);
    }

    Ok(lead_refs)
}

fn get_commit_walk(cmd: &mut App, cli: &Cli, repo: &Repository) -> anyhow::Result<CommitWalk> {
    let mut walk = CommitWalk::new();
    let since = cli.since.as_ref().map(|s| validate_time_input(cmd, s, "--since"));
    let until = cli.until.as_ref().map(|s| validate_time_input(cmd, s, "--until"));
    since.map(|s| walk.set_since(s));
    until.map(|u| walk.set_until(u));
    cli.max_count.map(|n| walk.set_max_count(n));

    walk.set_sort(Sort::TIME);

    if cli.all {
        walk.push_glob(RefGlobKind::All, None);
        return Ok(walk);
    }

    cli.branches.as_ref().map(|g| walk.push_glob(RefGlobKind::Branches, g.clone()));
    cli.tags.as_ref().map(|g| walk.push_glob(RefGlobKind::Tags, g.clone()));
    cli.remotes.as_ref().map(|g| walk.push_glob(RefGlobKind::Remotes, g.clone()));

    for ref_name in &cli.refs {
        let r#ref = validate_ref_input(cmd, &repo, ref_name);
        walk.push_start_oid(r#ref.peel_to_commit()?.id());
    }

    Ok(walk)
}

extern "C" {
    fn tree_sitter_java() -> Language;
}

fn main() -> anyhow::Result<()> {
    let mut cmd = Cli::command();
    let cli = <Cli as clap::Parser>::parse();
    env_logger::Builder::new().filter_level(cli.verbose.log_level_filter()).init();

    // Open repository
    let repo_path = cli.repo.clone().unwrap_or(PathBuf::from("."));
    let repo = Repository::discover(repo_path)
        .context("failed to find git repository at or above the provided directory")?;

    // This is a necessary config for Windows. Even though we never touch the actual
    // filesystem, because libgit2 emulates the behavior of the real git, it will
    // still crash on Windows when encountering especially long paths.
    repo.config()?.set_bool("core.longpaths", true)?;

    // Setup tree sitter
    let language = unsafe { tree_sitter_java() };
    let java_query = include_str!("../queries/java/tags.scm");
    let parsing_ctx = FileParser::new(language, java_query)?;
    let mut cache = ExtractionCtx::new(&repo, parsing_ctx);

    // Initial collection of commits into HashMap
    // We walk in reverse chronological order. This is to ensure the "-n" flag works
    // as expected. For instance, "-n 50" should fetch the 50 most recent commits.
    let walk = get_commit_walk(&mut cmd, &cli, &repo)?;
    let start = Instant::now();
    let commits = walk.walk(&repo)?.try_collect::<Vec<_>>()?;
    log::info!("Found {} commits in {}ms.", commits.len(), start.elapsed().as_millis());

    // Collect changed files
    let start = Instant::now();
    let diffed_files = diff_all_files(&repo, &commits, ".java")?;
    log::info!("Found {} changed files in {}ms", diffed_files.len(), start.elapsed().as_millis());

    // Calculate changes
    let start = Instant::now();
    let changes = diffed_files
        .iter()
        .flat_map(|diffed_file| get_changes(&mut cache, diffed_file).unwrap())
        .collect::<Vec<_>>();
    log::info!("Generated changes in {}ms", start.elapsed().as_millis());

    // Calculate presence
    let lead_refs = get_lead_refs(&mut cmd, &cli, &repo)?;
    let start = Instant::now();
    let presences = lead_refs
        .iter()
        .flat_map(|r| get_presences(&mut cache, &r.commit, ".java").unwrap())
        .collect::<Vec<_>>();
    log::info!("Generated presences in {}ms", start.elapsed().as_millis());

    // Create and insert into virtual database
    let mut db = VirtualDb::new();
    let start = Instant::now();

    for change in &changes {
        insert_change(&mut db, change)?;
    }

    for presence in &presences {
        insert_presence(&mut db, presence)?;
    }

    for r#ref in &lead_refs {
        insert_ref(&mut db, r#ref)?;
    }

    log::info!("Populated virtual database in {}ms", start.elapsed().as_millis());

    // Write virtual database to real (on disk) database
    let start = Instant::now();
    let mut conn = Connection::open(cli.db)?;
    let tx = conn.transaction()?;
    db.write(&tx)?;
    tx.commit()?;
    log::info!("Wrote virtual database to disk in {}ms", start.elapsed().as_millis());

    Ok(())
}
