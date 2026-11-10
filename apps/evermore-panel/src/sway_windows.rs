//! Sway window geometry for the feed drawer's drag-to-attach trick.
//!
//! A drag that starts in the feed window and ends over the AI window is
//! invisible to the AI window: Wayland keeps the implicit pointer grab on
//! the source surface, so egui's drop zone never sees the release. The feed
//! still receives the release, so it asks Sway where both windows are and
//! checks whether the pointer landed inside the AI drawer. That is enough to
//! make "drag a note onto the chat" work without an OS-level drag source.

use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// Both drawer rectangles in Sway layout coordinates. `None` when `swaymsg`
/// is unavailable or that window is not mapped.
pub fn drawer_rects(own_pid: u32) -> (Option<Rect>, Option<Rect>) {
    let Some(tree) = get_tree() else {
        return (None, None);
    };
    let mut feed = None;
    let mut ai = None;
    walk(&tree, &mut |node| {
        if !is_visible_view(node) {
            return;
        }
        match node.get("name").and_then(|name| name.as_str()) {
            Some("Evermore feed") if feed.is_none() => feed = node_rect(node),
            Some("Evermore AI") if ai.is_none() => ai = node_rect(node),
            _ => {}
        }
    });
    // Prefer our own feed window if more than one is open.
    if let Some(own) = find_view_by_title(&tree, "Evermore feed", Some(own_pid)) {
        feed = node_rect(own);
    }
    (feed, ai)
}

/// Whether a window with this title is currently mapped on any output.
pub fn window_exists(title: &str) -> bool {
    let Some(tree) = get_tree() else {
        return false;
    };
    find_view_by_title(&tree, title, None).is_some()
}

/// Moves this process's feed window flush against the right edge of the
/// workspace it opened on. Sway's `move position` has no right-anchor, and
/// computing it in the config with `100 ppt` + `move left` can pick the wrong
/// output on a multi-monitor layout, so the drawer positions itself once it
/// maps. Anchoring to the workspace (rather than the output) keeps it clear
/// of Waybar's exclusive zone and configured outer gaps.
///
/// `expected_width` is the width the caller just requested via
/// `ViewportCommand::InnerSize` (e.g. collapsing to 40px). A Wayland resize
/// is asynchronous -- the compositor has to configure, and the client has
/// to ack and commit a new buffer -- so `get_tree` can still report the
/// *old* width for a frame or two after the request. Trusting that stale
/// width here computes the right-edge target from the wrong window size and
/// then marks positioning done, permanently landing the window short of the
/// true edge (confirmed: this is exactly what left the collapsed drawer
/// sitting ~300px off the right edge instead of flush against it). Refusing
/// to proceed until the reported width matches what was actually requested
/// turns that race into a harmless retry instead of a silent wrong answer.
pub fn anchor_to_right_edge(pid: u32, expected_width: f32) -> Result<(), String> {
    let tree = get_tree().ok_or_else(|| "swaymsg get_tree is unavailable".to_string())?;
    let Some((con_id, target_x, target_y)) =
        compute_right_edge_move(&tree, "Evermore feed", pid, expected_width)?
    else {
        return Ok(());
    };
    let command = format!("[con_id={con_id}] move absolute position {target_x} {target_y}");
    let status = Command::new("swaymsg")
        .arg(command)
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("swaymsg could not move the feed window".into())
    }
}

/// The decision logic behind `anchor_to_right_edge`, split out so it can be
/// exercised against a crafted tree in a test without shelling out to a
/// real compositor: `Ok(None)` means the window is already flush against
/// the edge and nothing needs to move; `Ok(Some((con_id, x, y)))` is the
/// move to issue; `Err` covers everything that should make the caller
/// retry, including the stale-width guard documented on the caller.
fn compute_right_edge_move(
    tree: &serde_json::Value,
    title: &str,
    pid: u32,
    expected_width: f32,
) -> Result<Option<(i64, i64, i64)>, String> {
    let (node, workspace) = find_view_with_workspace(tree, title, pid)
        .ok_or_else(|| "feed window is not mapped yet".to_string())?;
    let window = node_rect(node).ok_or_else(|| "feed window has no rect".to_string())?;
    if (window.width - expected_width).abs() > 1.0 {
        return Err("feed window has not finished resizing yet".to_string());
    }
    let workspace = node_rect(workspace).ok_or_else(|| "workspace has no rect".to_string())?;
    let con_id = node
        .get("id")
        .and_then(|id| id.as_i64())
        .ok_or_else(|| "feed window has no con_id".to_string())?;
    let (target_x, target_y) = right_edge_target(workspace, window);
    if (window.x - target_x).abs() < 1.0 && (window.y - target_y).abs() < 1.0 {
        return Ok(None);
    }
    Ok(Some((
        con_id,
        target_x.round() as i64,
        target_y.round() as i64,
    )))
}

