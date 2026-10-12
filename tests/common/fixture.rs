//! Fixture layouts (what a test creates) and layout shapes (what Rex reports).

use std::path::Path;

use serde_json::{Map, Value, json};

pub const SHELL_FLAVOR: &str = "com.superlogical.terminal.shell";

/// A window layout to create, in `session.create` terms.
#[derive(Clone, Debug)]
pub enum Layout {
    Block {
        label: Option<String>,
        options: Map<String, Value>,
    },
    Split {
        direction: &'static str,
        ratio: f64,
        before: Box<Layout>,
        after: Box<Layout>,
    },
}

/// A shell block starting in `dir`.
pub fn shell(dir: impl AsRef<Path>) -> Layout {
    block(json!({ "cwd": dir.as_ref().to_str().unwrap() }))
}

/// A shell block with arbitrary terminal options (`cwd`, `command`,
/// `initial_input`, ...).
pub fn block(options: Value) -> Layout {
    let Value::Object(options) = options else {
        panic!("block options must be a JSON object");
    };
    Layout::Block {
        label: None,
        options,
    }
}

/// `before` on the left, `after` on the right; `before` gets `ratio` of the width.
pub fn hsplit(ratio: f64, before: Layout, after: Layout) -> Layout {
    split("horizontal", ratio, before, after)
}

/// `before` on top, `after` below; `before` gets `ratio` of the height.
pub fn vsplit(ratio: f64, before: Layout, after: Layout) -> Layout {
    split("vertical", ratio, before, after)
}

fn split(direction: &'static str, ratio: f64, before: Layout, after: Layout) -> Layout {
    Layout::Split {
        direction,
        ratio,
        before: Box::new(before),
        after: Box::new(after),
    }
}

impl Layout {
    /// Sets the block's label.
    pub fn labeled(self, label: impl Into<String>) -> Layout {
        match self {
            Layout::Block { options, .. } => Layout::Block {
                label: Some(label.into()),
                options,
            },
            Layout::Split { .. } => panic!("only blocks have labels"),
        }
    }

    pub fn to_json(&self) -> Value {
        match self {
            Layout::Block { label, options } => {
                let mut block = json!({ "flavor": SHELL_FLAVOR, "options": options });
                if let Some(label) = label {
                    block["label"] = json!(label);
                }
                json!({ "block": block })
            }
            Layout::Split {
                direction,
                ratio,
                before,
                after,
            } => json!({ "split": {
                "direction": direction,
                "ratio": ratio,
                "before": before.to_json(),
                "after": after.to_json(),
            }}),
        }
    }

    /// The shape Rex should report for this layout, each block standing for
    /// the `cwd` it was created in.
    pub fn expected_shape(&self) -> Shape {
        match self {
            Layout::Block { options, .. } => Shape::Block(
                options["cwd"]
                    .as_str()
                    .expect("expected_shape needs a cwd on every block")
                    .to_string(),
            ),
            Layout::Split {
                direction,
                ratio,
                before,
                after,
            } => Shape::Split {
                direction: direction.to_string(),
                ratio: *ratio,
                before: Box::new(before.expected_shape()),
                after: Box::new(after.expected_shape()),
            },
        }
    }
}

/// A window to create.
#[derive(Clone, Debug)]
pub struct Window {
    pub label: Option<String>,
    pub layout: Layout,
}

pub fn window(layout: Layout) -> Window {
    Window {
        label: None,
        layout,
    }
}

impl Window {
    pub fn labeled(mut self, label: impl Into<String>) -> Window {
        self.label = Some(label.into());
        self
    }

    pub fn to_json(&self) -> Value {
        let mut window = json!({ "layout": self.layout.to_json() });
        if let Some(label) = &self.label {
            window["window_label"] = json!(label);
        }
        window
    }
}

/// A fixture session as `session.create` returned it.
#[derive(Clone, Debug)]
pub struct FixtureSession {
    pub id: String,
    pub windows: Vec<FixtureWindow>,
}

#[derive(Clone, Debug)]
pub struct FixtureWindow {
    pub id: String,
    /// In depth-first, before-then-after order of the window's layout.
    pub block_ids: Vec<String>,
}

