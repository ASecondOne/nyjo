use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::model::{InstanceNode, NodeMetadata, SourceKind};

const TOP_LEVEL_SERVICES: &[&str] = &[
    "Lighting",
    "ReplicatedFirst",
    "ReplicatedStorage",
    "ServerStorage",
    "ServerScriptService",
    "StarterPlayer",
    "StarterGui",
    "SoundService",
    "Teams",
    "TextChatService",
    "Workspace",
];

const SPECIAL_DIRECTORY_CLASSES: &[(&str, &str)] = &[
    ("StarterPlayer", "StarterPlayerScripts"),
    ("StarterPlayer", "StarterCharacterScripts"),
];

const SYSTEM_NOISE: &[&str] = &[".DS_Store", "Thumbs.db"];
const DIRECTORY_SCRIPT_SOURCE_FILES: &[(&str, &str)] = &[
    ("Script", "init.server.lua"),
    ("LocalScript", "init.client.lua"),
    ("ModuleScript", "init.lua"),
];
const EMBEDDED_HEADER_MARKER: &str = "--!nyjo";
const EMBEDDED_HEADER_START: &str = "--HEADER";
const EMBEDDED_CONTENTS_START: &str = "--CONTENTS";

struct ExtensionSpec {
    suffix: &'static str,
    default_class: &'static str,
    payload: PayloadKind,
}

#[derive(Clone, Copy)]
enum PayloadKind {
    Empty,
    Text,
    Json,
    InstanceJson,
    StructuredInstance,
    ServiceJson,
}

const EXTENSION_SPECS: &[ExtensionSpec] = &[
    ExtensionSpec {
        suffix: ".server.lua",
        default_class: "Script",
        payload: PayloadKind::Text,
    },
    ExtensionSpec {
        suffix: ".client.lua",
        default_class: "LocalScript",
        payload: PayloadKind::Text,
    },
    ExtensionSpec {
        suffix: ".model.json",
        default_class: "Model",
        payload: PayloadKind::Json,
    },
    ExtensionSpec {
        suffix: ".instance.json",
        default_class: "Folder",
        payload: PayloadKind::InstanceJson,
    },
    ExtensionSpec {
        suffix: ".worldmodel",
        default_class: "WorldModel",
        payload: PayloadKind::StructuredInstance,
    },
    ExtensionSpec {
        suffix: ".model",
        default_class: "Model",
        payload: PayloadKind::StructuredInstance,
    },
    ExtensionSpec {
        suffix: ".part",
        default_class: "Part",
        payload: PayloadKind::StructuredInstance,
    },
    ExtensionSpec {
        suffix: ".service.json",
        default_class: "",
        payload: PayloadKind::ServiceJson,
    },
    ExtensionSpec {
        suffix: ".folder",
        default_class: "Folder",
        payload: PayloadKind::Empty,
    },
    ExtensionSpec {
        suffix: ".lua",
        default_class: "ModuleScript",
        payload: PayloadKind::Text,
    },
    ExtensionSpec {
        suffix: ".rf",
        default_class: "RemoteFunction",
        payload: PayloadKind::Empty,
    },
    ExtensionSpec {
        suffix: ".re",
        default_class: "RemoteEvent",
        payload: PayloadKind::Empty,
    },
    ExtensionSpec {
        suffix: ".bf",
        default_class: "BindableFunction",
        payload: PayloadKind::Empty,
    },
    ExtensionSpec {
        suffix: ".be",
        default_class: "BindableEvent",
        payload: PayloadKind::Empty,
    },
];

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InlineNodeDefinition {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    class_name: Option<String>,
    #[serde(default)]
    properties: BTreeMap<String, Value>,
    #[serde(default)]
    attributes: BTreeMap<String, Value>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default, deserialize_with = "deserialize_inline_children")]
    children: Vec<InlineNodeDefinition>,
}

#[derive(Debug, Clone)]
pub struct EmbeddedTextPayload {
    pub has_header: bool,
    pub metadata: Option<NodeMetadata>,
    pub contents: String,
}

pub fn scan_project(root: &Path) -> Result<InstanceNode> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to resolve project root {}", root.display()))?;
    let context = ParseContext::new(root)?;
    context.parse_root()
}

struct ParseContext {
    root: PathBuf,
    ignore: IgnoreMatcher,
}

impl ParseContext {
    fn new(root: PathBuf) -> Result<Self> {
        let ignore = IgnoreMatcher::from_root(&root)?;
        Ok(Self { root, ignore })
    }

