use time::{OffsetDateTime, UtcOffset};

pub fn to_datetime(time: &git2::Time) -> anyhow::Result<OffsetDateTime> {
    let datetime = OffsetDateTime::from_unix_timestamp(time.seconds())?;
    let offset = UtcOffset::from_whole_seconds(time.offset_minutes() * 60)?;
    Ok(datetime.replace_offset(offset))
}
