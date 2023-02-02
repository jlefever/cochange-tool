#![feature(iterator_try_collect)]

#[macro_use]
extern crate derive_builder;

mod ir;
mod persist;
mod tagging;
mod time;
mod walking;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use ::time::format_description::well_known::Iso8601;
use ::time::{Date, OffsetDateTime, PrimitiveDateTime, Time};
use anyhow::{bail, Context};
use clap::{App, CommandFactory};
use clap_verbosity_flag::{InfoLevel, Verbosity};
use git2::{
    Delta, DiffDelta, DiffHunk, DiffOptions, Oid, Reference, Repository, Sort, Tree, TreeWalkMode,
    TreeWalkResult,
};
use persist::{ChangeExtra, ChangeKey, ChangeVirtualTable, CommitKey, Id};
use rusqlite::{params, Connection};
use tagging::TagGenerator;
use tree_sitter::Language;

use crate::ir::*;
use crate::persist::{
    insert_commit, insert_tag, ChangeWriter, CommitVirtualTable, CommitWriter, EntityVirtualTable,
    EntityWriter,
};
use crate::walking::{CommitWalk, RefGlobKind};

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

fn validate_ref_input<'r>(app: &mut App, repo: &'r Repository, input: &String) -> Reference<'r> {
    match repo.resolve_reference_from_short_name(input) {
        Ok(reference) => reference,
        Err(_) => {
            let msg = format!("The given ref ('{}') was not found in this repository", input);
            app.error(clap::ErrorKind::ValueValidation, msg).exit();
        }
    }
}

impl TryFrom<DiffHunk<'_>> for Hunk {
    type Error = anyhow::Error;

    fn try_from(diff_hunk: DiffHunk<'_>) -> Result<Self, Self::Error> {
        // Avoid underflows
        if diff_hunk.old_start() == 0 || diff_hunk.new_start() == 0 {
            log::warn!("Found zero-based index: {:?}", diff_hunk);
            // bail!("expected one-based index");
        }

        // Convert to zero-based index
        let old_start = diff_hunk.old_start() - 1;
        let new_start = diff_hunk.new_start() - 1;

        // Convert to usize
        let old_start: usize = old_start.try_into()?;
        let new_start: usize = new_start.try_into()?;
        let old_lines: usize = diff_hunk.old_lines().try_into()?;
        let new_lines: usize = diff_hunk.new_lines().try_into()?;

        Ok(Hunk::new(
            Interval(old_start, old_start + old_lines),
            Interval(new_start, new_start + new_lines),
        ))
    }
}