    fn parse_root(&self) -> Result<InstanceNode> {
        let project_name = self
            .root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Project")
            .to_string();

        let mut root = InstanceNode::new(
            project_name,
            "DataModel",
            "",
            SourceKind::Virtual,
            Some(self.relative_string(&self.root)),
        );

        root.children = self.parse_directory_children(&self.root, None, true, None)?;

        if let Some(metadata) = load_directory_metadata(&self.root)? {
            root.apply_metadata(metadata);
        }

        Ok(root)
    }

    fn parse_directory_children(
        &self,
        directory: &Path,
        parent_class: Option<&str>,
        top_level: bool,
        skip_names: Option<&HashSet<String>>,
    ) -> Result<Vec<InstanceNode>> {
        let mut entries = fs::read_dir(directory)
            .with_context(|| format!("failed to read directory {}", directory.display()))?
            .collect::<std::io::Result<Vec<_>>>()
            .with_context(|| format!("failed to enumerate directory {}", directory.display()))?;

        entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_string());

        let mut children = Vec::new();
        for entry in entries {
            let path = entry.path();
            let file_type = entry
                .file_type()
                .with_context(|| format!("failed to read file type for {}", path.display()))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();

            if skip_names.is_some_and(|skipped| skipped.contains(name.as_ref())) {
                continue;
            }

            if should_skip_special_file(&name) {
                continue;
            }

            if self.should_skip_entry(&path, &name, file_type.is_dir()) {
                continue;
            }

            let parsed = if file_type.is_dir() {
                Some(self.parse_directory(&path, parent_class, top_level)?)
            } else if file_type.is_file() {
                self.parse_file(&path)?
            } else {
                None
            };

            if let Some(node) = parsed {
                children.push(node);
            }
        }

        Ok(children)
    }

    fn parse_directory(
        &self,
        path: &Path,
        parent_class: Option<&str>,
        top_level: bool,
    ) -> Result<InstanceNode> {
        let name = file_name(path)?;
        let mut metadata = load_directory_metadata(path)?;
        let base_class_name = special_directory_class(parent_class, top_level, &name)
            .unwrap_or("Folder")
            .to_string();
        let mut class_name = metadata
            .as_ref()
            .and_then(|metadata| metadata.class_name.clone())
            .unwrap_or(base_class_name);

        let source_file = detect_directory_source_file(path, Some(&class_name))?;
        if metadata.is_none()
            && let Some((detected_class_name, _)) = source_file
        {
            class_name = detected_class_name.to_string();
        }

        let mut node = InstanceNode::new(
            name,
            class_name,
            self.relative_string(path),
            SourceKind::Directory,
            Some(self.relative_string(path)),
        );

        let mut skip_names = HashSet::new();
        if let Some((detected_class_name, source_file_name)) = source_file {
            let source_path = path.join(source_file_name);
            let raw_source = fs::read_to_string(&source_path).with_context(|| {
                format!(
                    "failed to read directory-backed script source {}",
                    source_path.display()
                )
            })?;
            let parsed_source = parse_embedded_text_payload(&source_path, &raw_source)?;

            if metadata.is_none() {
                node.class_name = detected_class_name.to_string();
            }
            if let Some(embedded_metadata) = parsed_source.metadata {
                node.apply_metadata(embedded_metadata);
            }
            node.source = Some(parsed_source.contents);
            skip_names.insert(source_file_name.to_string());
        }

        node.children =
            self.parse_directory_children(path, Some(&node.class_name), false, Some(&skip_names))?;

        if let Some(directory_metadata) = metadata.take() {
            node.apply_metadata(directory_metadata);
        }

        Ok(node)
    }

    fn parse_file(&self, path: &Path) -> Result<Option<InstanceNode>> {
        let file_name = file_name(path)?;
        let Some(spec) = extension_spec(&file_name) else {
            return Ok(None);
        };

        let name = file_name
            .strip_suffix(spec.suffix)
            .filter(|candidate| !candidate.is_empty())
            .map(ToOwned::to_owned)
            .with_context(|| {
                format!(
                    "failed to derive node name from file {} using suffix {}",
                    path.display(),
                    spec.suffix
                )
            })?;

        let class_name = match spec.payload {
            PayloadKind::ServiceJson => name.clone(),
            _ => spec.default_class.to_string(),
        };

        let relative_path = self.relative_string(path);
        let mut node = InstanceNode::new(
            name.clone(),
            class_name,
            relative_path.clone(),
            SourceKind::File,
            Some(relative_path.clone()),
        );

        match spec.payload {
            PayloadKind::Empty => {
                let raw_contents = fs::read_to_string(path)
                    .with_context(|| format!("failed to read marker file {}", path.display()))?;
                let parsed_payload = parse_embedded_text_payload(path, &raw_contents)?;
                if parsed_payload.has_header && !parsed_payload.contents.trim().is_empty() {
                    bail!(
                        "marker file {} cannot store body contents after {}",
                        path.display(),
                        EMBEDDED_CONTENTS_START
                    );
                }
                if let Some(embedded_metadata) = parsed_payload.metadata {
                    node.apply_metadata(embedded_metadata);
                }
            }
            PayloadKind::Text => {
                let raw_source = fs::read_to_string(path)
                    .with_context(|| format!("failed to read script file {}", path.display()))?;
                let parsed_source = parse_embedded_text_payload(path, &raw_source)?;
                if let Some(embedded_metadata) = parsed_source.metadata {
                    node.apply_metadata(embedded_metadata);
                }
                node.source = Some(parsed_source.contents);
            }
            PayloadKind::Json | PayloadKind::InstanceJson | PayloadKind::ServiceJson => {
                let contents = fs::read_to_string(path)
                    .with_context(|| format!("failed to read json file {}", path.display()))?;
                let document = parse_json_value(path, &contents)?;
                if supports_inline_definition(spec.payload) && is_inline_node_document(&document) {
                    let definition = parse_inline_node_definition(path, &document)?;
                    apply_inline_definition(&mut node, definition, &relative_path)?;
                }
                node.document = Some(document);
            }
            PayloadKind::StructuredInstance => {
                let raw_contents = fs::read_to_string(path).with_context(|| {
                    format!("failed to read structured instance file {}", path.display())
                })?;
                let parsed_payload = parse_embedded_text_payload(path, &raw_contents)?;
                if let Some(embedded_metadata) = parsed_payload.metadata {
                    node.apply_metadata(embedded_metadata);
                }
                if !parsed_payload.contents.trim().is_empty() {
                    let document = parse_json_value(path, &parsed_payload.contents)?;
                    node.children = parse_structured_instance_children(
                        path,
                        document.clone(),
                        &relative_path,
                        &name,
                    )?;
                    node.document = Some(document);
                }
            }
        }

        if let Some(metadata) = load_file_metadata(path, &name)? {
            node.apply_metadata(metadata);
        }

        Ok(Some(node))
    }

    fn should_skip_entry(&self, path: &Path, name: &str, is_dir: bool) -> bool {
        if SYSTEM_NOISE.contains(&name) {
            return true;
        }

        if name.starts_with('.') {
            return true;
        }

        self.ignore.is_ignored(path, is_dir)
    }

    fn relative_string(&self, path: &Path) -> String {
        relative_string(&self.root, path)
    }
}

