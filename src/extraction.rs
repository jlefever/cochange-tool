use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use anyhow::bail;
use anyhow::Context;
use anyhow::Result;
use git2::Oid;
use time::OffsetDateTime;

use crate::gtl;
use crate::ir;
use crate::parsing::FileParser;

// Be explicit about whether an identifier is from the git2 namespace or ir
// namespace.

#[derive(Debug, Hash, PartialEq, Eq)]
pub enum RefGlobKind {
    All,
    Branches,
    Tags,
    Remotes,
}

#[derive(Debug, Clone)]
pub struct CommitWalk {
    sort_mode: git2::Sort,
    max_count: Option<usize>,
    since: Option<OffsetDateTime>,
    until: Option<OffsetDateTime>,
    globs: Vec<String>,
    start_oids: HashSet<Oid>,
}

impl CommitWalk {
    pub fn new() -> Self {
        Self {
            sort_mode: git2::Sort::NONE,
            max_count: None,
            since: None,
            until: None,
            globs: Vec::new(),
            start_oids: HashSet::new(),
        }
    }

    pub fn set_sort(&mut self, sort_mode: git2::Sort) {
        self.sort_mode = sort_mode;
    }

    pub fn set_max_count(&mut self, max_count: usize) {
        self.max_count = Some(max_count);
    }

    pub fn set_since(&mut self, since: OffsetDateTime) {
        self.since = Some(since);
    }

    pub fn set_until(&mut self, until: OffsetDateTime) {
        self.until = Some(until);
    }

    pub fn push_glob(&mut self, kind: RefGlobKind, glob: Option<String>) {
        let glob = glob.unwrap_or("*".to_string());

        self.globs.push(match kind {
            RefGlobKind::All => glob,
            RefGlobKind::Branches => format!("heads/{}", glob),
            RefGlobKind::Tags => format!("tags/{}", glob),
            RefGlobKind::Remotes => format!("remotes/{}", glob),
        });
    }

    pub fn push_start_oid(&mut self, oid: Oid) {
        self.start_oids.insert(oid);
    }

    pub fn revwalk<'r>(&self, repo: &'r git2::Repository) -> Result<git2::Revwalk<'r>> {
        let mut revwalk = repo.revwalk()?;
        revwalk.set_sorting(self.sort_mode)?;
        self.globs.iter().try_for_each(|g| revwalk.push_glob(g))?;
        self.start_oids.iter().try_for_each(|&oid| revwalk.push(oid))?;
        Ok(revwalk)
    }

    pub fn walk<'r>(self, repo: &'r git2::Repository) -> Result<CommitWalkIterator<'r>> {
        let revwalk = self.revwalk(repo)?;
        Ok(CommitWalkIterator::new(self, repo, revwalk))
    }
}

pub struct CommitWalkIterator<'r> {
    walk: CommitWalk,
    repo: &'r git2::Repository,
    revwalk: git2::Revwalk<'r>,
    count: usize,
}

impl<'r> CommitWalkIterator<'r> {
    fn new(walk: CommitWalk, repo: &'r git2::Repository, revwalk: git2::Revwalk<'r>) -> Self {
        Self { walk, repo, revwalk, count: 0 }
    }
}

impl<'r> Iterator for CommitWalkIterator<'r> {
    type Item = Result<git2::Commit<'r>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let oid_res = self.revwalk.next()?;
            let commit_res = oid_res.and_then(|oid| self.repo.find_commit(oid));

            if let Err(err) = commit_res {
                return Some(Err(anyhow::Error::new(err)));
            }

            let commit = commit_res.unwrap();
            let commit_datetime_res = gtl::to_datetime(&commit.time());

            if let Err(err) = commit_datetime_res {
                return Some(Err(err));
            }

            let commit_time = commit_datetime_res.unwrap();

            let is_valid_by_since = self.walk.since.map(|t| commit_time >= t).unwrap_or(true);
            let is_valid_by_until = self.walk.until.map(|t| commit_time <= t).unwrap_or(true);
            let is_valid_by_n = self.walk.max_count.map(|n| self.count < n).unwrap_or(true);

            if !is_valid_by_since || !is_valid_by_n {
                break;
            }

            if !is_valid_by_until {
                continue;
            }

            self.count += 1;
            return Some(Ok(commit));
        }

        return None;
    }
}

pub struct ExtractionCtx<'r> {
    repo: &'r git2::Repository,
    parser: FileParser,
    cache: HashMap<(String, Oid), Vec<ir::LocEntity>>,
}

impl<'r> ExtractionCtx<'r> {
    pub fn new(repo: &'r git2::Repository, parsing_ctx: FileParser) -> Self {
        Self { repo, parser: parsing_ctx, cache: HashMap::new() }
    }

    fn get_entities(&mut self, filename: &String, blob: Oid) -> &Vec<ir::LocEntity> {
        self.cache.entry((filename.clone(), blob)).or_insert_with(|| {
            if blob.is_zero() {
                return Vec::new();
            }

            let blob = self.repo.find_blob(blob).unwrap();
            self.parser.parse(blob.content(), filename).unwrap()
        })
    }
}