impl FixtureSession {
    pub(crate) fn from_create_response(created: &Value) -> FixtureSession {
        let windows = created["initial_windows"]
            .as_array()
            .map(|windows| {
                windows
                    .iter()
                    .map(|w| FixtureWindow {
                        id: w["window_id"].as_str().unwrap().to_string(),
                        block_ids: w["block_ids"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|b| b.as_str().unwrap().to_string())
                            .collect(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        FixtureSession {
            id: created["session_id"].as_str().unwrap().to_string(),
            windows,
        }
    }

    /// Every block, window by window, in layout order.
    pub fn block_ids(&self) -> Vec<&str> {
        self.windows
            .iter()
            .flat_map(|w| w.block_ids.iter().map(String::as_str))
            .collect()
    }
}

/// A layout as Rex reports it: the split tree with directions and ratios,
/// each block reduced to one string (its ID, or its working directory).
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Block(String),
    Split {
        direction: String,
        ratio: f64,
        before: Box<Shape>,
        after: Box<Shape>,
    },
}

impl Shape {
    /// Replaces each block's string with `f(string)`.
    pub fn map_blocks(self, f: &mut impl FnMut(String) -> String) -> Shape {
        match self {
            Shape::Block(s) => Shape::Block(f(s)),
            Shape::Split {
                direction,
                ratio,
                before,
                after,
            } => {
                let before = before.map_blocks(f);
                let after = after.map_blocks(f);
                Shape::Split {
                    direction,
                    ratio,
                    before: Box::new(before),
                    after: Box::new(after),
                }
            }
        }
    }

    /// Each block's string, in layout (depth-first, before-then-after) order.
    pub fn leaves(&self) -> Vec<String> {
        match self {
            Shape::Block(s) => vec![s.clone()],
            Shape::Split { before, after, .. } => {
                let mut leaves = before.leaves();
                leaves.extend(after.leaves());
                leaves
            }
        }
    }

    /// Replaces each block's path with its canonical form (`/tmp` ->
    /// `/private/tmp` on macOS), for comparing logical and physical paths.
    pub fn canonicalized(self) -> Shape {
        self.map_blocks(&mut |path| {
            std::fs::canonicalize(&path)
                .unwrap_or_else(|e| panic!("canonicalizing {path}: {e}"))
                .to_str()
                .unwrap()
                .to_string()
        })
    }
}

/// Each window's tiled layout from a `session.view` result, in window order,
/// with block IDs at the leaves. Floating layers are ignored.
pub fn tiled_layouts(view: &Value) -> Vec<Shape> {
    let windows = view["windows"].as_array().cloned().unwrap_or_default();
    windows
        .iter()
        .map(|w| {
            let tiled = w["layers"]
                .as_array()
                .unwrap()
                .iter()
                .find(|layer| layer["kind"] == "tiled")
                .expect("every window has a tiled layer");
            shape_of(&tiled["layout"])
        })
        .collect()
}

/// A window's labels as `session.view` reports them.
#[derive(Clone, Debug)]
pub struct WindowLabels {
    pub window_id: String,
    pub label: String,
    /// The tiled layout's block labels, in depth-first, before-then-after order.
    pub block_labels: Vec<String>,
}

/// Each window's labels from a `session.view` result, in window order.
pub fn labels(view: &Value) -> Vec<WindowLabels> {
    let windows = view["windows"].as_array().cloned().unwrap_or_default();
    windows
        .iter()
        .zip(tiled_layouts(view))
        .map(|(w, layout)| {
            let tiled = w["layers"]
                .as_array()
                .unwrap()
                .iter()
                .find(|layer| layer["kind"] == "tiled")
                .unwrap();
            let label_of = |block_id: &str| -> String {
                tiled["blocks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|b| b["block_id"] == block_id)
                    .unwrap_or_else(|| {
                        panic!("block {block_id} is in the layout but not in blocks[]")
                    })["label"]
                    .as_str()
                    .unwrap()
                    .to_string()
            };
            let mut block_labels = Vec::new();
            layout.map_blocks(&mut |block_id| {
                block_labels.push(label_of(&block_id));
                block_id
            });
            WindowLabels {
                window_id: w["window_id"].as_str().unwrap().to_string(),
                label: w["label"].as_str().unwrap().to_string(),
                block_labels,
            }
        })
        .collect()
}

/// The label Rex generates for a window nobody labeled: `Window ` plus the
/// last 8 hex characters of its ID.
pub fn default_window_label(window_id: &str) -> String {
    format!("Window {}", &window_id[window_id.len() - 8..])
}

fn shape_of(node: &Value) -> Shape {
    if let Some(split) = node.get("split") {
        Shape::Split {
            direction: split["direction"].as_str().unwrap().to_string(),
            ratio: split["ratio"].as_f64().unwrap(),
            before: Box::new(shape_of(&split["before"])),
            after: Box::new(shape_of(&split["after"])),
        }
    } else {
        Shape::Block(
            node["block_id"]
                .as_str()
                .unwrap_or_else(|| panic!("layout node is neither split nor block: {node}"))
                .to_string(),
        )
    }
}