fn right_edge_target(workspace: Rect, window: Rect) -> (f32, f32) {
    (workspace.x + workspace.width - window.width, workspace.y)
}

/// Raises this title to the top of Sway's floating stack. A floating
/// window normally renders above the tiling layer regardless of which
/// tiled window has focus, but stacking *among* floating windows follows
/// focus/raise order -- so a collapsed 40px drawer that lost focus to
/// whatever tiled window the user clicked into can end up stacked behind
/// it, with that window visibly covering part of the thin drawer strip.
/// Collapsing or expanding a drawer is always a deliberate click on that
/// same window, so re-asserting its focus right after the resize is not a
/// surprise focus steal -- it is just confirming the window the user was
/// already using stays on top.
pub fn raise_window(title: &str) -> Result<(), String> {
    let command = format!("[title=\"{title}\"] focus");
    let status = Command::new("swaymsg")
        .arg(command)
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("swaymsg could not focus \"{title}\""))
    }
}

fn find_view_with_workspace<'a>(
    node: &'a serde_json::Value,
    title: &str,
    pid: u32,
) -> Option<(&'a serde_json::Value, &'a serde_json::Value)> {
    fn walk<'a>(
        node: &'a serde_json::Value,
        title: &str,
        pid: u32,
        workspace: Option<&'a serde_json::Value>,
    ) -> Option<(&'a serde_json::Value, &'a serde_json::Value)> {
        let workspace = if node.get("type").and_then(|kind| kind.as_str()) == Some("workspace") {
            Some(node)
        } else {
            workspace
        };
        if node.get("name").and_then(|name| name.as_str()) == Some(title)
            && node.get("pid").and_then(|pid| pid.as_u64()) == Some(u64::from(pid))
            && node
                .get("visible")
                .and_then(|visible| visible.as_bool())
                .unwrap_or(false)
        {
            return workspace.map(|workspace| (node, workspace));
        }
        for key in ["nodes", "floating_nodes"] {
            if let Some(children) = node.get(key).and_then(|children| children.as_array()) {
                for child in children {
                    if let Some(found) = walk(child, title, pid, workspace) {
                        return Some(found);
                    }
                }
            }
        }
        None
    }
    walk(node, title, pid, None)
}

