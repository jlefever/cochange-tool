use bitflags::bitflags;
use derive_new::new;
use std::sync::Arc;

use git2::Oid;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tag {
    pub name: String,
    pub parent: Option<Arc<Tag>>,
    pub kind: Arc<String>,
}

impl Tag {
    pub fn new(parent: Arc<Tag>, name: String, kind: Arc<String>) -> Self {
        Self { name, parent: Some(parent), kind }
    }

    pub fn new_root(name: String, kind: Arc<String>) -> Self {
        Self { name, parent: None, kind }
    }

    pub fn to_vec(&self) -> Vec<(String, Arc<String>)> {
        let mut ancestors = vec![(self.name.clone(), self.kind.clone())];
        let mut current = &self.parent;

        while let Some(tag) = current {
            ancestors.push((tag.name.clone(), tag.kind.clone()));
            current = &tag.parent;
        }

        ancestors.reverse();
        ancestors
    }
}

// both endpoints should be inclusive
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Interval(pub usize, pub usize);

impl Interval {
    pub fn intersect(&self, other: &Interval) -> usize {
        let p0 = self.0.max(other.0);
        let p1 = self.1.min(other.1);
        p1.checked_sub(p0).unwrap_or_default()
    }
}

#[derive(new, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalTag {
    pub tag: Arc<Tag>,
    pub interval: Interval,
}

#[derive(new, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hunk {
    pub old_interval: Interval,
    pub new_interval: Interval,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ChangeKind {
    Added,
    #[default]
    Modified,
    Deleted,
}

impl ChangeKind {
    pub fn to_char(&self) -> char {
        self.into()
    }

    pub fn to_string(&self) -> String {
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

#[derive(Builder, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Change {
    pub tag: Arc<Tag>,
    pub commit: Oid,
    #[builder(default)]
    pub kind: ChangeKind,
    #[builder(default)]
    pub adds: usize,
    #[builder(default)]
    pub dels: usize,
}

#[derive(new, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Presence {
    pub local_tag: LocalTag,
    pub commit: Oid,
}

bitflags! {
    pub struct CommitInfo: u8 {
        const CHANGES = 0b00000001;
        const PRESENCE = 0b00000010;
        const REACHABILITY = 0b00000100;
    }
}

impl Default for CommitInfo {
    fn default() -> Self {
        Self::empty()
    }
}