impl TryFrom<git2::DiffHunk<'_>> for ir::Hunk {
    type Error = anyhow::Error;

    fn try_from(diff_hunk: git2::DiffHunk<'_>) -> Result<Self, Self::Error> {
        let old_start: usize = diff_hunk.old_start().try_into()?;
        let new_start: usize = diff_hunk.new_start().try_into()?;
        let old_lines: usize = diff_hunk.old_lines().try_into()?;
        let new_lines: usize = diff_hunk.new_lines().try_into()?;

        Ok(ir::Hunk::new(
            ir::Interval(old_start, old_start + old_lines),
            ir::Interval(new_start, new_start + new_lines),
        ))
    }
}

fn get_diff_delta_path(diff_delta: &git2::DiffDelta) -> Result<String> {
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

pub fn get_changes(ctx: &mut ExtractionCtx, df: &ir::DiffedFile) -> Result<Vec<ir::Change>> {
    // Tree-sitter uses zero-based indices for rows and colums
    // What about git-diff? I think its one-based.
    // TODO: Confirm that git-diff is one-based.
    // TODO: Check the inclusivity/exclusivity of the endpoints
    let mut changes: HashMap<Arc<ir::Entity>, ir::ChangeBuilder> = HashMap::new();

    let filename = &df.filename;
    let old_file = df.old_file;
    let new_file = df.new_file;

    for old_entity in ctx.get_entities(filename, old_file) {
        let dels = df.hunks.iter().map(|h| h.old_interval.intersect(&old_entity.loc)).sum();

        if dels > 0 {
            changes.entry(old_entity.entity.clone()).or_default().dels(dels);
        }
    }

    for new_entity in ctx.get_entities(filename, new_file) {
        let adds = df.hunks.iter().map(|h| h.new_interval.intersect(&new_entity.loc)).sum();

        if adds > 0 {
            changes.entry(new_entity.entity.clone()).or_default().adds(adds);
        }
    }

    let old_entities = ctx
        .get_entities(filename, old_file)
        .iter()
        .map(|t| t.entity.clone())
        .collect::<HashSet<_>>();
    let new_entities = ctx
        .get_entities(filename, new_file)
        .iter()
        .map(|t| t.entity.clone())
        .collect::<HashSet<_>>();

    for deleted in old_entities.difference(&new_entities) {
        changes.entry(deleted.clone()).or_default().kind(ir::ChangeKind::Deleted);
    }

    for created in new_entities.difference(&old_entities) {
        changes.entry(created.clone()).or_default().kind(ir::ChangeKind::Added);
    }

    Ok(changes
        .into_iter()
        .map(|(e, mut change)| change.entity(e).commit(df.commit.clone()).build())
        .try_collect()?)
}

pub fn get_presences(
    ctx: &mut ExtractionCtx,
    commit: &ir::Commit,
    suffix: &'static str,
) -> Result<Vec<ir::Presence>> {
    let mut blobs = Vec::new();

    let tree = ctx.repo.find_commit(commit.sha1)?.tree()?;

    tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
        if !matches!(entry.kind().unwrap(), git2::ObjectType::Blob) {
            return git2::TreeWalkResult::Ok;
        }

        let filename = format!("{}{}", dir, entry.name().unwrap());

        if !filename.ends_with(suffix) {
            return git2::TreeWalkResult::Ok;
        }

        blobs.push((filename, entry.id()));
        git2::TreeWalkResult::Ok
    })?;

    let mut presences = Vec::new();

    for (filename, blob) in &blobs {
        for loc_entity in ctx.get_entities(&filename, blob.clone()) {
            presences.push(ir::Presence::new(loc_entity.clone(), commit.clone()));
        }
    }

    Ok(presences)
}

pub fn diff_all_files(
    repo: &git2::Repository,
    commits: &Vec<git2::Commit>,
    suffix: &'static str,
) -> Result<Vec<ir::DiffedFile>> {
    let mut diffed_files: HashMap<(String, Oid), ir::DiffedFile> = HashMap::new();

    let mut opts = git2::DiffOptions::new();
    opts.ignore_filemode(true);
    opts.ignore_whitespace(false);
    opts.ignore_whitespace_change(false);
    opts.ignore_whitespace_eol(false);
    opts.ignore_blank_lines(false);
    opts.indent_heuristic(false);
    opts.context_lines(0);

    for commit in commits {
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
                    git2::Delta::Added => true,
                    git2::Delta::Deleted => true,
                    git2::Delta::Modified => true,
                    _ => false,
                };

                if !is_supported_status {
                    log::warn!("Skipping unsupported diff status: {:?}", &delta.status());
                    return true;
                }

                let filename = get_diff_delta_path(&delta)
                    .expect("failed to get the path of the changed file");

                if !filename.to_lowercase().ends_with(suffix) {
                    return true;
                }

                let diffed_file =
                    diffed_files.entry((filename.clone(), commit.id())).or_insert_with(|| {
                        gtl::to_diffed_file(filename.clone(), commit, &delta)
                            .expect("failed to create a diffed file")
                    });

                diffed_file.hunks.push(hunk.try_into().expect("failed to convert hunk"));
                true
            }),
            None,
        )
        .context("failed to iterate over diff")?;
    }

    Ok(diffed_files.into_values().collect::<Vec<_>>())
}