struct IgnoreMatcher {
    gitignore: Option<Gitignore>,
}

impl IgnoreMatcher {
    fn from_root(root: &Path) -> Result<Self> {
        let ignore_path = root.join(".nyjoignore");
        if !ignore_path.exists() {
            return Ok(Self { gitignore: None });
        }

        let mut builder = GitignoreBuilder::new(root);
        builder.add(&ignore_path);
        let gitignore = builder
            .build()
            .with_context(|| format!("failed to parse {}", ignore_path.display()))?;

        Ok(Self {
            gitignore: Some(gitignore),
        })
    }

    fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        self.gitignore
            .as_ref()
            .is_some_and(|matcher| matcher.matched(path, is_dir).is_ignore())
    }
}

fn load_directory_metadata(directory: &Path) -> Result<Option<NodeMetadata>> {
    let metadata_path = directory.join(".meta.json");
    load_metadata_path(&metadata_path)
}

fn load_file_metadata(file: &Path, base_name: &str) -> Result<Option<NodeMetadata>> {
    let parent = file
        .parent()
        .with_context(|| format!("file {} is missing a parent directory", file.display()))?;
    let file_name = file_name(file)?;

    let mut candidates = Vec::new();
    candidates.push(parent.join(format!("{file_name}.meta.json")));
    candidates.push(parent.join(format!("{base_name}.meta.json")));

    let mut seen = HashSet::new();
    for candidate in candidates {
        if seen.insert(candidate.clone()) {
            if let Some(metadata) = load_metadata_path(&candidate)? {
                return Ok(Some(metadata));
            }
        }
    }

    Ok(None)
}

fn load_metadata_path(path: &Path) -> Result<Option<NodeMetadata>> {
    if !path.exists() {
        return Ok(None);
    }

    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read metadata file {}", path.display()))?;
    let metadata = serde_json::from_str::<NodeMetadata>(&contents)
        .with_context(|| format!("failed to parse metadata file {}", path.display()))?;
    Ok(Some(metadata))
}

