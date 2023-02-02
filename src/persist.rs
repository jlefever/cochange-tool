use std::hash::Hash;
use std::{borrow::Borrow, collections::HashMap, sync::Arc};

use git2::Commit;
use rusqlite::{params, CachedStatement, Transaction};

use crate::time::to_datetime;

use crate::ir::*;

pub type Id = usize;

pub trait SqlWriter<'a, K: Hash + Eq, E> {
    fn prepare(tx: &'a Transaction) -> anyhow::Result<Self>
    where
        Self: Sized;
    fn execute(&mut self, id: Id, key: &K, extra: &E) -> anyhow::Result<usize>;
}

pub struct VirtualTable<K: Hash + Eq, E> {
    map: HashMap<K, (E, Id)>,
    next_id: Id,
}

impl<K: Hash + Eq, E> VirtualTable<K, E> {
    pub fn new() -> Self {
        Self { map: HashMap::new(), next_id: 0 }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn get_id(&self, key: &K) -> Option<Id> {
        self.map.get(key).map(|(_, id)| *id)
    }

    pub fn insert(&mut self, key: K, extra: E) -> Id {
        let (_, id) = self.map.entry(key).or_insert_with(|| {
            let id = self.next_id;
            self.next_id += 1;
            (extra, id)
        });

        *id
    }

    pub fn write<'a, W: SqlWriter<'a, K, E>>(self, tx: &'a Transaction) -> anyhow::Result<()> {
        let mut writer = W::prepare(tx)?;

        // Sorting is required for the entities table to maintain the "parent_id"
        // constraint
        let mut rows = self.map.into_iter().collect::<Vec<_>>();
        rows.sort_by_key(|(_, (_, id))| *id);

        for (key, (extra, id)) in rows {
            writer.execute(id, &key, &extra)?;
        }

        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NullExtra;

// ========================================================
// Entity -------------------------------------------------
// ========================================================

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntityKey {
    parent_id: Option<Id>,
    name: String,
    kind: Arc<String>,
}

impl EntityKey {
    pub fn new(parent_id: Option<Id>, name: String, kind: Arc<String>) -> Self {
        Self { parent_id, name, kind }
    }
}

pub type EntityVirtualTable = VirtualTable<EntityKey, NullExtra>;

pub struct EntityWriter<'a> {
    stmt: CachedStatement<'a>,
}

impl<'a> SqlWriter<'a, EntityKey, NullExtra> for EntityWriter<'a> {
    fn prepare(tx: &'a Transaction) -> anyhow::Result<Self> {
        let sql = "INSERT INTO entities (id, parent_id, name, kind) VALUES (?, ?, ?, ?);";
        Ok(Self { stmt: tx.prepare_cached(sql)? })
    }

    fn execute(&mut self, id: Id, key: &EntityKey, _: &NullExtra) -> anyhow::Result<usize> {
        Ok(self.stmt.execute(params![id, key.parent_id, key.name, key.kind])?)
    }
}

pub fn insert_tag<T: Borrow<Tag>>(vt: &mut EntityVirtualTable, tag: T) -> Id {
    let mut prev_id = None;

    for (name, kind) in tag.borrow().to_vec() {
        let key = EntityKey::new(prev_id, name, kind);
        prev_id = Some(vt.insert(key, NullExtra));
    }

    prev_id.unwrap()
}

// ========================================================
// Commit -------------------------------------------------
// ========================================================

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommitKey {
    sha1: String,
}

impl CommitKey {
    pub fn new(sha1: String) -> Self {
        Self { sha1 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommitExtra {
    is_merge: bool,
    author_time: i64,
    commit_time: i64,
    commit_info: CommitInfo,
}

impl CommitExtra {
    pub fn new(
        is_merge: bool,
        author_time: i64,
        commit_time: i64,
        commit_info: CommitInfo,
    ) -> Self {
        Self { is_merge, author_time, commit_time, commit_info }
    }
}

pub type CommitVirtualTable = VirtualTable<CommitKey, CommitExtra>;

pub struct CommitWriter<'a> {
    stmt: CachedStatement<'a>,
}

impl<'a> SqlWriter<'a, CommitKey, CommitExtra> for CommitWriter<'a> {
    fn prepare(tx: &'a Transaction) -> anyhow::Result<Self> {
        let sql = "INSERT INTO commits (id
                                      , sha1
                                      , is_merge
                                      , author_date
                                      , commit_date
                                      , has_change_info
                                      , has_presence_info
                                      , has_reachability_info)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?);";
        Ok(Self { stmt: tx.prepare_cached(sql)? })
    }

    fn execute(&mut self, id: Id, k: &CommitKey, e: &CommitExtra) -> anyhow::Result<usize> {
        Ok(self.stmt.execute(params![
            id,
            k.sha1,
            e.is_merge,
            e.author_time,
            e.commit_time,
            e.commit_info.contains(CommitInfo::CHANGES),
            e.commit_info.contains(CommitInfo::PRESENCE),
            e.commit_info.contains(CommitInfo::REACHABILITY),
        ])?)
    }
}

pub fn insert_commit(vt: &mut CommitVirtualTable, commit: &Commit) -> Id {
    let key = CommitKey::new(commit.id().to_string());
    let extra = CommitExtra::new(
        commit.parent_count() > 1,
        to_datetime(&commit.author().when()).unwrap().unix_timestamp(),
        to_datetime(&commit.committer().when()).unwrap().unix_timestamp(),
        CommitInfo::empty(),
    );
    vt.insert(key, extra)
}

// ========================================================
// Change -------------------------------------------------
// ========================================================

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChangeKey {
    commit_id: Id,
    entity_id: Id,
}

impl ChangeKey {
    pub fn new(commit_id: Id, entity_id: Id) -> Self {
        Self { commit_id, entity_id }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChangeExtra {
    kind: ChangeKind,
    adds: usize,
    dels: usize,
}

impl ChangeExtra {
    pub fn new(kind: ChangeKind, adds: usize, dels: usize) -> Self {
        Self { kind, adds, dels }
    }
}

pub type ChangeVirtualTable = VirtualTable<ChangeKey, ChangeExtra>;

pub struct ChangeWriter<'a> {
    stmt: CachedStatement<'a>,
}

impl<'a> SqlWriter<'a, ChangeKey, ChangeExtra> for ChangeWriter<'a> {
    fn prepare(tx: &'a Transaction) -> anyhow::Result<Self> {
        let sql = "INSERT INTO changes (id
                                      , commit_id
                                      , entity_id
                                      , kind
                                      , adds
                                      , dels)
                   VALUES (?, ?, ?, ?, ?, ?);";
        Ok(Self { stmt: tx.prepare_cached(sql)? })
    }

    fn execute(&mut self, id: Id, k: &ChangeKey, e: &ChangeExtra) -> anyhow::Result<usize> {
        Ok(self.stmt.execute(params![
            id,
            k.commit_id,
            k.entity_id,
            e.kind.to_string(),
            e.adds,
            e.dels
        ])?)
    }
}
