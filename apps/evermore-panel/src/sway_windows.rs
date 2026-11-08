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
pub fn anchor_to_right_edge(pid: u32) -> Result<(), String> {
    let tree = get_tree().ok_or_else(|| "swaymsg get_tree is unavailable".to_string())?;
    let (node, workspace) = find_view_with_workspace(&tree, "Evermore feed", pid)
        .ok_or_else(|| "feed window is not mapped yet".to_string())?;
    let window = node_rect(node).ok_or_else(|| "feed window has no rect".to_string())?;
    let workspace = node_rect(workspace).ok_or_else(|| "workspace has no rect".to_string())?;
    let con_id = node
        .get("id")
        .and_then(|id| id.as_i64())
        .ok_or_else(|| "feed window has no con_id".to_string())?;
    let (target_x, target_y) = right_edge_target(workspace, window);
    if (window.x - target_x).abs() < 1.0 && (window.y - target_y).abs() < 1.0 {
        return Ok(());
    }
    let command = format!(
        "[con_id={con_id}] move absolute position {} {}",
        target_x.round() as i64,
        target_y.round() as i64
    );
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

fn right_edge_target(workspace: Rect, window: Rect) -> (f32, f32) {
    (workspace.x + workspace.width - window.width, workspace.y)
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