fn parse_embedded_text_payload(path: &Path, raw_contents: &str) -> Result<EmbeddedTextPayload> {
    let mut remaining = raw_contents;
    let Some(marker_line) = consume_line(&mut remaining) else {
        return Ok(EmbeddedTextPayload {
            has_header: false,
            metadata: None,
            contents: String::new(),
        });
    };

    if trim_line(marker_line) != EMBEDDED_HEADER_MARKER {
        return Ok(EmbeddedTextPayload {
            has_header: false,
            metadata: None,
            contents: raw_contents.to_string(),
        });
    }

    let header_start = consume_line(&mut remaining).with_context(|| {
        format!(
            "missing {} block in {}",
            EMBEDDED_HEADER_START,
            path.display()
        )
    })?;
    if trim_line(header_start) != EMBEDDED_HEADER_START {
        bail!(
            "expected {} after {} in {}",
            EMBEDDED_HEADER_START,
            EMBEDDED_HEADER_MARKER,
            path.display()
        );
    }

    let mut header_lines = Vec::new();
    loop {
        let line = consume_line(&mut remaining).with_context(|| {
            format!(
                "missing {} block terminator in {}",
                EMBEDDED_CONTENTS_START,
                path.display()
            )
        })?;
        let trimmed = trim_line(line);
        if trimmed == EMBEDDED_CONTENTS_START {
            break;
        }

        let header_line = trimmed.strip_prefix("--").with_context(|| {
            format!(
                "embedded header lines in {} must begin with --",
                path.display()
            )
        })?;
        header_lines.push(
            header_line
                .strip_prefix(' ')
                .unwrap_or(header_line)
                .to_string(),
        );
    }

    let header_text = header_lines.join("\n");
    let metadata = if header_text.trim().is_empty() {
        None
    } else {
        Some(
            serde_json::from_str::<NodeMetadata>(&header_text).with_context(|| {
                format!(
                    "failed to parse embedded header metadata in {}",
                    path.display()
                )
            })?,
        )
    };

    Ok(EmbeddedTextPayload {
        has_header: true,
        metadata,
        contents: remaining.to_string(),
    })
}

pub fn read_embedded_text_payload(path: &Path, raw_contents: &str) -> Result<EmbeddedTextPayload> {
    parse_embedded_text_payload(path, raw_contents)
}

fn parse_json_value(path: &Path, contents: &str) -> Result<Value> {
    serde_json::from_str(contents)
        .with_context(|| format!("failed to parse json content in {}", path.display()))
}

fn parse_structured_instance_children(
    path: &Path,
    document: Value,
    source_path: &str,
    root_logical_name: &str,
) -> Result<Vec<InstanceNode>> {
    let child_value = match document {
        Value::Object(mut map) => {
            if let Some(children) = map.remove("children") {
                children
            } else {
                Value::Object(map)
            }
        }
        Value::Array(items) => Value::Array(items),
        other => {
            bail!(
                "structured instance contents in {} must be a JSON object, array, or {{\"children\": ...}}, got {}",
                path.display(),
                other
            )
        }
    };

    inline_children_from_value(child_value)?
        .into_iter()
        .map(|child| build_inline_child_node(child, source_path, root_logical_name))
        .collect()
}

fn supports_inline_definition(payload: PayloadKind) -> bool {
    matches!(
        payload,
        PayloadKind::Json | PayloadKind::InstanceJson | PayloadKind::ServiceJson
    )
}

fn is_inline_node_document(document: &Value) -> bool {
    const INLINE_NODE_KEYS: &[&str] = &[
        "name",
        "className",
        "properties",
        "attributes",
        "tags",
        "source",
        "children",
    ];

    match document {
        Value::Object(map) => INLINE_NODE_KEYS.iter().any(|key| map.contains_key(*key)),
        _ => false,
    }
}

fn parse_inline_node_definition(path: &Path, document: &Value) -> Result<InlineNodeDefinition> {
    serde_json::from_value(document.clone()).with_context(|| {
        format!(
            "failed to parse inline node definition in {}",
            path.display()
        )
    })
}

fn apply_inline_definition(
    node: &mut InstanceNode,
    definition: InlineNodeDefinition,
    source_path: &str,
) -> Result<()> {
    node.class_name = resolve_inline_class_name(
        definition.class_name,
        Some(node.class_name.as_str()),
        &node.name,
        source_path,
        definition.source.is_some(),
    )?;
    node.properties.extend(definition.properties);
    node.attributes.extend(definition.attributes);
    if !definition.tags.is_empty() {
        node.tags = definition.tags;
    }
    if definition.source.is_some() {
        node.source = definition.source;
    }
    node.children = definition
        .children
        .into_iter()
        .map(|child| build_inline_child_node(child, source_path, &node.name))
        .collect::<Result<Vec<_>>>()?;
    Ok(())
}

