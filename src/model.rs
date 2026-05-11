use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InstanceNode {
    pub name: String,
    pub class_name: String,
    pub path: String,
    pub source_kind: SourceKind,
    pub source_path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<InstanceNode>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    Virtual,
    Directory,
    File,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NodeMetadata {
    #[serde(default)]
    pub class_name: Option<String>,
    #[serde(default)]
    pub properties: BTreeMap<String, Value>,
    #[serde(default)]
    pub attributes: BTreeMap<String, Value>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeSummary {
    pub total_nodes: usize,
    pub class_counts: BTreeMap<String, usize>,
}

impl InstanceNode {
    pub fn new(
        name: impl Into<String>,
        class_name: impl Into<String>,
        path: impl Into<String>,
        source_kind: SourceKind,
        source_path: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            class_name: class_name.into(),
            path: path.into(),
            source_kind,
            source_path,
            children: Vec::new(),
            properties: BTreeMap::new(),
            attributes: BTreeMap::new(),
            tags: Vec::new(),
            source: None,
            document: None,
        }
    }

    pub fn apply_metadata(&mut self, metadata: NodeMetadata) {
        if let Some(class_name) = metadata.class_name {
            self.class_name = class_name;
        }

        self.properties.extend(metadata.properties);
        self.attributes.extend(metadata.attributes);

        if !metadata.tags.is_empty() {
            self.tags = metadata.tags;
        }
    }

    pub fn pretty_print(&self) -> String {
        let mut out = String::new();
        self.pretty_print_into(0, &mut out);
        out
    }

    fn pretty_print_into(&self, depth: usize, out: &mut String) {
        let indent = "  ".repeat(depth);
        let source = match self.source_kind {
            SourceKind::Virtual => "virtual",
            SourceKind::Directory => "dir",
            SourceKind::File => "file",
        };

        if depth == 0 {
            out.push_str(&format!("{} [{}]\n", self.name, self.class_name));
        } else {
            out.push_str(&format!(
                "{}- {} [{}] <{}>\n",
                indent, self.name, self.class_name, source
            ));
        }

        for child in &self.children {
            child.pretty_print_into(depth + 1, out);
        }
    }
}

pub fn summarize_tree(root: &InstanceNode) -> TreeSummary {
    fn visit(node: &InstanceNode, total: &mut usize, counts: &mut BTreeMap<String, usize>) {
        *total += 1;
        *counts.entry(node.class_name.clone()).or_insert(0) += 1;

        for child in &node.children {
            visit(child, total, counts);
        }
    }

    let mut total_nodes = 0;
    let mut class_counts = BTreeMap::new();
    visit(root, &mut total_nodes, &mut class_counts);

    TreeSummary {
        total_nodes,
        class_counts,
    }
}
