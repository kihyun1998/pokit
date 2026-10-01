//! A snapshot: the page's accessibility tree as text, with a ref beside every element an agent can act on.

use serde_json::Value;

pub struct Snapshot {
    pub text: String,
    /// Ref → backend DOM node id, in order of appearance.
    pub refs: Vec<(String, i64)>,
    /// The number the next snapshot's first ref takes.
    pub next_ref: u64,
}

/// Whether `s` names a ref (`e` followed by digits) rather than a CSS selector.
pub fn is_ref(s: &str) -> bool {
    s.len() > 1 && s.starts_with('e') && s[1..].chars().all(|c| c.is_ascii_digit())
}

/// Roles an agent acts on, whether or not the element reports itself focusable.
const ACTIONABLE: &[&str] = &[
    "button",
    "link",
    "textbox",
    "searchbox",
    "checkbox",
    "radio",
    "combobox",
    "listbox",
    "option",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "tab",
    "slider",
    "spinbutton",
    "switch",
    "treeitem",
];

/// Roles that carry no meaning of their own when they have no name; their children take their place.
const TRANSPARENT: &[&str] = &[
    "generic",
    "none",
    "LabelText",
    "paragraph",
    "group",
    "Section",
    "div",
];

/// Boolean properties shown after an element, in this order.
const STATES: &[&str] = &["focused", "disabled", "checked", "expanded", "selected"];

fn str_of<'a>(node: &'a Value, field: &str) -> &'a str {
    node[field]["value"].as_str().unwrap_or("")
}

fn property<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    node["properties"]
        .as_array()?
        .iter()
        .find(|p| p["name"] == name)
        .map(|p| &p["value"]["value"])
}

struct Renderer<'a> {
    by_id: std::collections::HashMap<&'a str, &'a Value>,
    text: String,
    refs: Vec<(String, i64)>,
    next_ref: u64,
}

impl<'a> Renderer<'a> {
    fn children(&self, node: &'a Value) -> Vec<&'a Value> {
        node["childIds"]
            .as_array()
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| id.as_str().and_then(|id| self.by_id.get(id).copied()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Writes `node` at `depth`; `parent_name` is the name of the nearest written ancestor.
    fn walk(&mut self, node: &'a Value, depth: usize, parent_name: &str) {
        let role = str_of(node, "role");
        let name = str_of(node, "name");
        let skip = node["ignored"] == true
            || role == "InlineTextBox"
            || (role == "StaticText" && (name.is_empty() || name == parent_name))
            || (TRANSPARENT.contains(&role) && name.is_empty());
        if skip {
            for child in self.children(node) {
                self.walk(child, depth, parent_name);
            }
            return;
        }

        let root = role == "RootWebArea";
        let mut line = format!(
            "{}- {}",
            "  ".repeat(depth),
            if root {
                "document"
            } else if role == "StaticText" {
                "text"
            } else {
                role
            }
        );
        if !name.is_empty() {
            line.push_str(&format!(" {}", serde_json::to_string(name).unwrap()));
        }
        let focusable = property(node, "focusable") == Some(&Value::Bool(true));
        if !root && (ACTIONABLE.contains(&role) || focusable) {
            if let Some(backend) = node["backendDOMNodeId"].as_i64() {
                let r = format!("e{}", self.next_ref);
                self.next_ref += 1;
                line.push_str(&format!(" [ref={r}]"));
                self.refs.push((r, backend));
            }
        }
        let value = str_of(node, "value");
        if !value.is_empty() {
            line.push_str(&format!(" value={}", serde_json::to_string(value).unwrap()));
        }
        if !root {
            for state in STATES {
                match property(node, state) {
                    Some(Value::Bool(true)) => line.push_str(&format!(" {state}")),
                    Some(Value::String(s)) if s == "true" || s == "mixed" => {
                        line.push_str(&format!(" {state}={s}"))
                    }
                    _ => {}
                }
            }
        }
        self.text.push_str(&line);
        self.text.push('\n');
        for child in self.children(node) {
            self.walk(child, depth + 1, name);
        }
    }
}

/// Renders `Accessibility.getFullAXTree` nodes, numbering refs from `first_ref`.
pub fn render(nodes: &[Value], first_ref: u64) -> Snapshot {
    let by_id = nodes
        .iter()
        .filter_map(|n| n["nodeId"].as_str().map(|id| (id, n)))
        .collect();
    let mut r = Renderer {
        by_id,
        text: String::new(),
        refs: Vec::new(),
        next_ref: first_ref,
    };
    for root in nodes.iter().filter(|n| n.get("parentId").is_none()) {
        r.walk(root, 0, "");
    }
    Snapshot {
        text: r.text,
        refs: r.refs,
        next_ref: r.next_ref,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixture's main page as `Accessibility.getFullAXTree` returned it, with the Name field focused.
    fn fixture_nodes() -> Vec<Value> {
        let tree: Value =
            serde_json::from_str(include_str!("../tests/data/ax-fixture-main.json")).unwrap();
        tree["nodes"].as_array().unwrap().clone()
    }

    #[test]
    fn the_fixture_page_renders_as_its_outline_with_refs_on_what_can_be_acted_on() {
        let snap = render(&fixture_nodes(), 1);
        let expected = "\
- document \"pokit fixture\"
  - heading \"pokit fixture\"
  - heading \"Form\"
  - text \"Name\"
  - textbox \"Name\" [ref=e1] focused
  - text \"Password\"
  - textbox \"Password\" [ref=e2]
  - button \"Submit\" [ref=e3]
  - status
  - heading \"Other\"
  - text \"Other input\"
  - textbox \"Other input\" [ref=e4]
  - heading \"Events\"
  - button \"Event target\" [ref=e5]
  - status
  - status
  - heading \"Actions\"
  - button \"Show later\" [ref=e6]
  - button \"Open second window\" [ref=e7]
  - button \"Throw\" [ref=e8]
  - button \"Backend log\" [ref=e9]
";
        assert_eq!(snap.text, expected);
    }

    #[test]
    fn each_ref_points_at_its_elements_backend_node() {
        let snap = render(&fixture_nodes(), 1);
        let refs: Vec<(&str, i64)> = snap.refs.iter().map(|(r, n)| (r.as_str(), *n)).collect();
        assert_eq!(
            refs,
            [
                ("e1", 1),
                ("e2", 2),
                ("e3", 22),
                ("e4", 3),
                ("e5", 29),
                ("e6", 34),
                ("e7", 35),
                ("e8", 36),
                ("e9", 37)
            ]
        );
    }

    #[test]
    fn a_later_snapshot_continues_the_numbering_so_no_ref_is_reused() {
        let first = render(&fixture_nodes(), 1);
        assert_eq!(first.next_ref, 10);
        let second = render(&fixture_nodes(), first.next_ref);
        assert_eq!(second.refs.first().map(|(r, _)| r.as_str()), Some("e10"));
        assert!(second.text.contains("textbox \"Name\" [ref=e10]"));
    }

    #[test]
    fn a_ref_is_e_followed_by_digits_and_anything_else_is_a_selector() {
        assert!(is_ref("e1") && is_ref("e123"));
        assert!(!is_ref("e") && !is_ref("#e1") && !is_ref("e1a") && !is_ref("button"));
    }
}