fn build_inline_child_node(
    definition: InlineNodeDefinition,
    source_path: &str,
    logical_path: &str,
) -> Result<InstanceNode> {
    let name = definition
        .name
        .filter(|name| !name.trim().is_empty())
        .with_context(|| {
            format!(
                "inline child node in {} is missing a non-empty name",
                source_path
            )
        })?;
    let class_name = resolve_inline_class_name(
        definition.class_name,
        None,
        &name,
        source_path,
        definition.source.is_some(),
    )?;
    let child_logical_path = format!("{logical_path}/{name}");
    let virtual_path = format!("{source_path}#{child_logical_path}");

    let mut node = InstanceNode::new(
        name,
        class_name,
        virtual_path,
        SourceKind::File,
        Some(source_path.to_string()),
    );
    node.properties = definition.properties;
    node.attributes = definition.attributes;
    node.tags = definition.tags;
    node.source = definition.source;
    node.children = definition
        .children
        .into_iter()
        .map(|child| build_inline_child_node(child, source_path, &child_logical_path))
        .collect::<Result<Vec<_>>>()?;
    Ok(node)
}

fn resolve_inline_class_name(
    explicit_class_name: Option<String>,
    fallback_class_name: Option<&str>,
    node_name: &str,
    source_path: &str,
    has_source: bool,
) -> Result<String> {
    if let Some(class_name) = explicit_class_name.filter(|class_name| !class_name.trim().is_empty())
    {
        return Ok(class_name);
    }

    if let Some(class_name) = fallback_class_name.filter(|class_name| !class_name.trim().is_empty())
    {
        return Ok(class_name.to_string());
    }

    if has_source {
        bail!(
            "inline node {} in {} declares source but no className",
            node_name,
            source_path
        );
    }

    Ok("Folder".to_string())
}

fn extension_spec(file_name: &str) -> Option<&'static ExtensionSpec> {
    EXTENSION_SPECS
        .iter()
        .find(|spec| file_name.ends_with(spec.suffix))
}

fn consume_line<'a>(input: &mut &'a str) -> Option<&'a str> {
    if input.is_empty() {
        return None;
    }

    if let Some(position) = input.find('\n') {
        let (line, rest) = input.split_at(position + 1);
        *input = rest;
        Some(line)
    } else {
        let line = *input;
        *input = "";
        Some(line)
    }
}

fn trim_line(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}

fn deserialize_inline_children<'de, D>(
    deserializer: D,
) -> Result<Vec<InlineNodeDefinition>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    inline_children_from_value(value).map_err(|error| serde::de::Error::custom(error.to_string()))
}

fn inline_children_from_value(value: Value) -> Result<Vec<InlineNodeDefinition>> {
    match value {
        Value::Null => Ok(Vec::new()),
        Value::Array(items) => items
            .into_iter()
            .map(|item| serde_json::from_value(item).map_err(anyhow::Error::from))
            .collect(),
        Value::Object(map) => {
            let mut children = Vec::new();
            for (name, child_value) in map {
                let mut child: InlineNodeDefinition =
                    serde_json::from_value(child_value).map_err(anyhow::Error::from)?;
                if child.name.is_none() {
                    child.name = Some(name);
                }
                children.push(child);
            }
            Ok(children)
        }
        other => bail!("expected children to be an array, object, or null, got {other}"),
    }
}

fn detect_directory_source_file(
    directory: &Path,
    current_class_name: Option<&str>,
) -> Result<Option<(&'static str, &'static str)>> {
    if let Some(class_name) = current_class_name {
        if let Some((expected_class_name, source_file_name)) = DIRECTORY_SCRIPT_SOURCE_FILES
            .iter()
            .find(|(expected_class_name, source_file_name)| {
                *expected_class_name == class_name && directory.join(source_file_name).is_file()
            })
            .copied()
        {
            return Ok(Some((expected_class_name, source_file_name)));
        }
    }

    let matches = DIRECTORY_SCRIPT_SOURCE_FILES
        .iter()
        .filter(|(_, source_file_name)| directory.join(source_file_name).is_file())
        .copied()
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [] => Ok(None),
        [single] => Ok(Some(*single)),
        _ => bail!(
            "directory {} contains multiple init script files; keep only one of init.server.lua, init.client.lua, or init.lua",
            directory.display()
        ),
    }
}

