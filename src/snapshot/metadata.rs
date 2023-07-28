use std::{
    borrow::Cow,
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::{
    glob::Glob, path_serializer, project::ProjectNode, snapshot_middleware::PathExt,
    ProjectSyncback, ProjectSyncbackPropertyMode,
};

use super::{
    default_filters_diff, default_filters_save, FsSnapshot, MiddlewareContextAny, PropertyFilter,
};

/// Rojo-specific metadata that can be associated with an instance or a snapshot
/// of an instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceMetadata {
    /// Whether instances not present in the source should be ignored when
    /// live-syncing. This is useful when there are instances that Rojo does not
    /// manage.
    pub ignore_unknown_instances: bool,

    /// If a change occurs to this instance, the instigating source is what
    /// should be run through the snapshot functions to regenerate it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instigating_source: Option<InstigatingSource>,

    /// The paths that, when changed, could cause the function that generated
    /// this snapshot to generate a different snapshot. Paths should be included
    /// even if they don't exist, since the presence of a file can change the
    /// outcome of a snapshot function.
    ///
    /// For example, a file named foo.lua might have these relevant paths:
    /// - foo.lua
    /// - foo.meta.json (even if this file doesn't exist!)
    ///
    /// A directory named bar/ might have these:
    /// - bar/
    /// - bar/init.meta.json
    /// - bar/init.lua
    /// - bar/init.server.lua
    /// - bar/init.client.lua
    /// - bar/default.project.json
    ///
    /// This path is used to make sure that file changes update all instances
    /// that may need updates.
    // TODO: Change this to be a SmallVec for performance in common cases?
    #[serde(serialize_with = "path_serializer::serialize_vec_absolute")]
    pub relevant_paths: Vec<PathBuf>,

    /// Contains information about this instance that should persist between
    /// snapshot invocations and is generally inherited.
    ///
    /// If an instance has a piece of context attached to it, then the next time
    /// that instance's instigating source is snapshotted directly, the same
    /// context will be passed into it.
    pub context: InstanceContext,

    /// The middleware that created this snapshot.
    ///
    /// We can use this to update, destroy, or replace the snapshot.
    #[serde(skip)]
    pub middleware_id: Option<&'static str>,

    /// The snapshot custom context we will need if a syncback is triggered.
    #[serde(skip)]
    pub middleware_context: Option<Arc<dyn MiddlewareContextAny>>,

    /// The filesystem snapshot we can use to tear down and reconstruct the fs
    /// representation of this instance, minus its children.
    pub fs_snapshot: Option<FsSnapshot>,
}

impl InstanceMetadata {
    pub fn new() -> Self {
        Self {
            ignore_unknown_instances: false,
            instigating_source: None,
            relevant_paths: Vec::new(),
            context: InstanceContext::default(),
            middleware_id: None,
            middleware_context: None,
            fs_snapshot: None,
        }
    }

    pub fn ignore_unknown_instances(self, ignore_unknown_instances: bool) -> Self {
        Self {
            ignore_unknown_instances,
            ..self
        }
    }

    pub fn instigating_source(self, instigating_source: impl Into<InstigatingSource>) -> Self {
        Self {
            instigating_source: Some(instigating_source.into()),
            ..self
        }
    }

    pub fn relevant_paths(self, relevant_paths: Vec<PathBuf>) -> Self {
        Self {
            relevant_paths,
            ..self
        }
    }

    pub fn context(self, context: &InstanceContext) -> Self {
        Self {
            context: context.clone(),
            ..self
        }
    }

    pub fn middleware_id(self, snapshot_middleware: &'static str) -> Self {
        Self {
            middleware_id: Some(snapshot_middleware),
            ..self
        }
    }

    pub fn middleware_context(self, context: Option<Arc<dyn MiddlewareContextAny>>) -> Self {
        Self {
            middleware_context: context,
            ..self
        }
    }

    pub fn fs_snapshot(self, fs_snapshot: FsSnapshot) -> Self {
        Self {
            fs_snapshot: Some(fs_snapshot),
            ..self
        }
    }

    pub fn snapshot_source_path(&self, allow_project_sources: bool) -> Option<Cow<Path>> {
        match &self.instigating_source {
            Some(InstigatingSource::Path(path)) => Some(Cow::Borrowed(path.as_path())),
            Some(InstigatingSource::ProjectNode(project_path, _, node, _)) => {
                if !allow_project_sources {
                    return None;
                }

                let path = node.path.as_ref()?.path();
                let project_dir = project_path.parent_or_cdir().ok()?;
                let absolute_path = path.make_absolute(&project_dir).ok()?;
                Some(Cow::Owned(absolute_path.to_path_buf()))
            }
            _ => None,
        }
    }
}