fn get_tree() -> Option<serde_json::Value> {
    let output = Command::new("swaymsg")
        .args(["-t", "get_tree", "-r"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

fn is_visible_view(node: &serde_json::Value) -> bool {
    node.get("pid").and_then(|pid| pid.as_u64()).is_some()
        && node
            .get("visible")
            .and_then(|visible| visible.as_bool())
            .unwrap_or(false)
}

fn find_view_by_title<'a>(
    tree: &'a serde_json::Value,
    title: &str,
    pid: Option<u32>,
) -> Option<&'a serde_json::Value> {
    let mut found = None;
    walk(tree, &mut |node| {
        if found.is_some() || !is_visible_view(node) {
            return;
        }
        if node.get("name").and_then(|name| name.as_str()) != Some(title) {
            return;
        }
        let node_pid = node.get("pid").and_then(|pid| pid.as_u64());
        if pid.is_none_or(|pid| node_pid == Some(u64::from(pid))) {
            found = Some(node);
        }
    });
    found
}

fn node_rect(node: &serde_json::Value) -> Option<Rect> {
    let rect = node.get("rect")?;
    Some(Rect {
        x: rect.get("x")?.as_f64()? as f32,
        y: rect.get("y")?.as_f64()? as f32,
        width: rect.get("width")?.as_f64()? as f32,
        height: rect.get("height")?.as_f64()? as f32,
    })
}

fn walk<'a>(node: &'a serde_json::Value, visit: &mut impl FnMut(&'a serde_json::Value)) {
    visit(node);
    for key in ["nodes", "floating_nodes"] {
        if let Some(children) = node.get(key).and_then(|children| children.as_array()) {
            for child in children {
                walk(child, visit);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_uses_half_open_bounds() {
        let rect = Rect {
            x: 100.0,
            y: 50.0,
            width: 340.0,
            height: 680.0,
        };
        assert!(rect.contains(100.0, 50.0));
        assert!(rect.contains(439.0, 729.0));
        assert!(!rect.contains(440.0, 700.0));
        assert!(!rect.contains(99.0, 50.0));
        assert!(!rect.contains(200.0, 730.0));
    }

    #[test]
    fn right_edge_target_sits_flush_against_the_workspace() {
        let workspace = Rect {
            x: 0.0,
            y: 30.0,
            width: 1920.0,
            height: 1050.0,
        };
        let window = Rect {
            x: 500.0,
            y: 200.0,
            width: 340.0,
            height: 680.0,
        };
        assert_eq!(right_edge_target(workspace, window), (1580.0, 30.0));

        let second = Rect {
            x: 1920.0,
            y: 30.0,
            width: 1920.0,
            height: 1050.0,
        };
        assert_eq!(right_edge_target(second, window), (3500.0, 30.0));
    }

    #[test]
    fn finds_a_view_and_its_workspace() {
        let tree = serde_json::json!({
            "type": "root",
            "nodes": [{
                "type": "output",
                "name": "HEADLESS-1",
                "nodes": [{
                    "type": "workspace",
                    "name": "1",
                    "rect": {"x": 0.0, "y": 30.0, "width": 1920.0, "height": 1050.0},
                    "floating_nodes": [{
                        "type": "floating_con",
                        "nodes": [{
                            "type": "con",
                            "name": "Evermore feed",
                            "pid": 42,
                            "visible": true,
                            "rect": {"x": 1200.0, "y": 30.0, "width": 340.0, "height": 680.0}
                        }]
                    }]
                }]
            }]
        });
        let (view, workspace) = find_view_with_workspace(&tree, "Evermore feed", 42).unwrap();
        assert_eq!(node_rect(view).unwrap().x, 1200.0);
        assert_eq!(node_rect(workspace).unwrap().width, 1920.0);
        assert!(find_view_with_workspace(&tree, "Evermore feed", 7).is_none());
    }

    fn feed_tree(window_width: f64, window_x: f64) -> serde_json::Value {
        serde_json::json!({
            "type": "root",
            "nodes": [{
                "type": "output",
                "name": "HEADLESS-1",
                "nodes": [{
                    "type": "workspace",
                    "name": "1",
                    "rect": {"x": 0.0, "y": 0.0, "width": 1920.0, "height": 1080.0},
                    "floating_nodes": [{
                        "type": "floating_con",
                        "nodes": [{
                            "type": "con",
                            "id": 7,
                            "name": "Evermore feed",
                            "pid": 42,
                            "visible": true,
                            "rect": {"x": window_x, "y": 0.0, "width": window_width, "height": 680.0}
                        }]
                    }]
                }]
            }]
        })
    }

    #[test]
    fn stale_width_is_rejected_instead_of_computing_a_wrong_target() {
        // A regression test for exactly the bug this was fixed for: the
        // window actually just resized to 40px (collapsing), but a
        // `get_tree` snapshot taken before the compositor applied that
        // resize still reports the old 340px width. Computing the
        // right-edge target from that stale width, then considering the
        // job done, is what left the collapsed drawer sitting ~300px short
        // of the true edge -- this must be rejected, not silently used.
        let tree = feed_tree(340.0, 1580.0);
        let result = compute_right_edge_move(&tree, "Evermore feed", 42, 40.0);
        assert!(
            result.is_err(),
            "a stale (pre-resize) width must be rejected"
        );
    }

    #[test]
    fn matching_width_computes_the_right_edge_move() {
        // Once the reported width actually matches what was requested
        // (1600 = 1920 - 320, confirming it is NOT still using the old
        // 340px width), the target follows the same math as
        // `right_edge_target_sits_flush_against_the_workspace` above.
        let tree = feed_tree(40.0, 1200.0);
        let (con_id, x, y) = compute_right_edge_move(&tree, "Evermore feed", 42, 40.0)
            .unwrap()
            .expect("window is not yet at the target position");
        assert_eq!((con_id, x, y), (7, 1880, 0));
    }

    #[test]
    fn already_correctly_positioned_needs_no_move() {
        let tree = feed_tree(40.0, 1880.0);
        let move_needed = compute_right_edge_move(&tree, "Evermore feed", 42, 40.0).unwrap();
        assert!(move_needed.is_none());
    }

    #[test]
    fn finds_a_view_by_title_and_pid() {
        let tree = serde_json::json!({
            "type": "root",
            "nodes": [{
                "type": "output",
                "name": "HEADLESS-1",
                "nodes": [{
                    "type": "floating_con",
                    "nodes": [{
                        "type": "con",
                        "name": "Evermore feed",
                        "pid": 42,
                        "visible": true,
                        "rect": {"x": 1200.0, "y": 0.0, "width": 340.0, "height": 680.0}
                    }]
                }]
            }]
        });
        let node = find_view_by_title(&tree, "Evermore feed", Some(42)).expect("feed view");
        assert_eq!(node_rect(node).unwrap().x, 1200.0);
        assert!(find_view_by_title(&tree, "Evermore feed", Some(7)).is_none());
        assert!(find_view_by_title(&tree, "Evermore AI", None).is_none());
    }
}