fn special_directory_class<'a>(
    parent_class: Option<&str>,
    top_level: bool,
    name: &'a str,
) -> Option<&'a str> {
    if top_level && TOP_LEVEL_SERVICES.contains(&name) {
        return Some(name);
    }

    parent_class.and_then(|parent| {
        SPECIAL_DIRECTORY_CLASSES
            .iter()
            .find(|(expected_parent, expected_name)| {
                parent == *expected_parent && name == *expected_name
            })
            .map(|(_, class_name)| *class_name)
    })
}

fn should_skip_special_file(name: &str) -> bool {
    name == ".nyjoignore" || name == ".meta.json" || name.ends_with(".meta.json")
}

fn file_name(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .with_context(|| format!("path {} is missing a valid utf-8 file name", path.display()))
}

fn relative_string(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(relative) => {
            let joined = relative
                .components()
                .filter_map(|component| component.as_os_str().to_str())
                .collect::<Vec<_>>()
                .join("/");
            joined
        }
        Err(_) => path.display().to_string(),
    }
}

pub fn metadata_path_for_target(path: &Path) -> Result<PathBuf> {
    if path.is_dir() {
        return Ok(path.join(".meta.json"));
    }

    let parent = path
        .parent()
        .with_context(|| format!("path {} is missing a parent directory", path.display()))?;
    let file_name = file_name(path)?;
    let Some(spec) = extension_spec(&file_name) else {
        bail!(
            "cannot attach metadata to unsupported file {}",
            path.display()
        );
    };
    let base_name = file_name
        .strip_suffix(spec.suffix)
        .filter(|candidate| !candidate.is_empty())
        .with_context(|| format!("failed to derive metadata base name for {}", path.display()))?;
    Ok(parent.join(format!("{base_name}.meta.json")))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use anyhow::Result;
    use tempfile::tempdir;

    use super::scan_project;

    #[test]
    fn parses_supported_entries_and_services() -> Result<()> {
        let dir = tempdir()?;
        write_file(
            dir.path(),
            "ReplicatedStorage/Shared.lua",
            "return { value = 1 }",
        )?;
        write_file(
            dir.path(),
            "ServerScriptService/Boot.server.lua",
            "print('server')",
        )?;
        write_file(
            dir.path(),
            "StarterPlayer/StarterPlayerScripts/Hud.client.lua",
            "print('client')",
        )?;
        write_file(dir.path(), "Workspace/Signal.re", "")?;
        write_file(dir.path(), "ReplicatedStorage/Empty.folder", "")?;

        let tree = scan_project(dir.path())?;

        let replicated = child(&tree, "ReplicatedStorage");
        assert_eq!(replicated.class_name, "ReplicatedStorage");

        let shared = child(replicated, "Shared");
        assert_eq!(shared.class_name, "ModuleScript");
        assert_eq!(shared.source.as_deref(), Some("return { value = 1 }"));

        let empty = child(replicated, "Empty");
        assert_eq!(empty.class_name, "Folder");

        let server_service = child(&tree, "ServerScriptService");
        let boot = child(server_service, "Boot");
        assert_eq!(boot.class_name, "Script");

        let starter_player = child(&tree, "StarterPlayer");
        let player_scripts = child(starter_player, "StarterPlayerScripts");
        assert_eq!(player_scripts.class_name, "StarterPlayerScripts");
        let hud = child(player_scripts, "Hud");
        assert_eq!(hud.class_name, "LocalScript");

        let workspace = child(&tree, "Workspace");
        let signal = child(workspace, "Signal");
        assert_eq!(signal.class_name, "RemoteEvent");

        Ok(())
    }

    #[test]
    fn parses_directory_backed_script_with_children() -> Result<()> {
        let dir = tempdir()?;
        write_file(
            dir.path(),
            "ServerScriptService/Main/init.server.lua",
            r#"--!nyjo
--HEADER
--{
--  "attributes": {
--    "Role": "Bootstrap"
--  }
--}
--CONTENTS
print('main')"#,
        )?;
        write_file(
            dir.path(),
            "ServerScriptService/Main/EnemyHandler.lua",
            "return {}",
        )?;

        let tree = scan_project(dir.path())?;
        let sss = child(&tree, "ServerScriptService");
        let main = child(sss, "Main");
        assert_eq!(main.class_name, "Script");
        assert_eq!(main.source.as_deref(), Some("print('main')"));
        assert_eq!(
            main.attributes.get("Role").and_then(|value| value.as_str()),
            Some("Bootstrap")
        );
        let enemy_handler = child(main, "EnemyHandler");
        assert_eq!(enemy_handler.class_name, "ModuleScript");

        Ok(())
    }

    #[test]
    fn parses_script_file_with_embedded_header_metadata() -> Result<()> {
        let dir = tempdir()?;
        write_file(
            dir.path(),
            "ServerScriptService/Boot.server.lua",
            r#"--!nyjo
--HEADER
--{
--  "properties": {
--    "Disabled": false
--  },
--  "attributes": {
--    "BootOrder": 1
--  },
--  "tags": ["Bootstrap"]
--}
--CONTENTS
print("boot")"#,
        )?;

        let tree = scan_project(dir.path())?;
        let service = child(&tree, "ServerScriptService");
        let boot = child(service, "Boot");

        assert_eq!(boot.class_name, "Script");
        assert_eq!(boot.source.as_deref(), Some("print(\"boot\")"));
        assert_eq!(
            boot.properties
                .get("Disabled")
                .and_then(|value| value.as_bool()),
            Some(false)
        );
        assert_eq!(
            boot.attributes
                .get("BootOrder")
                .and_then(|value| value.as_i64()),
            Some(1)
        );
        assert_eq!(boot.tags, vec!["Bootstrap"]);

        Ok(())
    }

    #[test]
    fn parses_marker_file_with_embedded_header_metadata() -> Result<()> {
        let dir = tempdir()?;
        write_file(
            dir.path(),
            "ReplicatedStorage/Ping.re",
            r#"--!nyjo
--HEADER
--{
--  "attributes": {
--    "Channel": "Gameplay"
--  },
--  "tags": ["Networked"]
--}
--CONTENTS
"#,
        )?;

        let tree = scan_project(dir.path())?;
        let replicated = child(&tree, "ReplicatedStorage");
        let ping = child(replicated, "Ping");

        assert_eq!(ping.class_name, "RemoteEvent");
        assert_eq!(
            ping.attributes
                .get("Channel")
                .and_then(|value| value.as_str()),
            Some("Gameplay")
        );
        assert_eq!(ping.tags, vec!["Networked"]);

        Ok(())
    }

    #[test]
    fn respects_ignore_rules_and_hidden_files() -> Result<()> {
        let dir = tempdir()?;
        write_file(dir.path(), ".nyjoignore", "Ignored/\n*.rf\n")?;
        write_file(dir.path(), "Ignored/ShouldNotAppear.lua", "return false")?;
        write_file(dir.path(), ".git/Hidden.lua", "return false")?;
        write_file(dir.path(), "Visible/Keep.lua", "return true")?;
        write_file(dir.path(), "Visible/Skip.rf", "")?;

        let tree = scan_project(dir.path())?;
        assert!(tree.children.iter().any(|child| child.name == "Visible"));
        assert!(tree.children.iter().all(|child| child.name != "Ignored"));

        let visible = child(&tree, "Visible");
        assert!(visible.children.iter().any(|child| child.name == "Keep"));
        assert!(visible.children.iter().all(|child| child.name != "Skip"));

        Ok(())
    }

    #[test]
    fn applies_metadata_overrides() -> Result<()> {
        let dir = tempdir()?;
        write_file(dir.path(), "Workspace/Enemy.folder", "")?;
        write_file(
            dir.path(),
            "Workspace/Enemy.meta.json",
            r#"{
  "className": "Part",
  "properties": { "Anchored": true, "Name": "EnemyPart" },
  "attributes": { "Damage": 10 },
  "tags": ["Enemy"]
}"#,
        )?;

        let tree = scan_project(dir.path())?;
        let workspace = child(&tree, "Workspace");
        let enemy = child(workspace, "Enemy");

        assert_eq!(enemy.class_name, "Part");
        assert_eq!(
            enemy
                .properties
                .get("Anchored")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
        assert_eq!(
            enemy
                .attributes
                .get("Damage")
                .and_then(|value| value.as_i64()),
            Some(10)
        );
        assert_eq!(enemy.tags, vec!["Enemy"]);

        Ok(())
    }

    #[test]
    fn parses_compact_service_ui_tree_from_single_file() -> Result<()> {
        let dir = tempdir()?;
        write_file(
            dir.path(),
            "StarterGui.service.json",
            r#"{
  "children": {
    "Hud": {
      "className": "ScreenGui",
      "properties": {
        "ResetOnSpawn": false
      },
      "children": {
        "Root": {
          "className": "Frame",
          "properties": {
            "Visible": true
          },
          "children": {
            "Title": {
              "className": "TextLabel",
              "properties": {
                "Text": "Nyjo"
              }
            }
          }
        }
      }
    }
  }
}"#,
        )?;

        let tree = scan_project(dir.path())?;
        let starter_gui = child(&tree, "StarterGui");
        assert_eq!(starter_gui.class_name, "StarterGui");

        let hud = child(starter_gui, "Hud");
        assert_eq!(hud.class_name, "ScreenGui");
        assert_eq!(
            hud.properties
                .get("ResetOnSpawn")
                .and_then(|value| value.as_bool()),
            Some(false)
        );

        let root = child(hud, "Root");
        assert_eq!(root.class_name, "Frame");

        let title = child(root, "Title");
        assert_eq!(title.class_name, "TextLabel");
        assert_eq!(
            title
                .properties
                .get("Text")
                .and_then(|value| value.as_str()),
            Some("Nyjo")
        );

        Ok(())
    }

    #[test]
    fn parses_compact_instance_tree_with_inline_scripts() -> Result<()> {
        let dir = tempdir()?;
        write_file(
            dir.path(),
            "ReplicatedStorage/Gameplay.instance.json",
            r#"{
  "children": {
    "Config": {
      "properties": {
        "Enabled": true
      }
    },
    "Shared": {
      "className": "ModuleScript",
      "source": "return { lives = 3 }"
    },
    "Net": {
      "children": {
        "Ping": {
          "className": "RemoteEvent"
        }
      }
    }
  }
}"#,
        )?;

        let tree = scan_project(dir.path())?;
        let replicated_storage = child(&tree, "ReplicatedStorage");
        let gameplay = child(replicated_storage, "Gameplay");
        assert_eq!(gameplay.class_name, "Folder");

        let config = child(gameplay, "Config");
        assert_eq!(config.class_name, "Folder");
        assert_eq!(
            config
                .properties
                .get("Enabled")
                .and_then(|value| value.as_bool()),
            Some(true)
        );

        let shared = child(gameplay, "Shared");
        assert_eq!(shared.class_name, "ModuleScript");
        assert_eq!(shared.source.as_deref(), Some("return { lives = 3 }"));

        let net = child(gameplay, "Net");
        let ping = child(net, "Ping");
        assert_eq!(ping.class_name, "RemoteEvent");

        Ok(())
    }

    #[test]
    fn preserves_arbitrary_model_json_documents() -> Result<()> {
        let dir = tempdir()?;
        write_file(
            dir.path(),
            "Workspace/Blob.model.json",
            r#"{
  "mesh": "demo",
  "version": 1
}"#,
        )?;

        let tree = scan_project(dir.path())?;
        let workspace = child(&tree, "Workspace");
        let blob = child(workspace, "Blob");

        assert_eq!(blob.class_name, "Model");
        assert!(blob.children.is_empty());
        assert_eq!(
            blob.document
                .as_ref()
                .and_then(|value| value.get("mesh"))
                .and_then(|value| value.as_str()),
            Some("demo")
        );

        Ok(())
    }

    #[test]
    fn parses_structured_part_file_with_embedded_properties_and_children() -> Result<()> {
        let dir = tempdir()?;
        write_file(
            dir.path(),
            "Workspace/Spawn.part",
            r#"--!nyjo
--HEADER
--{
--  "properties": {
--    "Anchored": true
--  },
--  "attributes": {
--    "Zone": "Lobby"
--  }
--}
--CONTENTS
{
  "children": {
    "Billboard": {
      "className": "StringValue",
      "properties": {
        "Value": "Spawn"
      }
    }
  }
}"#,
        )?;

        let tree = scan_project(dir.path())?;
        let workspace = child(&tree, "Workspace");
        let spawn = child(workspace, "Spawn");

        assert_eq!(spawn.class_name, "Part");
        assert_eq!(
            spawn
                .properties
                .get("Anchored")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
        assert_eq!(
            spawn
                .attributes
                .get("Zone")
                .and_then(|value| value.as_str()),
            Some("Lobby")
        );
        let billboard = child(spawn, "Billboard");
        assert_eq!(billboard.class_name, "StringValue");
        assert_eq!(
            billboard
                .properties
                .get("Value")
                .and_then(|value| value.as_str()),
            Some("Spawn")
        );

        Ok(())
    }

    fn write_file(root: &Path, relative: &str, contents: &str) -> Result<()> {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, contents)?;
        Ok(())
    }

    fn child<'a>(
        node: &'a crate::model::InstanceNode,
        name: &str,
    ) -> &'a crate::model::InstanceNode {
        node.children
            .iter()
            .find(|child| child.name == name)
            .unwrap_or_else(|| panic!("missing child {name}"))
    }
}
