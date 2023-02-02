use std::collections::HashSet;

use git2::{Commit, Oid, Repository, Revwalk, Sort};
use time::{OffsetDateTime, UtcOffset};

use crate::time::to_datetime;

#[derive(Debug, Clone)]
pub struct CommitWalk {
    sort_mode: Sort,
    max_count: Option<usize>,
    since: Option<OffsetDateTime>,
    until: Option<OffsetDateTime>,
    globs: Vec<String>,
    start_oids: HashSet<Oid>,
}

impl CommitWalk {
    pub fn new() -> Self {
        Self {
            sort_mode: Sort::NONE,
            max_count: None,
            since: None,
            until: None,
            globs: Vec::new(),
            start_oids: HashSet::new(),
        }
    }

    pub fn set_sort(&mut self, sort_mode: Sort) {
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

    pub fn revwalk<'r>(&self, repo: &'r Repository) -> anyhow::Result<Revwalk<'r>> {
        let mut revwalk = repo.revwalk()?;
        revwalk.set_sorting(self.sort_mode)?;
        self.globs.iter().try_for_each(|g| revwalk.push_glob(g))?;
        self.start_oids.iter().try_for_each(|&oid| revwalk.push(oid))?;
        Ok(revwalk)
    }

    pub fn walk<'r>(self, repo: &'r Repository) -> anyhow::Result<CommitWalkIterator<'r>> {
        let revwalk = self.revwalk(repo)?;
        Ok(CommitWalkIterator::new(self, repo, revwalk))
    }
}

#[derive(Debug, Hash, PartialEq, Eq)]
pub enum RefGlobKind {
    All,
    Branches,
    Tags,
    Remotes,
}

pub struct CommitWalkIterator<'r> {
    walk: CommitWalk,
    repo: &'r Repository,
    revwalk: Revwalk<'r>,
    count: usize,
}

impl<'r> CommitWalkIterator<'r> {
    fn new(walk: CommitWalk, repo: &'r Repository, revwalk: Revwalk<'r>) -> Self {
        Self { walk, repo, revwalk, count: 0 }
    }
}

impl<'r> Iterator for CommitWalkIterator<'r> {
    type Item = anyhow::Result<Commit<'r>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let oid_res = self.revwalk.next()?;
            let commit_res = oid_res.and_then(|oid| self.repo.find_commit(oid));

            if let Err(err) = commit_res {
                return Some(Err(anyhow::Error::new(err)));
            }

            let commit = commit_res.unwrap();
            let commit_datetime_res = to_datetime(&commit.time());

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

            return Some(Ok(commit));
        }

        return None;
    }
}
