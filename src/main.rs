#![feature(iterator_try_collect)]

#[macro_use]
extern crate derive_builder;

mod tagging;

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::Context;
use bitflags::bitflags;
use clap::{App, CommandFactory};
use clap_verbosity_flag::{InfoLevel, Verbosity};
use git2::{Commit, Oid, Reference, Repository};
use time::format_description::well_known::Iso8601;
use time::{Date, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

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

bitflags! {
    struct CommitInfo: u8 {
        const CHANGES = 0b00000001;
        const PRESENCE = 0b00000010;
        const REACHABILITY = 0b00000100;
        const ALL = Self::CHANGES.bits | Self::PRESENCE.bits | Self::REACHABILITY.bits;
    }
}

fn time_of(commit: &Commit) -> anyhow::Result<OffsetDateTime> {
    let commit_time = commit.time();
    let datetime = OffsetDateTime::from_unix_timestamp(commit_time.seconds())?;
    let offset = UtcOffset::from_whole_seconds(commit_time.offset_minutes() * 60)?;
    Ok(datetime.replace_offset(offset))
}

#[derive(Debug, Hash, PartialEq, Eq)]
enum RefGlobKind {
    All,
    Branches,
    Tags,
    Remotes,
}

struct CommitWalker {
    max_count: Option<usize>,
    since: Option<OffsetDateTime>,
    until: Option<OffsetDateTime>,
    globs: Vec<String>,
    start_oids: HashSet<Oid>,
}

impl CommitWalker {
    fn new() -> Self {
        Self {
            max_count: None,
            since: None,
            until: None,
            globs: Vec::new(),
            start_oids: HashSet::new(),
        }
    }

    fn set_max_count(&mut self, max_count: usize) {
        self.max_count = Some(max_count);
    }

    fn set_since(&mut self, since: OffsetDateTime) {
        self.since = Some(since);
    }

    fn set_until(&mut self, until: OffsetDateTime) {
        self.until = Some(until);
    }

    fn push_glob(&mut self, kind: RefGlobKind, glob: Option<String>) {
        let glob = glob.unwrap_or("*".to_string());

        self.globs.push(match kind {
            RefGlobKind::All => glob,
            RefGlobKind::Branches => format!("heads/{}", glob),
            RefGlobKind::Tags => format!("tags/{}", glob),
            RefGlobKind::Remotes => format!("remotes/{}", glob),
        });
    }

    fn push_start_oid(&mut self, oid: Oid) {
        self.start_oids.insert(oid);
    }

    fn walk<'r>(&self, repo: &'r Repository) -> anyhow::Result<Vec<Commit<'r>>> {
        let mut revwalk = repo.revwalk()?;
        revwalk.set_sorting(git2::Sort::TIME)?;
        self.globs.iter().try_for_each(|g| revwalk.push_glob(g))?;
        self.start_oids.iter().try_for_each(|&oid| revwalk.push(oid))?;

        let mut commits = Vec::new();

        for oid in revwalk {
            let commit = repo.find_commit(oid?)?;
            let commit_time = time_of(&commit)?;

            let is_valid_by_since = self.since.map(|t| commit_time >= t).unwrap_or(true);
            let is_valid_by_until = self.until.map(|t| commit_time <= t).unwrap_or(true);
            let is_valid_by_n = self.max_count.map(|n| commits.len() < n).unwrap_or(true);

            if !is_valid_by_since || !is_valid_by_n {
                break;
            }

            if !is_valid_by_until {
                continue;
            }

            commits.push(commit);
        }

        Ok(commits)
    }
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

fn validate_time_input(app: &mut App, input: String, argument: &'static str) -> OffsetDateTime {
    match parse_time_input(&input) {
        Some(datetime) => datetime,
        None => {
            let msg = format!(
                "The value ('{}') supplied to '{}' is not an ISO 8601 date or a duration.",
                &input, &argument
            );
            app.error(clap::ErrorKind::ValueValidation, msg).exit();
        }
    }
}

fn validate_ref_input<'r>(app: &mut App, repo: &'r Repository, input: String) -> Reference<'r> {
    match repo.resolve_reference_from_short_name(&input) {
        Ok(reference) => reference,
        Err(_) => {
            let msg = format!("The given ref ('{}') was not found in this repository", input);
            app.error(clap::ErrorKind::ValueValidation, msg).exit();
        }
    }
}

fn main() -> anyhow::Result<()> {
    let cli = <Cli as clap::Parser>::parse();
    env_logger::Builder::new().filter_level(cli.verbose.log_level_filter()).init();

    let mut cmd = Cli::command();
    let since = cli.since.map(|s| validate_time_input(&mut cmd, s, "--since"));
    let until = cli.until.map(|s| validate_time_input(&mut cmd, s, "--until"));

    let mut walker = CommitWalker::new();
    since.map(|s| walker.set_since(s));
    until.map(|u| walker.set_until(u));
    cli.max_count.map(|n| walker.set_max_count(n));

    if cli.all {
        walker.push_glob(RefGlobKind::All, None);
    }

    cli.branches.map(|g| walker.push_glob(RefGlobKind::Branches, g));
    cli.tags.map(|g| walker.push_glob(RefGlobKind::Tags, g));
    cli.remotes.map(|g| walker.push_glob(RefGlobKind::Remotes, g));

    let repo = Repository::discover(cli.repo.unwrap_or(PathBuf::from(".")))
        .context("failed to find git repository in the provided directory")?;

    for r#ref in cli.refs {
        let r#ref = validate_ref_input(&mut cmd, &repo, r#ref);
        walker.push_start_oid(r#ref.peel_to_commit()?.id());
    }

    let start = Instant::now();
    let commits = walker.walk(&repo)?;
    log::info!("Found {} commits in {} ms.", commits.len(), start.elapsed().as_millis());

    Ok(())
}
