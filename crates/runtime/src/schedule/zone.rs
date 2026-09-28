use std::path::PathBuf;

/// A zone's UTC offsets over time, read from a compiled tz database file (RFC 8536); the
/// default is UTC. Cron fields are wall time in the zone this reads.
#[derive(Debug, Clone, Default)]
pub struct Zone {
    transitions: Vec<(i64, usize)>,
    types: Vec<(i64, String)>,
}

fn be_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        bytes.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn be_time(bytes: &[u8], at: usize, width: usize) -> Option<i64> {
    let raw = bytes.get(at..at.checked_add(width)?)?;
    match width {
        4 => Some(i64::from(i32::from_be_bytes(raw.try_into().ok()?))),
        _ => Some(i64::from_be_bytes(raw.try_into().ok()?)),
    }
}

fn counts(bytes: &[u8], start: usize) -> Option<[usize; 6]> {
    if bytes.get(start..start.checked_add(4)?)? != b"TZif" {
        return None;
    }
    let mut out = [0; 6];
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = usize::try_from(be_u32(bytes, start.checked_add(20 + 4 * index)?)?).ok()?;
    }
    Some(out)
}

fn block(bytes: &[u8], start: usize, width: usize) -> Option<(Zone, usize)> {
    let [isut, isstd, leap, time, type_count, chars] = counts(bytes, start)?;
    let mut at = start.checked_add(44)?;
    let times = (0..time)
        .map(|index| be_time(bytes, at.checked_add(index.checked_mul(width)?)?, width))
        .collect::<Option<Vec<i64>>>()?;
    at = at.checked_add(time.checked_mul(width)?)?;
    let indices = bytes.get(at..at.checked_add(time)?)?;
    at = at.checked_add(time)?;
    let ttinfo = type_count.checked_mul(6)?;
    let names = bytes.get(at.checked_add(ttinfo)?..)?;
    let types = (0..type_count)
        .map(|index| {
            let entry = at.checked_add(index.checked_mul(6)?)?;
            let offset = be_time(bytes, entry, 4)?;
            let name = names.get(usize::from(*bytes.get(entry.checked_add(5)?)?)..)?;
            let name = name.split(|byte| *byte == 0).next().unwrap_or_default();
            Some((offset, String::from_utf8_lossy(name).into_owned()))
        })
        .collect::<Option<Vec<_>>>()?;
    let transitions = times
        .into_iter()
        .zip(indices.iter().map(|index| usize::from(*index)))
        .collect();
    let end = [ttinfo, chars, leap.checked_mul(width + 4)?, isstd, isut]
        .into_iter()
        .try_fold(at, usize::checked_add)?;
    Some((Zone { transitions, types }, end))
}

impl Zone {
    /// `$TZ` as a zone name or path, else `/etc/localtime`; UTC when neither loads, which
    /// `/heartbeat status` shows by naming the zone.
    pub fn local() -> Self {
        let path = match std::env::var("TZ") {
            Ok(tz) if !tz.is_empty() => {
                let name = tz.strip_prefix(':').unwrap_or(&tz);
                PathBuf::from("/usr/share/zoneinfo").join(name)
            }
            _ => PathBuf::from("/etc/localtime"),
        };
        std::fs::read(path)
            .ok()
            .and_then(|bytes| Self::from_tzif(&bytes))
            .unwrap_or_default()
    }

    pub fn from_tzif(bytes: &[u8]) -> Option<Self> {
        let (first, end) = block(bytes, 0, 4)?;
        if bytes.get(4).is_some_and(|version| *version >= b'2') {
            return block(bytes, end, 8).map(|(zone, _)| zone);
        }
        Some(first)
    }

    // ponytail: past the last transition the last type holds; a slim file (zic -b slim) ends its
    // transitions early and puts later DST in a POSIX footer this skips. Parse it if one ships.
    fn at(&self, utc_ms: u64) -> Option<&(i64, String)> {
        let seconds = i64::try_from(utc_ms / 1_000).unwrap_or(i64::MAX);
        let before = self
            .transitions
            .partition_point(|(when, _)| *when <= seconds);
        let index = match before.checked_sub(1) {
            Some(last) => self.transitions.get(last)?.1,
            None => 0,
        };
        self.types.get(index)
    }

    pub(crate) fn wall_ms(&self, utc_ms: u64) -> u64 {
        let offset = self.at(utc_ms).map_or(0, |(offset, _)| *offset);
        utc_ms.saturating_add_signed(offset.saturating_mul(1_000))
    }

    pub(crate) fn abbreviation(&self, utc_ms: u64) -> &str {
        self.at(utc_ms).map_or("UTC", |(_, name)| name.as_str())
    }
}
