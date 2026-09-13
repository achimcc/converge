//! Dotted field paths (`TrickplayOptions.EnableHwAcceleration`) over JSON
//! objects. A path names properties only -- no list indices: every object a
//! Jellyfin task sets is reached through named properties.

use serde_json::Value;

/// The segments of a path, or `None` if a segment is empty (`a..b`, `.a`).
pub fn segments(path: &str) -> Option<Vec<&str>> {
    let parts: Vec<&str> = path.split('.').collect();
    parts.iter().all(|p| !p.is_empty()).then_some(parts)
}

/// The value at `path`, if every segment exists.
pub fn get<'a>(object: &'a Value, path: &str) -> Option<&'a Value> {
    segments(path)?
        .into_iter()
        .try_fold(object, |current, segment| current.get(segment))
}

/// Replaces the value at `path`. Every segment must already exist: a path
/// the service did not send is never added on the way back.
pub fn set(object: &mut Value, path: &str, value: Value) -> bool {
    let Some(parts) = segments(path) else {
        return false;
    };
    let (last, parents) = parts.split_last().expect("split yields one part");
    let mut current = object;
    for segment in parents {
        match current.get_mut(*segment) {
            Some(next) => current = next,
            None => return false,
        }
    }
    match current.as_object_mut() {
        Some(map) if map.contains_key(*last) => {
            map.insert((*last).to_string(), value);
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn empty_segments_are_not_a_path() {
        assert_eq!(segments("a.b"), Some(vec!["a", "b"]));
        for bad in ["", ".a", "a.", "a..b"] {
            assert_eq!(segments(bad), None, "{bad}");
        }
    }

    #[test]
    fn get_follows_objects() {
        let v = json!({"A": {"B": true}, "C": 1});
        assert_eq!(get(&v, "A.B"), Some(&json!(true)));
        assert_eq!(get(&v, "C"), Some(&json!(1)));
        assert_eq!(get(&v, "A.X"), None);
        assert_eq!(get(&v, "C.D"), None);
    }

    #[test]
    fn set_replaces_but_never_adds() {
        let mut v = json!({"A": {"B": false, "Keep": 7}});
        assert!(set(&mut v, "A.B", json!(true)));
        assert_eq!(v, json!({"A": {"B": true, "Keep": 7}}));
        assert!(!set(&mut v, "A.New", json!(1)), "no new key");
        assert!(!set(&mut v, "X.B", json!(1)), "no new parent");
        assert_eq!(v, json!({"A": {"B": true, "Keep": 7}}));
    }
}