impl Default for InstanceMetadata {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceContext {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub path_ignore_rules: Arc<Vec<PathIgnoreRule>>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub transformer_rules: Arc<Vec<TransformerRule>>,

    pub syncback: Arc<SyncbackContext>,
}

impl InstanceContext {
    /// Extend the list of ignore rules in the context with the given new rules.
    pub fn add_path_ignore_rules<I>(&mut self, new_rules: I)
    where
        I: IntoIterator<Item = PathIgnoreRule>,
        I::IntoIter: ExactSizeIterator,
    {
        let new_rules = new_rules.into_iter();

        // If the iterator is empty, we can skip cloning our list of ignore
        // rules and appending to it.
        if new_rules.len() == 0 {
            return;
        }

        let rules = Arc::make_mut(&mut self.path_ignore_rules);
        rules.extend(new_rules);
    }

    /// Extend syncback options with new options
    pub fn add_syncback_options(&mut self, new_options: &ProjectSyncback) -> anyhow::Result<()> {
        let syncback = Arc::make_mut(&mut self.syncback);

        syncback
            .exclude_globs
            .extend(new_options.exclude_globs.iter().cloned());

        for (key, new_value) in &new_options.properties {
            // TODO: deduplicate these
            if syncback.property_filters_diff.contains_key(key) {
                let old_value = syncback.property_filters_diff.get_mut(key).unwrap();
                if let ProjectSyncbackPropertyMode::WhenNotEqual(new_eq_values) = &new_value.diff {
                    if let PropertyFilter::IgnoreWhenEq(old_eq_values) = old_value {
                        for eq_value in new_eq_values.iter().cloned() {
                            let eq_value = eq_value.resolve_unambiguous().with_context(|| format!("Excluded syncback property {} could not be resolved to a proper value.", key))?;
                            old_eq_values.push(eq_value);
                        }
                    }
                }

                match &new_value.diff {
                    ProjectSyncbackPropertyMode::Always => {
                        continue;
                    }
                    ProjectSyncbackPropertyMode::Never => {
                        syncback
                            .property_filters_diff
                            .insert(key.clone(), PropertyFilter::Ignore);
                    }
                    ProjectSyncbackPropertyMode::WhenNotEqual(eq_values) => {
                        let mut resolved_eq_values = Vec::new();
                        for eq_value in eq_values.iter().cloned() {
                            let eq_value = eq_value.resolve_unambiguous().with_context(|| format!("Excluded syncback property {} could not be resolved to a proper value.", key))?;
                            resolved_eq_values.push(eq_value);
                        }

                        syncback.property_filters_diff.insert(
                            key.clone(),
                            PropertyFilter::IgnoreWhenEq(resolved_eq_values.clone()),
                        );
                    }
                }
            }
            if syncback.property_filters_save.contains_key(key) {
                let old_value = syncback.property_filters_save.get_mut(key).unwrap();
                if let ProjectSyncbackPropertyMode::WhenNotEqual(new_eq_values) = &new_value.save {
                    if let PropertyFilter::IgnoreWhenEq(old_eq_values) = old_value {
                        for eq_value in new_eq_values.iter().cloned() {
                            let eq_value = eq_value.resolve_unambiguous().with_context(|| format!("Excluded syncback property {} could not be resolved to a proper value.", key))?;
                            old_eq_values.push(eq_value);
                        }
                    }
                }

                match &new_value.save {
                    ProjectSyncbackPropertyMode::Always => {
                        continue;
                    }
                    ProjectSyncbackPropertyMode::Never => {
                        syncback
                            .property_filters_save
                            .insert(key.clone(), PropertyFilter::Ignore);
                    }
                    ProjectSyncbackPropertyMode::WhenNotEqual(eq_values) => {
                        let mut resolved_eq_values = Vec::new();
                        for eq_value in eq_values.iter().cloned() {
                            let eq_value = eq_value.resolve_unambiguous().with_context(|| format!("Excluded syncback property {} could not be resolved to a proper value.", key))?;
                            resolved_eq_values.push(eq_value);
                        }

                        syncback.property_filters_save.insert(
                            key.clone(),
                            PropertyFilter::IgnoreWhenEq(resolved_eq_values.clone()),
                        );
                    }
                }
            }
        }

        Ok(())
    }

    /// Extend the list of type override rules in the context with the given new rules.
    pub fn add_transformer_rules<I>(&mut self, new_rules: I)
    where
        I: IntoIterator<Item = TransformerRule>,
        I::IntoIter: ExactSizeIterator,
    {
        let new_rules = new_rules.into_iter();

        // If the iterator is empty, we can skip cloning our list of ignore
        // rules and appending to it.
        if new_rules.len() == 0 {
            return;
        }

        let rules = Arc::make_mut(&mut self.transformer_rules);
        rules.extend(new_rules);
    }

