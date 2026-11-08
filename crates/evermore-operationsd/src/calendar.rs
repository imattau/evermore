//! Calendar query/projection over the existing relay store. The relay keeps
//! the signed NIP-52 events; this module only projects them for local clients
//! and optionally pulls from the owner's explicitly configured remote relays.

use std::collections::BTreeMap;
use std::net::TcpStream;
use std::time::Duration;

use nostr::prelude::*;
use serde_json::{Value, json};
use tungstenite::Message;
use tungstenite::stream::MaybeTlsStream;

const EVENT_KINDS: [u16; 4] = [31922, 31923, 31924, 31925];

pub fn list(limit: usize, owner_identity: &str, owner_pubkey: &PublicKey) -> Result<Value, String> {
    let limit = limit.clamp(1, 100);
    let filter = json!({"kinds": EVENT_KINDS, "limit": limit});
    let local_relay = super::configured_relay_url();
    let events = query(&local_relay, filter)?;
    let mut records = project_deduped(events, 100)?;
    let public_deletions = query(&local_relay, json!({"kinds": [5], "limit": 100}))?;
    let public_tombstones = public_deletion_targets(public_deletions);
    let wraps = query(
        &local_relay,
        json!({"kinds": [1059], "#p": [owner_pubkey.to_hex()], "limit": 100}),
    )?;
    let private_tombstones = private_deletion_targets(&wraps, owner_identity, owner_pubkey);
    let private = unwrap_private(wraps, owner_identity, owner_pubkey)?;
    let mut private_latest = BTreeMap::<String, Value>::new();
    for record in private {
        let Some(coordinate) = record.get("coordinate").and_then(Value::as_str) else {
            continue;
        };
        let replace = private_latest.get(coordinate).is_none_or(|current| {
            record
                .get("created_at")
                .and_then(Value::as_u64)
                .unwrap_or_default()
                > current
                    .get("created_at")
                    .and_then(Value::as_u64)
                    .unwrap_or_default()
        });
        if replace {
            private_latest.insert(coordinate.to_owned(), record);
        }
    }
    if let Some(records) = records.as_array_mut() {
        let known_ids = records
            .iter()
            .filter_map(|record| record.get("event_id").and_then(Value::as_str))
            .map(str::to_owned)
            .collect::<std::collections::HashSet<_>>();
        records.extend(private_latest.into_values().filter(|record| {
            record
                .get("event_id")
                .and_then(Value::as_str)
                .is_some_and(|id| !known_ids.contains(id))
        }));
        records.retain(|record| {
            !record_deleted(record, &public_tombstones)
                && !record_deleted(record, &private_tombstones)
        });
        records.sort_by(|a, b| {
            a.get("start_sort")
                .and_then(Value::as_i64)
                .cmp(&b.get("start_sort").and_then(Value::as_i64))
        });
        records.truncate(limit);
        for record in records.iter_mut() {
            record
                .as_object_mut()
                .map(|object| object.remove("start_sort"));
        }
    }
    Ok(records)
}

pub fn sync(owner_pubkey: &PublicKey, owner_identity: &str) -> Result<usize, String> {
    let mut stored = 0;
    for relay in configured_relays().into_iter().take(8) {
        let remote = match query(
            &relay,
            json!({"kinds": EVENT_KINDS, "authors": [owner_pubkey.to_hex()], "limit": 100}),
        ) {
            Ok(events) => events,
            Err(error) => {
                eprintln!("evermore-operationsd: calendar sync from {relay} deferred: {error}");
                continue;
            }
        };
        for event in remote {
            let value = serde_json::to_value(event).map_err(|error| error.to_string())?;
            if super::publish_event(&super::configured_relay_url(), &value).is_ok() {
                stored += 1;
            }
        }
        if let Ok(deletions) = query(
            &relay,
            json!({"kinds": [5], "authors": [owner_pubkey.to_hex()], "limit": 100}),
        ) {
            for event in deletions {
                let value = serde_json::to_value(event).map_err(|error| error.to_string())?;
                if super::publish_event(&super::configured_relay_url(), &value).is_ok() {
                    stored += 1;
                }
            }
        }
        let wraps = query(
            &relay,
            json!({"kinds": [1059], "#p": [owner_pubkey.to_hex()], "limit": 100}),
        );
        if let Ok(wraps) = wraps {
            for wrap in wraps {
                let valid_event = unwrap_private(vec![wrap.clone()], owner_identity, owner_pubkey)
                    .is_ok_and(|records| !records.is_empty());
                let valid_tombstone =
                    !private_deletion_targets(&[wrap.clone()], owner_identity, owner_pubkey)
                        .is_empty();
                if valid_event || valid_tombstone {
                    let value = serde_json::to_value(wrap).map_err(|error| error.to_string())?;
                    if super::publish_event(&super::configured_relay_url(), &value).is_ok() {
                        stored += 1;
                    }
                }
            }
        }
    }
    Ok(stored)
}

