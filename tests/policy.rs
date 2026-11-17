//! What the run may do without asking.
//!
//! The table is the whole security surface, and it is pure, so it is checked
//! here rather than through a run: no provider, no workspace, no network.

use std::collections::HashMap;

use harness::policy::{Permission, Policy};

fn lookup(entries: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> = entries
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    move |name| map.get(name).cloned()
}

#[test]
fn reading_is_allowed_and_writing_is_asked_about() {
    let policy = Policy::default();
    assert_eq!(policy.decide("read_file"), Permission::Allow);
    assert_eq!(policy.decide("list_dir"), Permission::Allow);
    assert_eq!(policy.decide("write_file"), Permission::Ask);
    assert_eq!(policy.decide("bash"), Permission::Ask);
}

#[test]
fn anything_unlisted_is_refused() {
    let policy = Policy::default();
    assert_eq!(policy.decide("rm_rf"), Permission::Deny);
    assert_eq!(policy.decide(""), Permission::Deny);
}

#[test]
fn the_environment_changes_one_tool_without_clearing_the_rest() {
    let policy = Policy::from_lookup(lookup(&[("LLM_TOOLS_ALLOW", "bash")]));

    assert_eq!(policy.decide("bash"), Permission::Allow, "asked is lifted");
    assert_eq!(
        policy.decide("write_file"),
        Permission::Ask,
        "the defaults it did not name still hold"
    );
}

#[test]
fn the_most_restrictive_listing_wins() {
    let policy = Policy::from_lookup(lookup(&[
        ("LLM_TOOLS_ALLOW", "bash"),
        ("LLM_TOOLS_DENY", "bash"),
    ]));

    assert_eq!(policy.decide("bash"), Permission::Deny);
}

#[test]
fn a_list_is_trimmed_and_may_be_empty() {
    let policy = Policy::from_lookup(lookup(&[("LLM_TOOLS_DENY", " bash , ,read_file ")]));

    assert_eq!(policy.decide("bash"), Permission::Deny);
    assert_eq!(policy.decide("read_file"), Permission::Deny);
}

#[test]
fn a_name_can_be_set_from_code() {
    let mut policy = Policy::default();
    policy.set("write_file", Permission::Deny);
    assert_eq!(policy.decide("write_file"), Permission::Deny);

    policy.set("new_tool", Permission::Allow);
    assert_eq!(policy.decide("new_tool"), Permission::Allow);
}