    pub fn should_syncback_path(&self, path: &Path) -> bool {
        !self
            .syncback
            .exclude_globs
            .iter()
            .map(|glob| glob.is_match(path))
            .any(|is_match| is_match)
    }

    pub fn get_transformer_override(&self, path: &Path) -> Option<Transformer> {
        for rule in self.transformer_rules.iter() {
            if rule.applies_to(path) {
                return Some(Transformer::from_str(&rule.transformer_name));
            }
        }

        None
    }
}

impl Default for InstanceContext {
    fn default() -> Self {
        InstanceContext {
            path_ignore_rules: Arc::new(Vec::new()),
            syncback: Arc::new(SyncbackContext::default()),
            transformer_rules: Arc::new(Vec::new()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncbackContext {
    pub exclude_globs: Vec<Glob>,
    pub property_filters_diff: BTreeMap<String, PropertyFilter>,
    pub property_filters_save: BTreeMap<String, PropertyFilter>,
}

impl Default for SyncbackContext {
    fn default() -> Self {
        Self {
            exclude_globs: Vec::new(),
            property_filters_diff: default_filters_diff().clone(),
            property_filters_save: default_filters_save().clone(),
        }
    }
}

impl PartialEq for SyncbackContext {
    fn eq(&self, other: &Self) -> bool {
        self.exclude_globs == other.exclude_globs
            && self.property_filters_diff == other.property_filters_diff
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathIgnoreRule {
    /// The path that this glob is relative to. Since ignore globs are defined
    /// in project files, this will generally be the folder containing the
    /// project file that defined this glob.
    #[serde(serialize_with = "path_serializer::serialize_absolute")]
    pub base_path: PathBuf,

    /// The actual glob that can be matched against the input path.
    pub glob: Glob,
}

impl PathIgnoreRule {
    pub fn passes<P: AsRef<Path>>(&self, path: P) -> bool {
        let path = path.as_ref();

        match path.strip_prefix(&self.base_path) {
            Ok(suffix) => !self.glob.is_match(suffix),
            Err(_) => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Transformer {
    Plain,
    LuauModule,
    LuauServer,
    LuauClient,
    Json,
    Toml,
    Csv,

    Project,
    Rbxm,
    Rbxmx,
    JsonModel,

    Ignore,
    Other(String),
}

impl Transformer {
    pub fn from_str(s: &str) -> Self {
        match s {
            "rojo/plaintext" => Self::Plain,
            "rojo/luau" => Self::LuauModule,
            "rojo/luauserver" => Self::LuauServer,
            "rojo/luauclient" => Self::LuauClient,
            "rojo/json" => Self::Json,
            "rojo/toml" => Self::Toml,
            "rojo/csv" => Self::Csv,

            "rojo/project" => Self::Project,
            "rojo/rbxm" => Self::Rbxm,
            "rojo/rbxmx" => Self::Rbxmx,
            "rojo/jsonmodel" => Self::JsonModel,

            "rojo/ignore" => Self::Ignore,

            _ => Self::Other(s.to_owned()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransformerRule {
    /// The glob to match files against for this type override
    pub pattern: Glob,

    /// The type of file this match should be treated as
    pub transformer_name: String,

    /// The path that this glob is relative to. Since ignore globs are defined
    /// in project files, this will generally be the folder containing the
    /// project file that defined this glob.
    #[serde(serialize_with = "path_serializer::serialize_absolute")]
    pub base_path: PathBuf,
}

impl TransformerRule {
    pub fn applies_to<P: AsRef<Path>>(&self, path: P) -> bool {
        let path = path.as_ref();

        match path.strip_prefix(&self.base_path) {
            Ok(suffix) => self.pattern.is_match(suffix),
            Err(_) => false,
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub enum InstigatingSource {
    Path(#[serde(serialize_with = "path_serializer::serialize_absolute")] PathBuf),
    ProjectNode(
        #[serde(serialize_with = "path_serializer::serialize_absolute")] PathBuf,
        String,
        ProjectNode,
        Option<String>,
    ),
}

impl fmt::Debug for InstigatingSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstigatingSource::Path(path) => write!(formatter, "Path({})", path.display()),
            InstigatingSource::ProjectNode(path, name, node, parent_class) => write!(
                formatter,
                "ProjectNode({}: {:?}) from path {} and parent class {:?}",
                name,
                node,
                path.display(),
                parent_class,
            ),
        }
    }
}

impl From<PathBuf> for InstigatingSource {
    fn from(path: PathBuf) -> Self {
        InstigatingSource::Path(path)
    }
}

impl From<&Path> for InstigatingSource {
    fn from(path: &Path) -> Self {
        InstigatingSource::Path(path.to_path_buf())
    }
}