pub fn get_for_write(
    event_id_or_coordinate: &str,
    owner_identity: &str,
    owner_pubkey: &PublicKey,
) -> Result<(Value, bool), String> {
    let requested_id = EventId::parse(event_id_or_coordinate)
        .map_err(|_| "calendar writes require the selected event's event_id, not a coordinate")?;
    let public_filter = json!({"ids": [requested_id.to_hex()], "limit": 1});
    let local_relay = super::configured_relay_url();
    if let Some(event) = query(&local_relay, public_filter)?.into_iter().next() {
        let value = serde_json::to_value(event).map_err(|error| error.to_string())?;
        ensure_current_revision(&value, false, owner_identity, owner_pubkey)?;
        return Ok((value, false));
    }

    let wraps = query(
        &local_relay,
        json!({"kinds": [1059], "#p": [owner_pubkey.to_hex()], "limit": 100}),
    )?;
    for wrap in wraps {
        if !has_recipient_tag(&wrap, owner_pubkey) {
            continue;
        }
        let Ok(seal_json) = signer_decrypt(owner_identity, &wrap.pubkey.to_hex(), &wrap.content)
        else {
            continue;
        };
        let Ok(seal) = serde_json::from_str::<Event>(&seal_json) else {
            continue;
        };
        if seal.kind.as_u16() != 13 || !seal.tags.is_empty() || seal.verify().is_err() {
            continue;
        }
        let Ok(rumor_json) = signer_decrypt(owner_identity, &seal.pubkey.to_hex(), &seal.content)
        else {
            continue;
        };
        let Ok(rumor) = serde_json::from_str::<UnsignedEvent>(&rumor_json) else {
            continue;
        };
        if rumor.pubkey != seal.pubkey
            || rumor.verify_id().is_err()
            || !matches!(rumor.kind.as_u16(), 31922 | 31923)
        {
            continue;
        }
        if rumor.id.as_ref() == Some(&requested_id) {
            let value: Value =
                serde_json::from_str(&rumor.as_json()).map_err(|error| error.to_string())?;
            ensure_current_revision(&value, true, owner_identity, owner_pubkey)?;
            return Ok((value, true));
        }
    }
    Err(format!(
        "calendar event '{event_id_or_coordinate}' was not found"
    ))
}

fn ensure_current_revision(
    raw: &Value,
    private: bool,
    owner_identity: &str,
    owner_pubkey: &PublicKey,
) -> Result<(), String> {
    let coordinate = super::event_coordinate(raw)?;
    let revision = raw["id"].as_str().ok_or("calendar event id is missing")?;
    let records = list(100, owner_identity, owner_pubkey)?;
    let current = records
        .as_array()
        .and_then(|records| {
            records.iter().find(|record| {
                record.get("coordinate").and_then(Value::as_str) == Some(&coordinate)
            })
        })
        .ok_or("calendar event is deleted or unavailable")?;
    if current.get("event_id").and_then(Value::as_str) != Some(revision)
        || current.get("visibility").and_then(Value::as_str)
            != Some(if private { "private" } else { "public" })
    {
        return Err("calendar event changed since it was selected; review the latest event before changing it".into());
    }
    Ok(())
}

fn configured_relays() -> Vec<String> {
    std::env::var("EVERMORE_CALENDAR_RELAYS")
        .unwrap_or_default()
        .split([',', ';'])
        .map(str::trim)
        .filter(|relay| !relay.is_empty() && *relay != super::configured_relay_url())
        .map(str::to_owned)
        .collect()
}

