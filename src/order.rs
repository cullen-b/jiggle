//! The ring's app order: saved once, then only changed by the user dragging apps around.

use std::collections::HashMap;

/// Sorts `paths` by the `saved` order. Paths seen for the first time go at the end, in the order given.
/// Returns the sorted paths and the saved order with those new paths appended.
pub fn apply(saved: &[String], paths: Vec<String>) -> (Vec<String>, Vec<String>) {
    let mut order = saved.to_vec();
    for p in &paths {
        if !order.contains(p) {
            order.push(p.clone());
        }
    }
    let pos: HashMap<&str, usize> = order.iter().enumerate().map(|(i, p)| (p.as_str(), i)).collect();
    let mut sorted = paths;
    sorted.sort_by_key(|p| pos[p.as_str()]);
    (sorted, order)
}

/// The saved order after the user dragged `moved` so that the apps on screen read `visible`.
/// Apps that aren't on screen (closed, hidden) keep their places.
pub fn after_move(saved: &[String], visible: &[String], moved: &str) -> Vec<String> {
    let mut order: Vec<String> = saved.iter().filter(|p| *p != moved).cloned().collect();
    let Some(i) = visible.iter().position(|p| p == moved) else { return saved.to_vec() };
    let at = match (visible.get(i + 1), i.checked_sub(1).map(|j| &visible[j])) {
        // Just before the app it now sits in front of...
        (Some(next), _) => order.iter().position(|p| p == next).unwrap_or(order.len()),
        // ...or, if it's last on screen, just after the one before it.
        (None, Some(prev)) => order.iter().position(|p| p == prev).map_or(order.len(), |j| j + 1),
        (None, None) => 0,
    };
    order.insert(at, moved.to_string());
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn saved_order_wins_and_new_apps_go_last() {
        let (sorted, order) = apply(&v(&["c", "a", "b"]), v(&["a", "new", "b", "c"]));
        assert_eq!(sorted, v(&["c", "a", "b", "new"]));
        assert_eq!(order, v(&["c", "a", "b", "new"]));
    }

    #[test]
    fn closing_an_app_does_not_move_the_others() {
        let saved = v(&["a", "b", "c", "d"]);
        let (sorted, order) = apply(&saved, v(&["d", "a", "c"]));
        assert_eq!(sorted, v(&["a", "c", "d"]));
        assert_eq!(order, saved);
    }

    #[test]
    fn moving_keeps_offscreen_apps_in_place() {
        // "x" is closed (not on screen); drag "d" between "a" and "b".
        let saved = v(&["a", "x", "b", "c", "d"]);
        assert_eq!(after_move(&saved, &v(&["a", "d", "b", "c"]), "d"), v(&["a", "x", "d", "b", "c"]));
        // Drag "a" to the end.
        assert_eq!(after_move(&saved, &v(&["b", "c", "d", "a"]), "a"), v(&["x", "b", "c", "d", "a"]));
        // Drag "d" to the front.
        assert_eq!(after_move(&saved, &v(&["d", "a", "b", "c"]), "d"), v(&["d", "a", "x", "b", "c"]));
    }
}
