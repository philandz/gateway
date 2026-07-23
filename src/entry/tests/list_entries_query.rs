//! Tests for gateway ListEntriesQuery parsing of comma-joined repeated IDs.

use crate::entry::ListEntriesQuery;

// ---------------------------------------------------------------------------
// category_ids parsing
// ---------------------------------------------------------------------------

#[test]
fn category_ids_parses_two_uuids() {
    let q: ListEntriesQuery = serde_urlencoded::from_str(
        "category_ids=11111111-1111-1111-1111-111111111111,22222222-2222-2222-2222-222222222222",
    )
    .unwrap();

    let ids: Vec<&str> = q
        .category_ids
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter(|s| !s.is_empty())
        .collect();

    assert_eq!(ids.len(), 2, "expected 2 IDs, got: {ids:?}");
    assert_eq!(ids[0], "11111111-1111-1111-1111-111111111111");
    assert_eq!(ids[1], "22222222-2222-2222-2222-222222222222");
}

#[test]
fn category_ids_parses_single_uuid() {
    let q: ListEntriesQuery =
        serde_urlencoded::from_str("category_ids=11111111-1111-1111-1111-111111111111").unwrap();

    assert_eq!(
        q.category_ids.as_deref(),
        Some("11111111-1111-1111-1111-111111111111")
    );
}

// ---------------------------------------------------------------------------
// member_ids parsing
// ---------------------------------------------------------------------------

#[test]
fn member_ids_parses_single_uuid() {
    let q: ListEntriesQuery =
        serde_urlencoded::from_str("member_ids=aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();

    let ids: Vec<&str> = q
        .member_ids
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter(|s| !s.is_empty())
        .collect();

    assert_eq!(ids.len(), 1, "expected 1 ID, got: {ids:?}");
    assert_eq!(ids[0], "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa");
}

// ---------------------------------------------------------------------------
// Empty string -> None
// ---------------------------------------------------------------------------

#[test]
fn category_ids_empty_string_yields_none() {
    let q: ListEntriesQuery = serde_urlencoded::from_str("category_ids=").unwrap();
    let split: Vec<&str> = q
        .category_ids
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter(|s| !s.is_empty())
        .collect();
    assert!(
        split.is_empty(),
        "empty category_ids should produce empty split, got: {split:?}"
    );
}

#[test]
fn member_ids_empty_string_yields_none() {
    let q: ListEntriesQuery = serde_urlencoded::from_str("member_ids=").unwrap();
    let split: Vec<&str> = q
        .member_ids
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter(|s| !s.is_empty())
        .collect();
    assert!(
        split.is_empty(),
        "empty member_ids should produce empty split, got: {split:?}"
    );
}

// ---------------------------------------------------------------------------
// Absent param -> None
// ---------------------------------------------------------------------------

#[test]
fn category_ids_absent_yields_none() {
    let q: ListEntriesQuery = serde_urlencoded::from_str("").unwrap();
    assert!(
        q.category_ids.is_none(),
        "absent category_ids should be None, got: {:?}",
        q.category_ids
    );
}

#[test]
fn member_ids_absent_yields_none() {
    let q: ListEntriesQuery = serde_urlencoded::from_str("").unwrap();
    assert!(
        q.member_ids.is_none(),
        "absent member_ids should be None, got: {:?}",
        q.member_ids
    );
}

// ---------------------------------------------------------------------------
// Singular category_id still works when category_ids absent
// ---------------------------------------------------------------------------

#[test]
fn singular_category_id_works_when_plural_absent() {
    let q: ListEntriesQuery =
        serde_urlencoded::from_str("category_id=cccccccc-cccc-cccc-cccc-cccccccccccc").unwrap();

    assert_eq!(
        q.category_id.as_deref(),
        Some("cccccccc-cccc-cccc-cccc-cccccccccccc")
    );
    // plural is still absent
    assert!(q.category_ids.is_none());
}

// ---------------------------------------------------------------------------
// Mixed singular + plural
// ---------------------------------------------------------------------------

#[test]
fn both_category_id_and_category_ids_present() {
    // Server precedence rule: singular wins when present
    let q: ListEntriesQuery = serde_urlencoded::from_str(
        "category_id=ssssssss-ssss-ssss-ssss-ssssssssssss&category_ids=11111111-1111-1111-1111-111111111111",
    )
    .unwrap();

    // Both deserialize correctly
    assert!(q.category_id.is_some());
    assert!(q.category_ids.is_some());
    // Server is responsible for precedence (singular wins)
}
