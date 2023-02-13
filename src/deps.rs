use std::collections::HashMap;
use std::fmt::Display;
use std::fmt::Formatter;
use std::fs::read_to_string;
use std::hash::Hash;
use std::path::Path;

use anyhow::bail;
use anyhow::Context;
use anyhow::Result;
use rusqlite::Connection;

use crate::db::DepExtra;
use crate::db::DepKey;
use crate::db::DepVirtualTable;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct DepFile {
    #[serde(rename = "cells")]
    cells: Vec<Cell>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Cell {
    #[serde(rename = "details", default)]
    details: Vec<Dep>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Dep {
    #[serde(rename = "src")]
    pub src: Endpoint,
    #[serde(rename = "dest")]
    pub tgt: Endpoint,
    #[serde(rename = "type")]
    pub kind: DepKind,
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub enum DepKind {
    Annotation,
    Call,
    Cast,
    Contain,
    Create,
    Extend,
    Implement,
    Import,
    Parameter,
    Return,
    Throw,
    Use,
}

impl Display for DepKind {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        match self {
            DepKind::Annotation => write!(f, "Annotation"),
            DepKind::Call => write!(f, "Call"),
            DepKind::Cast => write!(f, "Cast"),
            DepKind::Contain => write!(f, "Contain"),
            DepKind::Create => write!(f, "Create"),
            DepKind::Extend => write!(f, "Extend"),
            DepKind::Implement => write!(f, "Implement"),
            DepKind::Import => write!(f, "Import"),
            DepKind::Parameter => write!(f, "Parameter"),
            DepKind::Return => write!(f, "Return"),
            DepKind::Throw => write!(f, "Throw"),
            DepKind::Use => write!(f, "Use"),
        }
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Endpoint {
    #[serde(rename = "object")]
    pub full_name: String,
    #[serde(rename = "type")]
    pub kind: EndpointKind,
    #[serde(rename = "file")]
    pub file: String,
    #[serde(rename = "lineNumber")]
    pub line: usize,
}

impl Endpoint {
    pub fn name(&self) -> &str {
        match self.full_name.split('.').last() {
            Some(name) => name,
            None => &self.full_name,
        }
    }

    pub fn parent_name(&self) -> &str {
        todo!()
    }
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub enum EndpointKind {
    #[serde(rename = "file")]
    File,
    #[serde(rename = "function")]
    Function,
    // #[serde(rename = "package")]
    // Package
    #[serde(rename = "type")]
    Type,
    #[serde(rename = "var")]
    Var,
}

impl Display for EndpointKind {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        match self {
            EndpointKind::File => write!(f, "File"),
            EndpointKind::Function => write!(f, "Function"),
            EndpointKind::Type => write!(f, "Type"),
            EndpointKind::Var => write!(f, "Var"),
        }
    }
}

#[derive(Debug)]
pub struct Loc {
    entity_id: usize,
    name: String,
    filename: String,
    level: usize,
    start_row: usize,
    end_row: usize,
}

pub fn load_dep_file<P: AsRef<Path>>(path: P) -> Result<Vec<Dep>> {
    let json = read_to_string(path)?;
    let dep_file = serde_json::from_str::<DepFile>(&json)?;
    Ok(dep_file.cells.into_iter().flat_map(|f| f.details).collect())
}

pub fn get_commit_id(conn: &Connection, sha1: &String) -> Result<usize> {
    let mut stmt = conn.prepare("SELECT id FROM commits WHERE sha1 = :sha1")?;

    let res =
        stmt.query_map(&[(":sha1", &sha1)], |row| Ok(row.get(0)?))?.try_collect::<Vec<usize>>()?;

    assert!(res.len() == 1);
    Ok(res[0])
}

pub fn load_locs(conn: &Connection, sha1: &String) -> Result<HashMap<String, Vec<Loc>>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE filenames (entity_id, filename, level) AS
        (
            SELECT E.id AS entity_id, E.name AS filename, 0 as level
            FROM entities E
            WHERE E.kind = 'file'
        
            UNION ALL
        
            SELECT E.id AS entity_id, F.filename, F.level + 1
            FROM entities E, filenames F
            WHERE E.parent_id = F.entity_id
        )
        SELECT F.entity_id, E.name,  F.filename, F.level, P.start_row, P.end_row
        FROM presence P
        JOIN commits CO ON CO.id = P.commit_id
        JOIN filenames F ON P.entity_id = F.entity_id
        JOIN entities E ON P.entity_id = E.id
        WHERE CO.sha1 = :sha1
        ORDER BY P.entity_id",
    )?;

    let locs = stmt.query_map(&[(":sha1", &sha1)], |row| {
        Ok(Loc {
            entity_id: row.get(0)?,
            name: row.get(1)?,
            filename: row.get(2)?,
            level: row.get(3)?,
            start_row: row.get(4)?,
            end_row: row.get(5)?,
        })
    })?;

    let mut map: HashMap<String, Vec<Loc>> = HashMap::new();

    for loc in locs {
        let loc = loc?;
        map.entry(loc.filename.clone()).or_default().push(loc);
    }

    Ok(map)
}

pub enum MatchRes {
    Success(usize),
    TooMany(Vec<usize>),
    NotFound,
    FileNotFound,
}

pub fn match_entity_id(locs: &HashMap<String, Vec<Loc>>, ep: &Endpoint) -> Option<usize> {
    let locs = locs.get(&ep.file);

    if locs.is_none() {
        log::warn!("Could not find file {}", &ep.file);
        return None;
    }

    let locs = locs.unwrap().into_iter();

    let locs: Vec<_> = match (ep.line, ep.kind) {
        (_, EndpointKind::File) => locs.filter(|l| l.level == 0).collect(),
        // Maybe just ignore this dep, if the line number is 0 and its not a file
        (0, _) => {
            return None;
        },
        _ => locs.filter(|l| ep.line >= l.start_row && ep.line <= l.end_row).collect(),
    };

    if locs.len() == 0 {
        log::warn!("Could not find a {} at {}:{}", &ep.kind, &ep.file, &ep.line);
        return None;
    } else if locs.len() == 1 {
        return Some(locs.get(0).unwrap().entity_id);
    }

    let ep_name = ep.name();
    let by_name_locs = locs.iter().filter(|l| l.name == ep_name).collect::<Vec<_>>();

    if by_name_locs.len() == 0 {
        // There are a couple reasons why an entity can't be found by name:
        // - It is a parameter name
        // - It is a function inside an anonymous class inside a function
        // Maybe we could check for these cases?
        log::debug!("Could not find a {} named '{}' at {}:{}", &ep.kind, ep_name, &ep.file, &ep.line);
    } else if by_name_locs.len() == 1 {
        return Some(by_name_locs.get(0).unwrap().entity_id);
    }

    // If can't find by name, default to the max level
    let max_level = locs.iter().map(|l| l.level).max().unwrap();
    let locs = locs.into_iter().filter(|l| l.level == max_level).collect::<Vec<_>>();

    if locs.len() == 1 {
        return Some(locs.get(0).unwrap().entity_id);
    }

    log::warn!("Found too many entities named '{}' at {}:{}", ep_name, &ep.file, &ep.line);
    None
}

pub fn insert_deps(
    vt: &mut DepVirtualTable,
    locs: &HashMap<String, Vec<Loc>>,
    deps: &Vec<Dep>,
    commit_id: usize,
) -> Result<()> {
    for dep in deps {
        let src_id = match match_entity_id(locs, &dep.src) {
            Some(id) => id,
            None => continue,
        };

        let tgt_id = match match_entity_id(locs, &dep.tgt) {
            Some(id) => id,
            None => continue,
        };

        let key = DepKey::new(commit_id, src_id, tgt_id, dep.kind.to_string());
        let extra = DepExtra::new(dep.src.line);
        vt.insert(key, extra);
    }

    Ok(())
}
