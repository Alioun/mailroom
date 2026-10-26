//! Structural changes between two JSON values, so a journal frame carries only
//! the parts of a field that changed: the batch a run just updated rather than
//! the whole run, and the step a run just added rather than all its steps.

use super::CensusError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Below this depth a change replaces the whole value, which keeps a frame's
/// nesting well inside the JSON parser's recursion limit.
const MAX_DEPTH: usize = 24;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Patch {
    Replace {
        value: Value,
    },
    Object {
        set: BTreeMap<String, Patch>,
        remove: Vec<String>,
    },
    Array {
        len: usize,
        set: Vec<(usize, Patch)>,
    },
}

/// The change from `old` to `new`, or `None` when they are equal.
pub(super) fn diff(old: &Value, new: &Value) -> Option<Patch> {
    diff_at(old, new, 0)
}

fn diff_at(old: &Value, new: &Value, depth: usize) -> Option<Patch> {
    if old == new {
        return None;
    }
    let replace = || Patch::Replace { value: new.clone() };
    Some(match (old, new) {
        (Value::Object(old), Value::Object(new)) if depth < MAX_DEPTH => Patch::Object {
            set: new
                .iter()
                .filter_map(|(key, value)| {
                    let patch = match old.get(key) {
                        Some(before) => diff_at(before, value, depth + 1)?,
                        None => Patch::Replace {
                            value: value.clone(),
                        },
                    };
                    Some((key.clone(), patch))
                })
                .collect(),
            remove: old
                .keys()
                .filter(|key| !new.contains_key(*key))
                .cloned()
                .collect(),
        },
        (Value::Array(old), Value::Array(new)) if depth < MAX_DEPTH => Patch::Array {
            len: new.len(),
            set: new
                .iter()
                .enumerate()
                .filter_map(|(index, value)| {
                    let patch = match old.get(index) {
                        Some(before) => diff_at(before, value, depth + 1)?,
                        None => Patch::Replace {
                            value: value.clone(),
                        },
                    };
                    Some((index, patch))
                })
                .collect(),
        },
        _ => replace(),
    })
}

pub(super) fn apply(target: &mut Value, patch: Patch) -> Result<(), CensusError> {
    let invalid = || CensusError::Storage;
    match patch {
        Patch::Replace { value } => *target = value,
        Patch::Object { set, remove } => {
            let Value::Object(map) = target else {
                return Err(invalid());
            };
            for key in remove {
                map.remove(&key).ok_or_else(invalid)?;
            }
            for (key, patch) in set {
                match patch {
                    Patch::Replace { value } => {
                        map.insert(key, value);
                    }
                    patch => apply(map.get_mut(&key).ok_or_else(invalid)?, patch)?,
                }
            }
        }
        Patch::Array { len, set } => {
            let Value::Array(items) = target else {
                return Err(invalid());
            };
            let mut previous = None;
            for (index, patch) in set {
                if previous.is_some_and(|p| index <= p) || index >= len {
                    return Err(invalid());
                }
                previous = Some(index);
                if index < items.len() {
                    apply(&mut items[index], patch)?;
                } else if let (true, Patch::Replace { value }) = (index == items.len(), patch) {
                    items.push(value);
                } else {
                    return Err(invalid());
                }
            }
            if items.len() < len {
                return Err(invalid());
            }
            items.truncate(len);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn round_trip(old: Value, new: Value) -> Option<Patch> {
        let patch = diff(&old, &new);
        let mut applied = old.clone();
        if let Some(p) = patch.clone() {
            let encoded: Patch = serde_json::from_slice(&serde_json::to_vec(&p).unwrap()).unwrap();
            apply(&mut applied, encoded).unwrap();
        }
        assert_eq!(applied, new);
        patch
    }

    #[test]
    fn patches_reproduce_the_new_value_and_carry_only_the_change() {
        assert_eq!(round_trip(json!({"a": 1}), json!({"a": 1})), None);
        let run = |batches: Vec<Value>| json!({"runs": [{"id": "r", "batches": batches, "status": "running"}]});
        let big = json!({"evidence": "x".repeat(4096), "state": "intent"});
        let mut changed = big.clone();
        changed["state"] = json!("acknowledged");
        let patch = round_trip(
            run(vec![big.clone(), big.clone(), big.clone()]),
            run(vec![big.clone(), changed, big.clone()]),
        )
        .unwrap();
        assert!(serde_json::to_vec(&patch).unwrap().len() < 256);
        round_trip(json!([1, 2, 3]), json!([1, 5]));
        round_trip(json!([1]), json!([1, {"b": [2]}, 3]));
        round_trip(json!({"a": 1, "b": 2}), json!({"b": 3, "c": null}));
        round_trip(json!({"a": [1]}), json!({"a": {"x": 1}}));
        round_trip(json!(null), json!({"a": 1}));
        let mut deep = json!(1);
        for _ in 0..40 {
            deep = json!({"n": deep});
        }
        let mut deeper = deep.clone();
        let mut cursor = &mut deeper;
        while cursor.get("n").is_some() {
            cursor = cursor.get_mut("n").unwrap();
        }
        *cursor = json!(2);
        round_trip(deep, deeper);
    }

    #[test]
    fn patches_that_do_not_fit_their_target_are_rejected() {
        for (target, patch) in [
            (json!([1]), json!({"op": "object", "set": {}, "remove": []})),
            (
                json!({"a": 1}),
                json!({"op": "object", "set": {}, "remove": ["b"]}),
            ),
            (
                json!({"a": 1}),
                json!({"op": "object", "set": {"b": {"op": "array", "len": 0, "set": []}}, "remove": []}),
            ),
            (
                json!([1]),
                json!({"op": "array", "len": 3, "set": [[2, {"op": "replace", "value": 1}]]}),
            ),
            (
                json!([1, 2]),
                json!({"op": "array", "len": 2, "set": [[1, {"op": "replace", "value": 1}], [0, {"op": "replace", "value": 1}]]}),
            ),
            (json!([1]), json!({"op": "array", "len": 2, "set": []})),
        ] {
            let mut target = target;
            let patch: Patch = serde_json::from_value(patch).unwrap();
            assert_eq!(apply(&mut target, patch), Err(CensusError::Storage));
        }
    }
}
