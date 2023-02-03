use anyhow::Context;
use anyhow::Result;
use time::OffsetDateTime;
use time::UtcOffset;

use crate::ir;

pub fn to_datetime(time: &git2::Time) -> Result<OffsetDateTime> {
    let datetime = OffsetDateTime::from_unix_timestamp(time.seconds())?;
    let offset = UtcOffset::from_whole_seconds(time.offset_minutes() * 60)?;
    Ok(datetime.replace_offset(offset))
}

pub fn to_commit(commit: &git2::Commit) -> Result<ir::Commit> {
    Ok(ir::Commit::new(
        commit.id(),
        commit.parent_count() > 1,
        to_datetime(&commit.author().when())?,
        to_datetime(&commit.committer().when())?,
    ))
}

pub fn to_ref(r#ref: &git2::Reference) -> Result<ir::Ref> {
    let commit = to_commit(&r#ref.peel_to_commit()?)?;
    let name = r#ref.name().context("missing ref name")?.to_string();
    Ok(ir::Ref::new(commit, name))
}

pub fn to_diffed_file(
    name: String,
    commit: &git2::Commit,
    delta: &git2::DiffDelta,
) -> Result<ir::DiffedFile> {
    Ok(ir::DiffedFile::new(
        name,
        to_commit(&commit).unwrap(),
        delta.old_file().id(),
        delta.new_file().id(),
        Vec::new(),
    ))
}