fn get_diff_delta_path(diff_delta: &DiffDelta) -> anyhow::Result<String> {
    let old_path = diff_delta.old_file().path();
    let new_path = diff_delta.new_file().path();

    Ok(match (old_path, new_path) {
        (None, None) => bail!("at least one side of diff must be non-empty"),
        (None, Some(path)) => path,
        (Some(path), None) => path,
        (Some(old_path), Some(new_path)) => {
            if old_path != new_path {
                bail!("renames and moves are not supported");
            } else {
                old_path
            }
        }
    }
    .to_string_lossy()
    .to_string())
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct ChangedFile {
    filename: String,
    old_file: Oid,
    new_file: Oid,
    commit: Oid,
}

impl ChangedFile {
    fn new(filename: String, old_file: Oid, new_file: Oid, commit: Oid) -> Self {
        Self { filename, old_file, new_file, commit }
    }
}

struct ChangeGenerator<'t, 'r> {
    cache: HashMap<(String, Oid), Vec<LocalTag>>,
    tag_gen: &'t mut TagGenerator,
    repo: &'r Repository,
}

impl<'t, 'r> ChangeGenerator<'t, 'r> {
    fn new(tag_gen: &'t mut TagGenerator, repo: &'r Repository) -> Self {
        Self { cache: HashMap::new(), tag_gen, repo }
    }

    fn get_tags(&mut self, filename: &String, blob: Oid) -> &Vec<LocalTag> {
        self.cache.entry((filename.clone(), blob)).or_insert_with(|| {
            if blob.is_zero() {
                return Vec::new();
            }

            self.tag_gen
                .generate_tags(filename, self.repo.find_blob(blob).unwrap().content())
                .unwrap()
        })
    }

    fn generate_changes(&mut self, changed_file: &ChangedFile, hunks: &Vec<Hunk>) -> Vec<Change> {
        // Tree-sitter uses zero-based indices for rows and colums
        // What about git-diff? I think its one-based.
        // TODO: Confirm that git-diff is one-based.
        // TODO: Check the inclusivity/exclusivity of the endpoints
        let mut changes: HashMap<Arc<Tag>, ChangeBuilder> = HashMap::new();

        let filename = &changed_file.filename;
        let old_file = changed_file.old_file;
        let new_file = changed_file.new_file;

        for old_tag in self.get_tags(filename, old_file) {
            let dels = hunks.iter().map(|h| h.old_interval.intersect(&old_tag.interval)).sum();

            if dels > 0 {
                changes.entry(old_tag.tag.clone()).or_default().dels(dels);
            }
        }

        for new_tag in self.get_tags(filename, new_file) {
            let adds = hunks.iter().map(|h| h.new_interval.intersect(&new_tag.interval)).sum();

            if adds > 0 {
                changes.entry(new_tag.tag.clone()).or_default().adds(adds);
            }
        }

        let old_tags =
            self.get_tags(filename, old_file).iter().map(|t| t.tag.clone()).collect::<HashSet<_>>();
        let new_tags =
            self.get_tags(filename, new_file).iter().map(|t| t.tag.clone()).collect::<HashSet<_>>();

        for deleted in old_tags.difference(&new_tags) {
            changes.entry(deleted.clone()).or_default().kind(ChangeKind::Deleted);
        }

        for created in new_tags.difference(&old_tags) {
            changes.entry(created.clone()).or_default().kind(ChangeKind::Added);
        }

        changes
            .into_iter()
            .map(|(tag, mut change)| change.tag(tag).commit(changed_file.commit).build().unwrap())
            .collect()
    }

    fn generate_presence(&mut self, tree: &Tree) -> anyhow::Result<Vec<LocalTag>> {
        let mut blobs = Vec::new();

        tree.walk(TreeWalkMode::PreOrder, |dir, entry| {
            if !matches!(entry.kind().unwrap(), git2::ObjectType::Blob) {
                return TreeWalkResult::Ok;
            }

            let filename = format!("{}{}", dir, entry.name().unwrap());

            if !filename.ends_with(".java") {
                return TreeWalkResult::Ok;
            }

            let blob = entry.to_object(self.repo).unwrap().id();
            blobs.push((filename, blob));
            TreeWalkResult::Ok
        })?;

        let mut local_tags = Vec::new();

        for (filename, blob) in &blobs {
            for local_tag in self.get_tags(&filename, blob.clone()) {
                local_tags.push(local_tag.clone());
            }
        }

        Ok(local_tags)
    }
}

fn main() -> anyhow::Result<()> {
    let cli = <Cli as clap::Parser>::parse();
    env_logger::Builder::new().filter_level(cli.verbose.log_level_filter()).init();

    let mut cmd = Cli::command();

    // Create CommitWalk from cli input
    let mut walk = CommitWalk::new();
    let since = cli.since.map(|s| validate_time_input(&mut cmd, s, "--since"));
    let until = cli.until.map(|s| validate_time_input(&mut cmd, s, "--until"));
    since.map(|s| walk.set_since(s));
    until.map(|u| walk.set_until(u));
    cli.max_count.map(|n| walk.set_max_count(n));

    if cli.all {
        walk.push_glob(RefGlobKind::All, None);
    }

    cli.branches.map(|g| walk.push_glob(RefGlobKind::Branches, g));
    cli.tags.map(|g| walk.push_glob(RefGlobKind::Tags, g));
    cli.remotes.map(|g| walk.push_glob(RefGlobKind::Remotes, g));

    let repo = Repository::discover(cli.repo.unwrap_or(PathBuf::from(".")))
        .context("failed to find git repository at or above the provided directory")?;

    // TODO: Add support for --all flag
    let mut lead_refs = Vec::new();

    for ref_name in &cli.refs {
        lead_refs.push(validate_ref_input(&mut cmd, &repo, ref_name));
        let r#ref = validate_ref_input(&mut cmd, &repo, ref_name);
        walk.push_start_oid(r#ref.peel_to_commit()?.id());
    }

    // This is a necessary config for Windows. Even though we never touch the actual
    // filesystem, because libgit2 emulates the behavior of the real git, it will
    // still crash on Windows when encountering especially long paths.
    repo.config()?.set_bool("core.longpaths", true)?;

    // Initial collection of commits into HashMap
    // We walk in reverse chronological order. This is to ensure the "-n" flag works
    // as expected. For instance, "-n 50" should fetch the 50 most recent commits.
    let start = Instant::now();
    walk.set_sort(Sort::TIME);
    let commits = walk.clone().walk(&repo)?;
    let commits = commits.map(|res| res.map(|c| (c.id(), c))).try_collect::<HashMap<_, _>>()?;
    log::info!("Found {} commits in {}ms.", commits.len(), start.elapsed().as_millis());

    // Re-order commits topologically.
    // Not sure if this is necessary anymore.
    let start = Instant::now();
    walk.set_sort(Sort::TOPOLOGICAL | Sort::REVERSE);
    let revwalk = walk.revwalk(&repo)?;
    let commits = revwalk.filter_map(|res| commits.get(&res.unwrap())).collect::<Vec<_>>();
    log::info!("Re-ordered {} commits in {}ms.", commits.len(), start.elapsed().as_millis());

    // Diff Experiment
    let mut opts = DiffOptions::new();
    opts.ignore_filemode(true);
    opts.ignore_whitespace(false);
    opts.ignore_whitespace_change(false);
    opts.ignore_whitespace_eol(false);
    opts.ignore_blank_lines(false);
    opts.indent_heuristic(false);
    opts.context_lines(0);

    let start = Instant::now();
    let mut hunks: HashMap<ChangedFile, Vec<Hunk>> = HashMap::new();

    for commit in &commits {
        let parents = commit.parents().collect::<Vec<_>>();
        let new_tree = commit.tree()?;

        let diff = match parents.len() {
            0 => repo.diff_tree_to_tree(None, Some(&new_tree), Some(&mut opts)),
            1 => {
                let parent = parents.get(0).unwrap();
                let old_tree = parent.tree()?;
                repo.diff_tree_to_tree(Some(&old_tree), Some(&new_tree), Some(&mut opts))
            }
            _ => continue,
        }?;

        diff.foreach(
            &mut |_, _| true,
            None,
            Some(&mut |delta, hunk| {
                let is_supported_status = match delta.status() {
                    Delta::Added => true,
                    Delta::Deleted => true,
                    Delta::Modified => true,
                    _ => false,
                };

                if !is_supported_status {
                    log::warn!("Skipping unsupported diff status: {:?}", &delta.status());
                    return true;
                }

                let filename = get_diff_delta_path(&delta)
                    .expect("failed to get the path of the changed file");

                if !filename.to_lowercase().ends_with(".java") {
                    return true;
                }

                let old_file = delta.old_file().id();
                let new_file = delta.new_file().id();

                let changed_file = ChangedFile::new(filename, old_file, new_file, commit.id());

                hunks
                    .entry(changed_file)
                    .or_default()
                    .push(hunk.try_into().expect("failed to convert hunk"));

                return true;
            }),
            None,
        )
        .context("failed to iterate over diff")?;
    }

    log::info!("Found {} changed files in {}ms", hunks.len(), start.elapsed().as_millis());

    extern "C" {
        fn tree_sitter_java() -> Language;
    }

    let language = unsafe { tree_sitter_java() };
    let java_query = include_str!("../queries/java/tags.scm");
    let mut tag_gen = TagGenerator::new(language, java_query)?;
    let mut change_gen = ChangeGenerator::new(&mut tag_gen, &repo);

    let start = Instant::now();

    let mut changes = Vec::new();

    for (changed_file, hunks) in &hunks {
        for change in change_gen.generate_changes(changed_file, hunks) {
            println!("{:?}", change);
            changes.push(change);
        }

        println!()
    }

    log::info!("Generated changes in {}ms", start.elapsed().as_millis());

    // Calculate presence
    let mut local_tags = Vec::new();

    for lead_ref in &lead_refs {
        let lead_ref_name = lead_ref.name().context("expected a named ref")?;
        log::info!("Finding tags present in {}...", lead_ref_name);
        let tree = &lead_ref.peel_to_tree()?;
        for local_tag in change_gen.generate_presence(tree)? {
            local_tags.push(local_tag);
        }
    }

    let start = Instant::now();
    let mut entity_vt = EntityVirtualTable::new();

    for local_tag in local_tags {
        insert_tag(&mut entity_vt, local_tag.tag);
    }

    log::info!(
        "Inserted {} entitities into the virtual table in {}ms",
        entity_vt.len(),
        start.elapsed().as_millis()
    );

    let start = Instant::now();
    let mut commit_vt = CommitVirtualTable::new();

    for commit in &commits {
        insert_commit(&mut commit_vt, commit);
    }

    log::info!(
        "Inserted {} commits into the virtual table in {}ms",
        commit_vt.len(),
        start.elapsed().as_millis()
    );

    let start = Instant::now();
    let mut change_vt = ChangeVirtualTable::new();

    for change in &changes {
        insert_change(&mut commit_vt, &mut entity_vt, &mut change_vt, change);
    }

    log::info!(
        "Inserted {} commits into the virtual table in {}ms",
        commit_vt.len(),
        start.elapsed().as_millis()
    );

    let mut db = Connection::open(cli.db)?;

    let init_script = include_str!("../sql/init_entities.sql");
    db.execute(init_script, params![])?;
    let init_script = include_str!("../sql/init_commits.sql");
    db.execute(init_script, params![])?;
    let init_script = include_str!("../sql/init_changes.sql");
    db.execute(init_script, params![])?;

    let start = Instant::now();
    let tx = db.transaction()?;
    entity_vt.write::<EntityWriter>(&tx)?;
    commit_vt.write::<CommitWriter>(&tx)?;
    change_vt.write::<ChangeWriter>(&tx)?;
    tx.commit()?;
    log::info!("Wrote virtual tables to disk in {}ms", start.elapsed().as_millis());

    Ok(())
}

pub fn insert_change(
    commit_vt: &mut CommitVirtualTable,
    entity_vt: &mut EntityVirtualTable,
    change_vt: &mut ChangeVirtualTable,
    change: &Change,
) -> Id {
    let commit_id = commit_vt.get_id(&CommitKey::new(change.commit.to_string())).unwrap();
    let entity_id = insert_tag(entity_vt, change.tag.clone());

    let change_key = ChangeKey::new(commit_id, entity_id);
    let change_extra = ChangeExtra::new(change.kind, change.adds, change.dels);

    change_vt.insert(change_key, change_extra)
}