fn query(relay_url: &str, filter: Value) -> Result<Vec<Event>, String> {
    let (mut socket, _) = tungstenite::connect(relay_url)
        .map_err(|error| format!("couldn't connect to calendar relay {relay_url}: {error}"))?;
    match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => set_timeout(stream)?,
        MaybeTlsStream::Rustls(stream) => set_timeout(&stream.sock)?,
        _ => {}
    }
    let subscription = format!("evermore-calendar-{}", std::process::id());
    socket
        .send(Message::text(
            json!(["REQ", subscription, filter]).to_string(),
        ))
        .map_err(|error| format!("calendar subscription failed: {error}"))?;
    let max_events = filter
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .clamp(1, 100) as usize;
    let mut events = Vec::new();
    loop {
        let message = match socket.read() {
            Ok(message) => message,
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                break;
            }
            Err(error) => return Err(format!("calendar relay read failed: {error}")),
        };
        let Message::Text(text) = message else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        match value.get(0).and_then(Value::as_str) {
            Some("EVENT") if value.get(1).and_then(Value::as_str) == Some(&subscription) => {
                if let Some(raw) = value.get(2) {
                    let event: Event = serde_json::from_value(raw.clone())
                        .map_err(|error| format!("relay returned an invalid event: {error}"))?;
                    event
                        .verify()
                        .map_err(|error| format!("relay event signature is invalid: {error}"))?;
                    if EVENT_KINDS.contains(&event.kind.as_u16())
                        || matches!(event.kind.as_u16(), 5 | 1059)
                    {
                        events.push(event);
                        if events.len() >= max_events {
                            break;
                        }
                    }
                }
            }
            Some("EOSE") if value.get(1).and_then(Value::as_str) == Some(&subscription) => break,
            Some("CLOSED") if value.get(1).and_then(Value::as_str) == Some(&subscription) => {
                return Err("calendar relay closed the subscription".into());
            }
            _ => {}
        }
    }
    let _ = socket.send(Message::text(json!(["CLOSE", subscription]).to_string()));
    Ok(events)
}

fn set_timeout(stream: &TcpStream) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(4)))
        .map_err(|error| error.to_string())
}

fn unwrap_private(
    wraps: Vec<Event>,
    owner_identity: &str,
    owner_pubkey: &PublicKey,
) -> Result<Vec<Value>, String> {
    let mut records = Vec::new();
    for wrap in wraps {
        if wrap.kind.as_u16() != 1059
            || wrap.verify().is_err()
            || !has_recipient_tag(&wrap, owner_pubkey)
        {
            continue;
        }
        let Ok(seal_json) = signer_decrypt(owner_identity, &wrap.pubkey.to_hex(), &wrap.content)
        else {
            continue;
        };
        let Ok(seal) = serde_json::from_str::<Event>(&seal_json) else {
            continue;
        };
        if seal.kind.as_u16() != 13 || !seal.tags.is_empty() || seal.verify().is_err() {
            continue;
        }
        let Ok(rumor_json) = signer_decrypt(owner_identity, &seal.pubkey.to_hex(), &seal.content)
        else {
            continue;
        };
        let Ok(rumor) = serde_json::from_str::<UnsignedEvent>(&rumor_json) else {
            continue;
        };
        if rumor.pubkey != seal.pubkey
            || rumor.verify_id().is_err()
            || !matches!(rumor.kind.as_u16(), 31922 | 31923)
        {
            continue;
        }
        let mut raw: Value =
            serde_json::from_str(&rumor.as_json()).map_err(|error| error.to_string())?;
        raw["sig"] = Value::String("00".repeat(64));
        let event: Event = serde_json::from_value(raw).map_err(|error| error.to_string())?;
        if let Ok(mut record) = project_event(event) {
            record["visibility"] = Value::String("private".into());
            record
                .as_object_mut()
                .map(|object| object.remove("raw_event"));
            records.push(record);
        }
    }
    Ok(records)
}

