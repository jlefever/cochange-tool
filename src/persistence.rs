use std::borrow::Borrow;

use git2::Commit;
use git2::Reference;

use crate::db::*;
use crate::ir::*;
use crate::time::to_datetime;

pub fn insert_tag<T: Borrow<Tag>>(db: &mut VirtualDb, tag: T) -> Id {
    let mut prev_id = None;

    for (name, kind) in tag.borrow().to_vec() {
        let key = EntityKey::new(prev_id, name, kind);
        prev_id = Some(db.entity_vt.insert(key, NullExtra));
    }

    prev_id.unwrap()
}

pub fn insert_commit(db: &mut VirtualDb, commit: &Commit) -> Id {
    let key = CommitKey::new(commit.id().to_string());
    let extra = CommitExtra::new(
        commit.parent_count() > 1,
        to_datetime(&commit.author().when()).unwrap().unix_timestamp(),
        to_datetime(&commit.committer().when()).unwrap().unix_timestamp(),
        CommitInfo::empty(),
    );
    db.commit_vt.insert(key, extra)
}

pub fn insert_change(db: &mut VirtualDb, change: &Change) -> Id {
    let commit_id = db.commit_vt.get_id(&CommitKey::new(change.commit.to_string())).unwrap();
    let entity_id = insert_tag(db, change.tag.clone());

    let change_key = ChangeKey::new(commit_id, entity_id);
    let change_extra = ChangeExtra::new(change.kind, change.adds, change.dels);

    db.change_vt.insert(change_key, change_extra)
}

pub fn insert_presence(db: &mut VirtualDb, presence: &Presence) -> Id {
    let commit_id = db.commit_vt.get_id(&CommitKey::new(presence.commit.to_string())).unwrap();
    let entity_id = insert_tag(db, presence.local_tag.tag.clone());

    let interval = presence.local_tag.interval;

    let presence_key = PresenceKey::new(commit_id, entity_id);
    let presence_extra = PresenceExtra::new(interval.0, interval.1);

    db.presence_vt.insert(presence_key, presence_extra)
}

pub fn insert_ref<'r>(db: &mut VirtualDb, r#ref: &Reference<'r>) -> Id {
    let ref_name = r#ref.name().unwrap().to_string();
    let commit = r#ref.peel_to_commit().unwrap();
    let commit_id = insert_commit(db, &commit);

    let ref_key = RefKey::new(ref_name);
    let ref_extra = RefExtra::new(commit_id);

    db.ref_vt.insert(ref_key, ref_extra)
}