use std::hash::Hash;
use std::{collections::HashMap, sync::Arc};

use derive_new::new;
use rusqlite::{params, CachedStatement, Transaction};

use crate::ir::{ChangeKind, CommitInfo};

pub type Id = usize;

pub trait SqlWriter<'a, K: Hash + Eq, E> {
    fn create_table_script() -> &'static str;
    fn prepare(tx: &'a Transaction) -> anyhow::Result<Self>
    where
        Self: Sized;
    fn execute(&mut self, id: Id, key: &K, extra: &E) -> anyhow::Result<usize>;
}

#[derive(Debug, Default)]
pub struct VirtualTable<K: Default + Hash + Eq, E: Default> {
    map: HashMap<K, (E, Id)>,
    next_id: Id,
}

impl<K: Default + Hash + Eq, E: Default> VirtualTable<K, E> {
    /// Creates a new [`VirtualTable<K, E>`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the length of this [`VirtualTable<K, E>`].
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
        // Create table
        tx.execute(W::create_table_script(), params![])?;

        // Sorting is required for the entities table to maintain the "parent_id"
        // constraint
        let mut rows = self.map.into_iter().collect::<Vec<_>>();
        rows.sort_by_key(|(_, (_, id))| *id);

        // Insert all
        let mut writer = W::prepare(tx)?;

        for (key, (extra, id)) in rows {
            writer.execute(id, &key, &extra)?;
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NullExtra;

// ========================================================
// Entity -------------------------------------------------
// ========================================================

#[derive(new, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntityKey {
    parent_id: Option<Id>,
    name: String,
    kind: Arc<String>,
}

pub type EntityVirtualTable = VirtualTable<EntityKey, NullExtra>;

pub struct EntityWriter<'a> {
    stmt: CachedStatement<'a>,
}

impl<'a> SqlWriter<'a, EntityKey, NullExtra> for EntityWriter<'a> {
    fn create_table_script() -> &'static str {
        "CREATE TABLE entities (
            id INT NOT NULL PRIMARY KEY,
            parent_id INT,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            -- extra TEXT,
            
            FOREIGN KEY(parent_id) REFERENCES entities(id),
            CHECK((kind == 'file' AND parent_id IS NULL) OR
                  (kind != 'file' AND parent_id IS NOT NULL)),
            UNIQUE(parent_id, name, kind)
        ) WITHOUT ROWID;"
    }

    fn prepare(tx: &'a Transaction) -> anyhow::Result<Self> {
        let sql = "INSERT INTO entities (id, parent_id, name, kind) VALUES (?, ?, ?, ?);";
        Ok(Self { stmt: tx.prepare_cached(sql)? })
    }

    fn execute(&mut self, id: Id, key: &EntityKey, _: &NullExtra) -> anyhow::Result<usize> {
        Ok(self.stmt.execute(params![id, key.parent_id, key.name, key.kind])?)
    }
}

// ========================================================
// Commit -------------------------------------------------
// ========================================================

#[derive(new, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommitKey {
    sha1: String,
}

#[derive(new, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommitExtra {
    is_merge: bool,
    author_time: i64,
    commit_time: i64,
    commit_info: CommitInfo,
}

pub type CommitVirtualTable = VirtualTable<CommitKey, CommitExtra>;

pub struct CommitWriter<'a> {
    stmt: CachedStatement<'a>,
}

impl<'a> SqlWriter<'a, CommitKey, CommitExtra> for CommitWriter<'a> {
    fn create_table_script() -> &'static str {
        "CREATE TABLE commits (
            id INT NOT NULL PRIMARY KEY,
            sha1 CHAR(40) NOT NULL UNIQUE,
            is_merge BOOLEAN NOT NULL,
            -- author_name TEXT,
            -- author_mail TEXT,
            author_date INT NOT NULL,
            -- commit_name TEXT,
            -- commit_mail TEXT,
            commit_date INT NOT NULL,
        
            has_change_info BOOLEAN NOT NULL,
            has_presence_info BOOLEAN NOT NULL,
            has_reachability_info BOOLEAN NOT NULL
        ) WITHOUT ROWID;"
    }

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

// ========================================================
// Ref ----------------------------------------------------
// ========================================================

#[derive(new, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RefKey {
    name: String,
}