fn private_deletion_targets(
    wraps: &[Event],
    owner_identity: &str,
    owner_pubkey: &PublicKey,
) -> BTreeMap<String, String> {
    let mut targets = BTreeMap::new();
    for wrap in wraps {
        if wrap.kind.as_u16() != 1059
            || wrap.verify().is_err()
            || !has_recipient_tag(wrap, owner_pubkey)
        {
            continue;
        }
        let Ok(seal_json) = signer_decrypt(owner_identity, &wrap.pubkey.to_hex(), &wrap.content)
        else {
            continue;
        };
        let Ok(seal) = serde_json::from_str::<Event>(&seal_json) else {
            continue;
        };
        if seal.kind.as_u16() != 13 || !seal.tags.is_empty() || seal.verify().is_err() {
            continue;
        }
        let Ok(rumor_json) = signer_decrypt(owner_identity, &seal.pubkey.to_hex(), &seal.content)
        else {
            continue;
        };
        let Ok(rumor) = serde_json::from_str::<UnsignedEvent>(&rumor_json) else {
            continue;
        };
        if rumor.pubkey != seal.pubkey || rumor.verify_id().is_err() || rumor.kind.as_u16() != 5 {
            continue;
        }
        for tag in rumor.tags {
            let values = tag.as_slice();
            if matches!(values.first().map(String::as_str), Some("a" | "e")) {
                if let Some(target) = values.get(1) {
                    targets.insert(target.clone(), rumor.pubkey.to_hex());
                }
            }
        }
    }
    targets
}

fn public_deletion_targets(events: Vec<Event>) -> BTreeMap<String, String> {
    let mut targets = BTreeMap::new();
    for event in events {
        if event.kind.as_u16() != 5 || event.verify().is_err() {
            continue;
        }
        for tag in event.tags.iter() {
            let values = tag.as_slice();
            if matches!(values.first().map(String::as_str), Some("a" | "e")) {
                if let Some(target) = values.get(1) {
                    targets.insert(target.clone(), event.pubkey.to_hex());
                }
            }
        }
    }
    targets
}

fn record_deleted(record: &Value, tombstones: &BTreeMap<String, String>) -> bool {
    let author = record.get("pubkey").and_then(Value::as_str);
    let target = record
        .get("event_id")
        .and_then(Value::as_str)
        .filter(|id| tombstones.contains_key(*id))
        .or_else(|| {
            record
                .get("coordinate")
                .and_then(Value::as_str)
                .filter(|coordinate| tombstones.contains_key(*coordinate))
        });
    target
        .and_then(|target| tombstones.get(target))
        .is_some_and(|deleter| Some(deleter.as_str()) == author)
}

fn signer_decrypt(identity: &str, sender_pubkey: &str, ciphertext: &str) -> Result<String, String> {
    match super::send_request::<_, evermore_common::Response>(
        &evermore_common::runtime_socket_path(),
        &evermore_common::Request::Nip44Decrypt {
            identity: identity.into(),
            sender_pubkey: sender_pubkey.into(),
            ciphertext: ciphertext.into(),
        },
    )? {
        evermore_common::Response::Ok { data } => data
            .get("plaintext")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or("signer response is missing plaintext".into()),
        evermore_common::Response::Err { message } => Err(message),
    }
}

fn has_recipient_tag(event: &Event, recipient: &PublicKey) -> bool {
    tags(event, "p")
        .iter()
        .any(|value| value == &recipient.to_hex())
}

fn project_deduped(events: Vec<Event>, limit: usize) -> Result<Value, String> {
    let mut latest = BTreeMap::<String, Event>::new();
    for event in events {
        let d = tag(&event, "d").unwrap_or_default();
        let coordinate = format!("{}:{}:{d}", event.kind.as_u16(), event.pubkey.to_hex());
        let replace = latest
            .get(&coordinate)
            .is_none_or(|current| event.created_at > current.created_at);
        if replace {
            latest.insert(coordinate, event);
        }
    }
    let mut records = latest
        .into_values()
        .filter_map(|event| match project_event(event) {
            Ok(record) => Some(record),
            Err(error) => {
                eprintln!("evermore-operationsd: ignoring malformed calendar event: {error}");
                None
            }
        })
        .collect::<Vec<_>>();
    records.sort_by(|a, b| {
        a.get("start_sort")
            .and_then(Value::as_i64)
            .cmp(&b.get("start_sort").and_then(Value::as_i64))
    });
    records.truncate(limit);
    serde_json::to_value(records).map_err(|error| error.to_string())
}

