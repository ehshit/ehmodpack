use std::io::{Cursor, Read};

type Range = ((u32, u32), (u32, u32));

const ANY_MINOR: u32 = u32::MAX;

pub fn declared_ranges(mcmeta: &[u8]) -> Vec<Range> {
    let mut ranges = Vec::new();
    let Ok(doc) = serde_json::from_slice::<serde_json::Value>(mcmeta) else {
        return ranges;
    };
    let Some(pack) = doc.get("pack") else {
        return ranges;
    };

    if let Some(pf) = pack.get("pack_format").and_then(|v| v.as_u64()) {
        ranges.push(((pf as u32, 0), (pf as u32, ANY_MINOR)));
    }
    if let Some(sf) = pack.get("supported_formats") {
        push_legacy(&mut ranges, sf);
    }
    push_new_style(&mut ranges, pack);

    for holder in [doc.get("overlays"), pack.get("overlays")]
        .into_iter()
        .flatten()
    {
        if let Some(entries) = holder.as_array() {
            for entry in entries {
                if let Some(f) = entry.get("formats") {
                    push_legacy(&mut ranges, f);
                }
                push_new_style(&mut ranges, entry);
            }
        }
    }

    ranges
}

pub fn supports(mcmeta: &[u8], format: u32) -> bool {
    let ranges = declared_ranges(mcmeta);
    if ranges.is_empty() {
        return false;
    }
    let target = (format, 0u32);
    ranges
        .iter()
        .any(|(lo, hi)| *lo <= target && target <= *hi)
}

pub fn zip_supports(bytes: &[u8], format: u32) -> bool {
    let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(bytes)) else {
        return false;
    };
    let names: Vec<String> = (0..archive.len())
        .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
        .filter(|n| n.ends_with("pack.mcmeta"))
        .collect();
    if names.is_empty() {
        return false;
    }
    let best = names
        .iter()
        .min_by_key(|n| (n.matches('/').count(), n.len()))
        .cloned()
        .unwrap();
    let Ok(mut file) = archive.by_name(&best) else {
        return false;
    };
    let mut buf = Vec::new();
    if file.read_to_end(&mut buf).is_err() {
        return false;
    }
    supports(&buf, format)
}

fn push_legacy(ranges: &mut Vec<Range>, value: &serde_json::Value) {
    if let Some(v) = value.as_u64() {
        ranges.push(((v as u32, 0), (v as u32, ANY_MINOR)));
    } else if let Some(arr) = value.as_array() {
        match arr.len() {
            1 => {
                if let Some(a) = arr[0].as_u64() {
                    ranges.push(((a as u32, 0), (a as u32, ANY_MINOR)));
                }
            }
            _ => {
                if let (Some(a), Some(b)) = (arr[0].as_u64(), arr[1].as_u64()) {
                    ranges.push(((a as u32, 0), (b as u32, ANY_MINOR)));
                }
            }
        }
    } else if let (Some(lo), Some(hi)) = (
        value.get("min_inclusive").and_then(|v| v.as_u64()),
        value.get("max_inclusive").and_then(|v| v.as_u64()),
    ) {
        ranges.push(((lo as u32, 0), (hi as u32, ANY_MINOR)));
    }
}

fn push_new_style(ranges: &mut Vec<Range>, obj: &serde_json::Value) {
    let min_raw = obj.get("min_format");
    let max_raw = obj.get("max_format");
    if min_raw.is_none() && max_raw.is_none() {
        return;
    }
    let min = min_raw.and_then(new_bound).unwrap_or((0, 0));
    let max = max_raw.and_then(new_bound).unwrap_or((ANY_MINOR, ANY_MINOR));
    ranges.push((min, max));
}

fn new_bound(value: &serde_json::Value) -> Option<(u32, u32)> {
    if let Some(v) = value.as_u64() {
        return Some((v as u32, 0));
    }
    let arr = value.as_array()?;
    match arr.len() {
        1 => Some((arr[0].as_u64()? as u32, 0)),
        _ => Some((arr[0].as_u64()? as u32, arr[1].as_u64()? as u32)),
    }
}