#[derive(new, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RefExtra {
    commit_id: Id,
}

pub type RefVirtualTable = VirtualTable<RefKey, RefExtra>;

pub struct RefWriter<'a> {
    stmt: CachedStatement<'a>,
}

impl<'a> SqlWriter<'a, RefKey, RefExtra> for RefWriter<'a> {
    fn create_table_script() -> &'static str {
        "CREATE TABLE refs (
            id INT NOT NULL PRIMARY KEY,
            commit_id INT NOT NULL,
            name TEXT NOT NULL UNIQUE,
        
            FOREIGN KEY(commit_id) REFERENCES commits(id)
        ) WITHOUT ROWID;"
    }

    fn prepare(tx: &'a Transaction) -> anyhow::Result<Self> {
        let sql = "INSERT INTO refs (id, commit_id, name) VALUES (?, ?, ?);";
        Ok(Self { stmt: tx.prepare_cached(sql)? })
    }

    fn execute(&mut self, id: Id, k: &RefKey, e: &RefExtra) -> anyhow::Result<usize> {
        Ok(self.stmt.execute(params![id, e.commit_id, k.name])?)
    }
}

// ========================================================
// Change -------------------------------------------------
// ========================================================

#[derive(new, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChangeKey {
    commit_id: Id,
    entity_id: Id,
}

#[derive(new, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChangeExtra {
    kind: ChangeKind,
    adds: usize,
    dels: usize,
}

pub type ChangeVirtualTable = VirtualTable<ChangeKey, ChangeExtra>;

pub struct ChangeWriter<'a> {
    stmt: CachedStatement<'a>,
}

impl<'a> SqlWriter<'a, ChangeKey, ChangeExtra> for ChangeWriter<'a> {
    fn create_table_script() -> &'static str {
        "CREATE TABLE changes (
            id INT NOT NULL PRIMARY KEY,
            commit_id INT NOT NULL,
            entity_id INT NOT NULL,
            kind CHAR NOT NULL,
            adds INT NOT NULL,
            dels INT NOT NULL,
        
            FOREIGN KEY(commit_id) REFERENCES commits(id),
            FOREIGN KEY(entity_id) REFERENCES entities(id),
            UNIQUE(commit_id, entity_id),
            CHECK(kind = 'A' OR kind = 'D' or kind = 'M')
            -- CHECK(adds > 0 OR dels > 0)
        ) WITHOUT ROWID;"
    }

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

// ========================================================
// Range -------------------------------------------------
// ========================================================

// #[derive(new, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
// pub struct RangeKey {
//     start_byte: usize,
//     start_col: usize,
//     start_row: usize,
//     end_byte: usize,
//     end_col: usize,
//     end_row: usize,
// }

// pub type RangeVirtualTable = VirtualTable<RangeKey, NullExtra>;

// pub struct RangeWriter<'a> {
//     stmt: CachedStatement<'a>,
// }

// impl<'a> SqlWriter<'a, RangeKey, NullExtra> for RangeWriter<'a> {
//     fn create_table_script() -> &'static str {
//         "CREATE TABLE ranges (
//             id INT NOT NULL PRIMARY KEY,
//             start_byte INT NOT NULL,
//             start_col INT NOT NULL,
//             start_row INT NOT NULL,
//             end_byte INT NOT NULL,
//             end_col INT NOT NULL,
//             end_row INT NOT NULL,

//             UNIQUE(start_byte, start_col, start_row, end_byte, end_col,
// end_row)         ) WITHOUT ROWID;"
//     }

//     fn prepare(tx: &'a Transaction) -> anyhow::Result<Self> {
//         let sql = "INSERT INTO ranges (id
//                                      , start_byte
//                                      , start_col
//                                      , start_row
//                                      , end_byte
//                                      , end_col
//                                      , end_row)
//                    VALUES (?, ?, ?, ?, ?, ?, ?);";
//         Ok(Self { stmt: tx.prepare_cached(sql)? })
//     }

//     fn execute(&mut self, id: Id, k: &RangeKey, _: &NullExtra) ->
// anyhow::Result<usize> {         Ok(self.stmt.execute(params![
//             id,
//             k.start_byte,
//             k.start_col,
//             k.end_byte,
//             k.end_col,
//             k.end_row
//         ])?)
//     }
// }