fn project_event(event: Event) -> Result<Value, String> {
    let kind = event.kind.as_u16();
    let d = tag(&event, "d").ok_or("NIP-52 event is missing its d tag")?;
    let title = tag(&event, "title").unwrap_or_else(|| {
        if kind == 31924 {
            "Calendar".into()
        } else {
            "RSVP".into()
        }
    });
    let start = tag(&event, "start").unwrap_or_default();
    let end = tag(&event, "end");
    if matches!(kind, 31922 | 31923) && (title.is_empty() || start.is_empty()) {
        return Err("calendar event is missing its title or start tag".into());
    }
    if kind == 31924 && tag(&event, "title").is_none() {
        return Err("calendar list is missing its title tag".into());
    }
    if kind == 31925 && (tag(&event, "a").is_none() || tag(&event, "status").is_none()) {
        return Err("calendar RSVP is missing its event reference or status".into());
    }
    let start_unix = if kind == 31923 {
        start.parse::<i64>().ok()
    } else {
        None
    };
    if kind == 31923 && start_unix.is_none() {
        return Err("time-based start must be Unix seconds".into());
    }
    if kind == 31922
        && (!valid_date(&start) || end.as_deref().is_some_and(|value| !valid_date(value)))
    {
        return Err("date-based start/end must use valid YYYY-MM-DD dates".into());
    }
    let start_utc = start_unix.and_then(|seconds| {
        ::time::OffsetDateTime::from_unix_timestamp(seconds)
            .ok()?
            .format(&::time::format_description::well_known::Rfc3339)
            .ok()
    });
    let start_sort = if kind == 31922 {
        date_sort_key(&start).unwrap_or(i64::MAX)
    } else {
        start_unix.unwrap_or(i64::MAX)
    };
    let record = json!({
        "event_id": event.id.to_hex(),
        "coordinate": format!("{kind}:{}:{d}", event.pubkey.to_hex()),
        "kind": kind,
        "pubkey": event.pubkey.to_hex(),
        "d_tag": d,
        "title": title,
        "start": start,
        "end": end,
        "start_utc": start_utc,
        "start_tzid": tag(&event, "start_tzid"),
        "end_tzid": tag(&event, "end_tzid"),
        "summary": tag(&event, "summary"),
        "content": event.content,
        "locations": tags(&event, "location"),
        "participants": tags(&event, "p"),
        "related_coordinates": tags(&event, "a"),
        "rsvp_status": tag(&event, "status"),
        "rsvp_free_busy": tag(&event, "fb"),
        "visibility": "public",
        "created_at": event.created_at.as_secs(),
        "raw_event": event,
        "start_sort": start_sort
    });
    Ok(record)
}

pub fn valid_date(value: &str) -> bool {
    let Some((year, rest)) = value.split_once('-') else {
        return false;
    };
    let Some((month, day)) = rest.split_once('-') else {
        return false;
    };
    let (Ok(year), Ok(month), Ok(day)) = (
        year.parse::<u32>(),
        month.parse::<u32>(),
        day.parse::<u32>(),
    ) else {
        return false;
    };
    if value.len() != 10 || month == 0 || month > 12 {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=max_day).contains(&day)
}

fn date_sort_key(value: &str) -> Option<i64> {
    if !valid_date(value) {
        return None;
    }
    let year = value[0..4].parse::<i64>().ok()?;
    let month = value[5..7].parse::<i64>().ok()?;
    let day = value[8..10].parse::<i64>().ok()?;
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some((era * 146097 + day_of_era - 719468) * 86400)
}

fn tag(event: &Event, name: &str) -> Option<String> {
    event.tags.iter().find_map(|tag| {
        let fields = tag.as_slice();
        (fields.first().map(String::as_str) == Some(name))
            .then(|| fields.get(1).cloned())
            .flatten()
    })
}

fn tags(event: &Event, name: &str) -> Vec<String> {
    event
        .tags
        .iter()
        .filter_map(|tag| {
            let fields = tag.as_slice();
            (fields.first().map(String::as_str) == Some(name))
                .then(|| fields.get(1).cloned())
                .flatten()
        })
        .collect()
}
