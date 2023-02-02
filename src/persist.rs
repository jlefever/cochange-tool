use std::hash::Hash;
use std::marker::PhantomData;
use std::{borrow::Borrow, collections::HashMap, sync::Arc};

use bitflags::bitflags;
use git2::{Commit, Oid};
use rusqlite::{params, params_from_iter, CachedStatement, Params, ToSql, Transaction};

use crate::{tagging::Tag, time::to_datetime};

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

bitflags! {
    pub struct CommitInfo: u8 {
        const CHANGES = 0b00000001;
        const PRESENCE = 0b00000010;
        const REACHABILITY = 0b00000100;
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
    pub fn new(is_merge: bool, author_time: i64, commit_time: i64, commit_info: CommitInfo) -> Self {
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

// TODO: Move this elsewhere
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ChangeKind {
    Added,
    #[default]
    Modified,
    Deleted,
}

impl ChangeKind {
    fn to_char(&self) -> char {
        self.into()
    }

    fn to_string(&self) -> String {
        self.to_char().to_string()
    }
}

impl From<&ChangeKind> for char {
    fn from(change_kind: &ChangeKind) -> Self {
        match change_kind {
            ChangeKind::Added => 'A',
            ChangeKind::Modified => 'M',
            ChangeKind::Deleted => 'D',
        }
    }
}

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

// #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
// struct EntityRow {
//     parent_id: Option<Pk>,
//     name: String,
//     kind: Arc<String>,
// }

// impl EntityRow {
//     fn new(parent_id: Option<Pk>, name: String, kind: Arc<String>) -> Self {
//         Self { parent_id, name, kind }
//     }
// }

// pub struct EntityVt {
//     map: HashMap<EntityRow, Pk>,
//     next_pk: Pk,
// }

// impl EntityVt {
//     pub fn new() -> Self {
//         Self { map: HashMap::new(), next_pk: 0 }
//     }

//     pub fn insert_entity<E: Borrow<Tag>>(&mut self, entity: E) -> Pk {
//         let mut prev_id = None;

//         for (name, kind) in entity.borrow().to_vec() {
//             let row = EntityRow::new(prev_id, name, kind);
//             prev_id = Some(self.insert_row(row));
//         }

//         prev_id.unwrap()
//     }

//     fn insert_row(&mut self, row: EntityRow) -> Pk {
//         *self.map.entry(row).or_insert_with(|| {
//             let pk = self.next_pk;
//             self.next_pk += 1;
//             pk
//         })
//     }

//     pub fn len(&self) -> usize {
//         self.map.len()
//     }

//     pub fn write(self, tx: &Transaction) -> anyhow::Result<()> {
//         let mut arr = self.map.into_iter().collect::<Vec<_>>();
//         arr.sort_by_key(|&(_, pk)| pk);

//         let mut stmt = tx.prepare_cached(
//             "INSERT INTO entities (id, parent_id, name, kind) VALUES (?, ?,
// ?, ?);",         )?;

//         for (row, pk) in arr {
//             let x = params![pk, row.parent_id, row.name, row.kind];

//             stmt.execute(params![pk, row.parent_id, row.name, row.kind])?;
//         }

//         Ok(())
//     }
// }

// #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
// struct CommitRow {
//     sha1: Oid,
//     is_merge: bool,

//     author_name: Option<String>,
//     author_mail: Option<String>,
//     author_time: i64,

//     commit_name: Option<String>,
//     commit_mail: Option<String>,
//     commit_time: i64,

//     has_change_info: bool,
//     has_presence_info: bool,
//     has_reachability_info: bool,
// }

// impl CommitRow {
//     fn new<'r, C: Borrow<Commit<'r>>>(commit: C, info: CommitInfo) -> Self {
//         let commit: &Commit = commit.borrow();

//         Self {
//             sha1: commit.id(),
//             is_merge: commit.parent_count() > 1,
//             author_name: commit.author().name().map(str::to_string),
//             author_mail: commit.author().email().map(str::to_string),
//             author_time:
// to_datetime(&commit.author().when()).unwrap().unix_timestamp(),
// commit_name: commit.committer().name().map(str::to_string),
// commit_mail: commit.committer().email().map(str::to_string),
// commit_time:
// to_datetime(&commit.committer().when()).unwrap().unix_timestamp(),
//             has_change_info: info.contains(CommitInfo::CHANGES),
//             has_presence_info: info.contains(CommitInfo::PRESENCE),
//             has_reachability_info: info.contains(CommitInfo::REACHABILITY),
//         }
//     }
// }

// pub struct CommitVt {
//     map: HashMap<Oid, (CommitRow, Pk)>,
//     next_pk: Pk,
// }

// impl CommitVt {
//     pub fn new() -> Self {
//         Self { map: HashMap::new(), next_pk: 0 }
//     }

//     pub fn insert_commit<'r, C: Borrow<Commit<'r>>>(&mut self, commit: C) ->
// Pk {         todo!()
//     }

//     fn insert_row(&mut self, row: CommitRow) -> Pk {}
// }