// ========================================================
// Presence -----------------------------------------------
// ========================================================

#[derive(new, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PresenceKey {
    commit_id: Id,
    entity_id: Id,
}

#[derive(new, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PresenceExtra {
    start_row: usize,
    end_row: usize,
}

pub type PresenceVirtualTable = VirtualTable<PresenceKey, PresenceExtra>;

pub struct PresenceWriter<'a> {
    stmt: CachedStatement<'a>,
}

impl<'a> SqlWriter<'a, PresenceKey, PresenceExtra> for PresenceWriter<'a> {
    fn create_table_script() -> &'static str {
        "CREATE TABLE presence (
            id INT NOT NULL PRIMARY KEY,
            commit_id INT NOT NULL,
            entity_id INT NOT NULL,
            start_row INT NOT NULL,
            end_row INT NOT NULL,
        
            FOREIGN KEY(commit_id) REFERENCES commits(id),
            FOREIGN KEY(entity_id) REFERENCES entities(id),
            UNIQUE(commit_id, entity_id)
        ) WITHOUT ROWID;"
    }

    fn prepare(tx: &'a Transaction) -> anyhow::Result<Self> {
        let sql = "INSERT INTO presence (id, commit_id, entity_id, start_row, end_row) VALUES (?, ?, ?, ?, ?);";
        Ok(Self { stmt: tx.prepare_cached(sql)? })
    }

    fn execute(&mut self, id: Id, k: &PresenceKey, e: &PresenceExtra) -> anyhow::Result<usize> {
        Ok(self.stmt.execute(params![id, k.commit_id, k.entity_id, e.start_row, e.end_row])?)
    }
}

// ========================================================
// Reachability -------------------------------------------
// ========================================================

#[derive(new, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReachabilityKey {
    source_id: Id,
    target_id: Id,
}

pub type ReachabilityVirtualTable = VirtualTable<ReachabilityKey, NullExtra>;

pub struct ReachabilityWriter<'a> {
    stmt: CachedStatement<'a>,
}

impl<'a> SqlWriter<'a, ReachabilityKey, NullExtra> for ReachabilityWriter<'a> {
    fn create_table_script() -> &'static str {
        "CREATE TABLE reachability (
            id INT NOT NULL PRIMARY KEY,
            source_id INT NOT NULL,
            target_id INT NOT NULL,
        
            FOREIGN KEY(source_id) REFERENCES commits(id),
            FOREIGN KEY(target_id) REFERENCES commits(id),
            UNIQUE(source_id, target_id)
        ) WITHOUT ROWID;"
    }

    fn prepare(tx: &'a Transaction) -> anyhow::Result<Self> {
        let sql = "INSERT INTO reachability (id, source_id, target_id) VALUES (?, ?, ?);";
        Ok(Self { stmt: tx.prepare_cached(sql)? })
    }

    fn execute(&mut self, id: Id, k: &ReachabilityKey, _: &NullExtra) -> anyhow::Result<usize> {
        Ok(self.stmt.execute(params![id, k.source_id, k.target_id])?)
    }
}

// ========================================================
// Database -----------------------------------------------
// ========================================================

#[derive(Debug, Default)]
pub struct VirtualDb {
    pub entity_vt: EntityVirtualTable,
    pub commit_vt: CommitVirtualTable,
    pub ref_vt: RefVirtualTable,
    pub change_vt: ChangeVirtualTable,
    // pub range_vt: RangeVirtualTable,
    pub presence_vt: PresenceVirtualTable,
    pub reachability_vt: ReachabilityVirtualTable,
}

impl VirtualDb {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn write<'a>(self, tx: &'a Transaction) -> anyhow::Result<()> {
        self.entity_vt.write::<EntityWriter>(&tx)?;
        self.commit_vt.write::<CommitWriter>(&tx)?;
        self.ref_vt.write::<RefWriter>(&tx)?;
        self.change_vt.write::<ChangeWriter>(&tx)?;
        // self.range_vt.write::<RangeWriter>(&tx)?;
        self.presence_vt.write::<PresenceWriter>(&tx)?;
        self.reachability_vt.write::<ReachabilityWriter>(&tx)?;
        Ok(())
    }
}