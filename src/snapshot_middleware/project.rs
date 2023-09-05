use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{bail, Context};
use indexmap::IndexMap;
use memofs::Vfs;
use rbx_dom_weak::types::Attributes;
use rbx_reflection::ClassTag;

use crate::{
    project::{PathNode, Project, ProjectNode},
    resolution::UnresolvedValue,
    snapshot::{
        get_best_syncback_middleware, InstanceContext, InstanceMetadata, InstanceSnapshot,
        InstigatingSource, MiddlewareContextAny, PathIgnoreRule, PropertiesFiltered,
        SnapshotMiddleware, SnapshotRule, SyncbackArgs, SyncbackNode, SyncbackPlanner,
        SyncbackPlannerWrapped,
    },
    snapshot_middleware::util::PathExt,
};

use super::{get_middlewares_prefixed, snapshot_from_vfs};

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ProjectMiddlewareContext {
    node_middleware: Option<&'static str>,
    node_context: Option<Arc<dyn MiddlewareContextAny>>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ProjectMiddleware;

pub type OldRefPack<'a> = (
    &'a crate::snapshot::RojoTree,
    rbx_dom_weak::types::Ref,
    Option<Arc<dyn MiddlewareContextAny>>,
);

impl SnapshotMiddleware for ProjectMiddleware {
    fn middleware_id(&self) -> &'static str {
        "project"
    }

    fn default_globs(&self) -> &[&'static str] {
        &["**/*.project.json"]
    }

    fn init_names(&self) -> &[&'static str] {
        &["default.project.json"]
    }

    fn snapshot(
        &self,
        context: &InstanceContext,
        vfs: &Vfs,
        path: &Path,
    ) -> anyhow::Result<Option<InstanceSnapshot>> {
        let project = Project::load_from_slice(&vfs.read(path)?, path)
            .with_context(|| format!("File was not a valid Rojo project: {}", path.display()))?;

        let mut context = context.clone();

        let path_ignore_rules = project.glob_ignore_paths.iter().map(|glob| PathIgnoreRule {
            glob: glob.clone(),
            base_path: project.folder_location().to_path_buf(),
        });

        context.add_path_ignore_rules(path_ignore_rules);

        if let Some(syncback_options) = &project.syncback {
            context.add_syncback_options(syncback_options)?;
        }

        let mut snapshot_rules = Vec::new();
        for rule in project.snapshot_rules.clone() {
            if !get_middlewares_prefixed().contains_key(rule.middleware_name.as_str()) {
                bail!(
                    "Unknown middleware: {}; Known middlewares: {}",
                    rule.middleware_name,
                    get_middlewares_prefixed()
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }

            snapshot_rules.push(SnapshotRule {
                inner: rule.clone(),
                base_path: project.folder_location().to_path_buf(),
            });
        }

        context.add_snapshot_rules(snapshot_rules);

        match snapshot_project_node(&context, path, &project.name, &project.tree, vfs, None)? {
            Some(found_snapshot) => {
                let mut snapshot = found_snapshot;
                // Setting the instigating source to the project file path is a little
                // coarse.
                //
                // Ideally, we'd only snapshot the project file if the project file
                // actually changed. Because Rojo only has the concept of one
                // relevant path -> snapshot path mapping per instance, we pick the more
                // conservative approach of snapshotting the project file if any
                // relevant paths changed.
                snapshot.metadata.instigating_source = Some(path.to_path_buf().into());

                // Mark this snapshot (the root node of the project file) as being
                // related to the project file.
                //
                // We SHOULD NOT mark the project file as a relevant path for any
                // nodes that aren't roots. They'll be updated as part of the project
                // file being updated.
                snapshot.metadata.relevant_paths.push(path.to_path_buf());

                snapshot.metadata.fs_snapshot = Some(
                    snapshot
                        .metadata
                        .fs_snapshot
                        .unwrap_or_default()
                        .with_file(path),
                );

                Ok(Some(snapshot))
            }
            None => Ok(None),
        }
    }

    fn syncback_priority(
        &self,
        _dom: &rbx_dom_weak::WeakDom,
        _instance: &rbx_dom_weak::Instance,
        _consider_descendants: bool,
    ) -> Option<i32> {
        None
    }

    fn syncback_new_path(
        &self,
        _parent_path: &Path,
        _name: &str,
        _new_inst: &rbx_dom_weak::Instance,
    ) -> anyhow::Result<PathBuf> {
        bail!("Cannot create new project files from syncback")
    }

    fn syncback(&self, sync: &SyncbackArgs<'_, '_>) -> anyhow::Result<SyncbackNode> {
        let vfs = sync.vfs;
        let diff = sync.diff;
        let project_path = sync.path;
        let old: &Option<OldRefPack> = &sync.old;
        let new = sync.new;
        let metadata = sync.metadata;
        let _overrides = &sync.overrides;

        let (old_dom, old_root_ref, root_middleware_context) = match old {
            Some(v) => v,
            None => bail!(
                "Cannot create new project files from syncback ({})",
                project_path.display()
            ),
        };

        let (new_dom, new_root_ref) = new;

        let my_metadata = metadata.clone();
        let mut project = Project::load_from_slice(&vfs.read(project_path)?, project_path)
            .with_context(|| {
                format!(
                    "File was not a valid Rojo project: {}",
                    project_path.display()
                )
            })?;
        let mut project_changed = false;

        let mut children = Vec::new();
        let mut root_syncback_node = None;

        let project_directory = project_path.parent_or_cdir()?;

        // We use node paths here instead of nodes to get around mutable borrow
        // issues caused by borrowing nodes to stick them in the list.
        let mut processing = vec![(Vec::<String>::new(), *old_root_ref)];
        while let Some((project_node_path, node_ref)) = processing.pop() {
            let node = {
                let mut node = &mut project.tree;
                for path in project_node_path.iter() {
                    node = node.children.get_mut(path).unwrap();
                }
                node
            };

            let node_inst = old_dom.get_instance(node_ref);
            let node_inst = match node_inst {
                Some(inst) => inst,
                None => {
                    log::warn!(
                        "Missing instance for node. Was it deleted in a previous syncback change? Skipping. ({:?} of {})",
                        node.path,
                        project_path.display()
                    );
                    continue;
                }
            };

            if let Some(path_node) = &node.path {
                let node_path = path_node.path();
                let node_path = node_path.make_absolute(project_directory)?;

                if !metadata.context.should_syncback_path(&node_path) {
                    log::info!("Skipping syncback of {} because it is excluded by syncback ignore path rules.", node_path.display());
                    continue;
                }
            }

            if diff.has_changed_descendants(node_ref) {
                for child_ref in node_inst.children() {
                    let child_inst = old_dom.get_instance(*child_ref).unwrap();
                    if let Some(InstigatingSource::ProjectNode(_, node_name, _, _)) =
                        &child_inst.metadata().instigating_source
                    {
                        let matching_node = node.children.get_mut(child_inst.name());
                        if matching_node.is_some() {
                            let next_node_path = project_node_path
                                .iter()
                                .cloned()
                                .chain(std::iter::once(node_name.clone()))
                                .collect();
                            processing.push((next_node_path, *child_ref));
                        } else {
                            log::warn!("Could not find project node for instance even though it was created by a project. Skipping. ({:?} of {})", node.path, project_path.display());
                        }
                    }
                }
            }

            if !diff.has_changed_descendants(node_ref) && !diff.has_changed_properties(node_ref) {
                continue;
            }

            if let Some(path_node) = &node.path {
                let node_path = path_node.path();
                let node_path = node_path.make_absolute(project_directory)?;

                let new_ref = diff
                    .get_matching_new_ref(node_ref)
                    .with_context(|| "no matching new ref (diff)")?;
                let new_inst = new_dom
                    .get_by_ref(new_ref)
                    .with_context(|| "missing ref (tree)")?;

                let middleware_context = if node_ref == *old_root_ref {
                    root_middleware_context
                } else {
                    &node_inst.metadata().middleware_context
                };

                let project_context = if let Some(middleware_context) = middleware_context {
                    middleware_context.downcast_ref::<ProjectMiddlewareContext>()
                } else {
                    None
                };

                let project_context = project_context.as_ref();

                let existing_middleware_id = project_context.and_then(|v| v.node_middleware);
                let existing_middleware_context =
                    project_context.and_then(|v| v.node_context.clone());

                let new_middleware_id =
                    get_best_syncback_middleware(new_dom, new_inst, true, existing_middleware_id);

                if existing_middleware_id != new_middleware_id {
                    log::warn!("Project update wants to change {} from {} to {}. Changing filetype in project updates is not allowed. Skipping.", node_path.display(), existing_middleware_id.map_or_else(|| "None", |id| id), new_middleware_id.map_or_else(|| "None", |id| id));
                    continue;
                }

                if let Some(middleware_id) = new_middleware_id {
                    let child_syncback_node = SyncbackPlanner::from_update(
                        old_dom,
                        node_ref,
                        new_dom,
                        new_ref,
                        Some(&node_path),
                        Some((middleware_id, existing_middleware_context)),
                    )?
                    .syncback(
                        vfs, diff,
                        None,
                        // Some(SnapshotOverride {
                        //     known_class: override,
                        // }),
                    )?;

                    if let Some(child_syncback_node) = child_syncback_node {
                        // TODO: properly check properties, this checks snapshot
                        // properties whish is different from meta properties
                        if !node.properties.is_empty() || node.class_name.is_some() {
                            // Properties are moved to a .meta file now; delete from project file
                            node.properties = IndexMap::new();
                            // Getting class name inheritance from the project
                            // node working is a pain, so we'll just enforce
                            // putting it in the meta file if you're putting
                            // properties in there too.
                            node.class_name = None;
                            project_changed = true;
                        }

                        if node_ref == *old_root_ref {
                            root_syncback_node = Some(child_syncback_node);
                            log::trace!("Root node updated from syncback for {}", node_inst.name());
                        } else {
                            children.push(child_syncback_node);
                        }
                    }
                } else {
                    log::warn!(
                        "Project node cannot be updated from syncback (no matching middleware). Skipping. ({:?} of {})",
                        node.path,
                        project_path.display()
                    );
                }
            } else if diff.has_changed_properties(node_ref) {
                // All properties should be put in the node since it has no other source

                node.properties = node_inst
                    .properties_filtered(&metadata.context.syncback.property_filters_save, true)
                    .map(|(key, value)| {
                        (
                            key.to_string(),
                            UnresolvedValue::from_variant_property(
                                node_inst.class_name(),
                                key,
                                value.clone(),
                            ),
                        )
                    })
                    .collect();

                project_changed = true;
            }
        }

        let new_root_inst = new_dom.get_by_ref(new_root_ref).unwrap();

        let mut violates_rules = false;
        if let Some(root_syncback_node) = &root_syncback_node {
            let inst_sync = &root_syncback_node.instance_snapshot;
            if let Some(fs_snapshot) = &inst_sync.metadata.fs_snapshot {
                violates_rules = fs_snapshot
                    .files
                    .keys()
                    .chain(fs_snapshot.dirs.iter())
                    .any(|path| !inst_sync.metadata.context.should_syncback_path(path));
            }
        }

        if violates_rules {
            log::info!("Skipping syncback of {} because it is excluded by syncback ignore path rules. (at project level; still syncing in project children, only skipping project init)", root_syncback_node.as_ref().unwrap().instance_snapshot.name);
            root_syncback_node = None;
        }

        let mut root_syncback_node = root_syncback_node.unwrap_or_else(|| {
            SyncbackNode::new(
                (*old_root_ref, new_root_ref),
                project_path,
                InstanceSnapshot::new()
                    .properties(new_root_inst.properties.clone())
                    .name(&new_root_inst.name)
                    .class_name(&new_root_inst.class)
                    .metadata(my_metadata)
                    .preferred_ref(sync.ref_for_save()),
            )
        });

        let snapshot = &mut root_syncback_node.instance_snapshot;

        snapshot.metadata.instigating_source = Some(project_path.to_path_buf().into());
        snapshot
            .metadata
            .relevant_paths
            .push(project_path.to_path_buf());

        let mut fs_snapshot = snapshot.metadata.fs_snapshot.take().unwrap_or_default();
        log::trace!(
            "existing root node fs snapshot for {}:\n{:#?}",
            project_path.display(),
            fs_snapshot
        );
        if project_changed {
            fs_snapshot = fs_snapshot
                .with_file_contents_owned(project_path, serde_json::to_string_pretty(&project)?);
        } else {
            fs_snapshot = fs_snapshot.with_file(project_path);
        }
        snapshot.metadata.fs_snapshot = Some(fs_snapshot);

        let inner_get_children = root_syncback_node.get_children;
        let inner_path = root_syncback_node.path.clone();
        let inner_root_middleware_context = root_middleware_context.clone();

        root_syncback_node.get_children = Some(Box::new(move |sync| {
            let mut result_children = Vec::new();
            let mut result_remove = HashSet::new();

            if let Some(inner_get_children) = inner_get_children {
                let (children, remove) = inner_get_children(&SyncbackArgs {
                    path: &inner_path,
                    old: sync.old.as_ref().map(|(old_dom, old_ref, _)| {
                        (*old_dom, *old_ref, inner_root_middleware_context)
                    }),
                    overrides: None,
                    ..sync.clone()
                })?;
                result_children.extend(children);
                result_remove.extend(remove);
            }

            result_children.extend(children);

            Ok((result_children, result_remove))
        }));

        Ok(root_syncback_node)
    }
}

pub fn snapshot_project_node(
    context: &InstanceContext,
    project_path: &Path,
    instance_name: &str,
    node: &ProjectNode,
    vfs: &Vfs,
    parent_class: Option<&str>,
) -> anyhow::Result<Option<InstanceSnapshot>> {
    let project_folder = project_path.parent().unwrap();

    let class_name_from_project = node
        .class_name
        .as_ref()
        .map(|name| Cow::Owned(name.clone()));
    let mut class_name_from_path = None;

    let name = Cow::Owned(instance_name.to_owned());
    let mut properties = HashMap::new();
    let mut children = Vec::new();
    let mut metadata = InstanceMetadata {
        middleware_id: Some("project"),
        ..Default::default()
    };

    if let Some(path_node) = &node.path {
        let path = path_node.path();

        // If the path specified in the project is relative, we assume it's
        // relative to the folder that the project is in, project_folder.
        let full_path = if path.is_relative() {
            Cow::Owned(project_folder.join(path))
        } else {
            Cow::Borrowed(path)
        };

        if let Some(snapshot) = snapshot_from_vfs(context, vfs, &full_path)? {
            log::trace!("project snapshot from vfs for {}", full_path.display());
            // log::trace!("{:?}", snapshot);

            class_name_from_path = Some(snapshot.class_name);

            // Properties from the snapshot are pulled in unchanged, and
            // overridden by properties set on the project node.
            properties.reserve(snapshot.properties.len());
            for (key, value) in snapshot.properties.into_iter() {
                properties.insert(key, value);
            }

            // The snapshot's children will be merged with the children defined
            // in the project node, if there are any.
            children.reserve(snapshot.children.len());
            for child in snapshot.children.into_iter() {
                children.push(child);
            }

            // Take the snapshot's metadata as-is, which will be mutated later
            // on.
            metadata = snapshot.metadata;

            // Move sub-snapshot middleware context into our own middleware context.
            metadata.middleware_context = Some(Arc::new(ProjectMiddlewareContext {
                node_middleware: metadata.middleware_id,
                node_context: metadata.middleware_context,
            }));
            metadata.middleware_id = Some("project");
        } else {
            log::trace!("no snapshot from vfs for {}", full_path.display());
        }
    }

    let class_name_from_inference = infer_class_name(&name, parent_class);

    let class_name = match (
        class_name_from_project,
        class_name_from_path,
        class_name_from_inference,
        &node.path,
    ) {
        // These are the easy, happy paths!
        (Some(project), None, None, _) => project,
        (None, Some(path), None, _) => path,
        (None, None, Some(inference), _) => inference,

        // If the user specifies a class name, but there's an inferred class
        // name, we prefer the name listed explicitly by the user.
        (Some(project), None, Some(_), _) => project,

        // If the user has a $path pointing to a folder and we're able to infer
        // a class name, let's use the inferred name. If the path we're pointing
        // to isn't a folder, though, that's a user error.
        (None, Some(path), Some(inference), _) => {
            if path == "Folder" {
                inference
            } else {
                path
            }
        }

        (Some(project), Some(path), _, _) => {
            if path == "Folder" {
                project
            } else {
                bail!(
                    "ClassName for Instance \"{}\" was specified in both the project file (as \"{}\") and from the filesystem (as \"{}\").\n\
                     If $className and $path are both set, $path must refer to a Folder.
                     \n\
                     Project path: {}\n\
                     Filesystem path: {}\n",
                    instance_name,
                    project,
                    path,
                    project_path.display(),
                    node.path.as_ref().unwrap().path().display()
                );
            }
        }

        (None, None, None, Some(PathNode::Optional(_))) => {
            return Ok(None);
        }

        (_, None, _, Some(PathNode::Required(path))) => {
            anyhow::bail!(
                "Rojo project referred to a file using $path that could not be turned into a Roblox Instance by Rojo.\n\
                Check that the file exists and is a file type known by Rojo.\n\
                \n\
                Project path: {}\n\
                File $path: {}",
                project_path.display(),
                path.display(),
            );
        }

        (None, None, None, None) => {
            bail!(
                "Instance \"{}\" is missing some required information.\n\
                 One of the following must be true:\n\
                 - $className must be set to the name of a Roblox class\n\
                 - $path must be set to a path of an instance\n\
                 - The instance must be a known service, like ReplicatedStorage\n\
                 \n\
                 Project path: {}",
                instance_name,
                project_path.display(),
            );
        }
    };

    for (child_name, child_project_node) in &node.children {
        if let Some(child) = snapshot_project_node(
            context,
            project_path,
            child_name,
            child_project_node,
            vfs,
            Some(&class_name),
        )? {
            children.push(child);
        }
    }

    for (key, unresolved) in &node.properties {
        let value = unresolved
            .clone()
            .resolve(&class_name, key)
            .with_context(|| {
                format!(
                    "Unresolvable property in project at path {}",
                    project_path.display()
                )
            })?;

        match key.as_str() {
            "Name" | "Parent" => {
                log::warn!(
                    "Property '{}' cannot be set manually, ignoring. Attempted to set in '{}' at {}",
                    key,
                    instance_name,
                    project_path.display()
                );
                continue;
            }

            _ => {}
        }

        properties.insert(key.clone(), value);
    }

    if !node.attributes.is_empty() {
        let mut attributes = Attributes::new();

        for (key, unresolved) in &node.attributes {
            let value = unresolved.clone().resolve_unambiguous().with_context(|| {
                format!(
                    "Unresolvable attribute in project at path {}",
                    project_path.display()
                )
            })?;

            attributes.insert(key.clone(), value);
        }

        properties.insert("Attributes".into(), attributes.into());
    }

    // If the user specified $ignoreUnknownInstances, overwrite the existing
    // value.
    //
    // If the user didn't specify it AND $path was not specified (meaning
    // there's no existing value we'd be stepping on from a project file or meta
    // file), set it to true.
    if let Some(ignore) = node.ignore_unknown_instances {
        metadata.ignore_unknown_instances = ignore;
    } else if node.path.is_none() {
        // TODO: Introduce a strict mode where $ignoreUnknownInstances is never
        // set implicitly.
        metadata.ignore_unknown_instances = true;
    }

    metadata.instigating_source = Some(InstigatingSource::ProjectNode(
        project_path.to_path_buf(),
        instance_name.to_string(),
        node.clone(),
        parent_class.map(|name| name.to_owned()),
    ));

    Ok(Some(InstanceSnapshot {
        snapshot_id: None,
        name,
        class_name,
        preferred_ref: node.referent,
        properties,
        children,
        metadata,
    }))
}

fn infer_class_name(name: &str, parent_class: Option<&str>) -> Option<Cow<'static, str>> {
    // If className wasn't defined from another source, we may be able
    // to infer one.

    let parent_class = parent_class?;

    if parent_class == "DataModel" {
        // Members of DataModel with names that match known services are
        // probably supposed to be those services.

        let descriptor = rbx_reflection_database::get().classes.get(name)?;

        if descriptor.tags.contains(&ClassTag::Service) {
            return Some(Cow::Owned(name.to_owned()));
        }
    } else if parent_class == "StarterPlayer" {
        // StarterPlayer has two special members with their own classes.

        if name == "StarterPlayerScripts" || name == "StarterCharacterScripts" {
            return Some(Cow::Owned(name.to_owned()));
        }
    }

    None
}

// #[cfg(feature = "broken-tests")]
#[cfg(test)]
mod test {
    use super::*;

    use maplit::hashmap;
    use memofs::{InMemoryFs, VfsSnapshot};

    #[ignore = "Functionality moved to root snapshot middleware"]
    #[test]
    fn project_from_folder() {
        let _ = env_logger::try_init();

        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot(
            "/foo",
            VfsSnapshot::dir(hashmap! {
                "default.project.json" => VfsSnapshot::file(r#"
                    {
                        "name": "indirect-project",
                        "tree": {
                            "$className": "Folder"
                        }
                    }
                "#),
            }),
        )
        .unwrap();

        let mut vfs = Vfs::new(imfs);

        let instance_snapshot = ProjectMiddleware
            .snapshot(&InstanceContext::default(), &mut vfs, Path::new("/foo"))
            .expect("snapshot error")
            .expect("snapshot returned no instances");

        insta::assert_yaml_snapshot!(instance_snapshot);
    }

    #[test]
    fn project_from_direct_file() {
        let _ = env_logger::try_init();

        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot(
            "/foo",
            VfsSnapshot::dir(hashmap! {
                "hello.project.json" => VfsSnapshot::file(r#"
                    {
                        "name": "direct-project",
                        "tree": {
                            "$className": "Model"
                        }
                    }
                "#),
            }),
        )
        .unwrap();

        let mut vfs = Vfs::new(imfs);

        let instance_snapshot = ProjectMiddleware
            .snapshot(
                &InstanceContext::default(),
                &mut vfs,
                Path::new("/foo/hello.project.json"),
            )
            .expect("snapshot error")
            .expect("snapshot returned no instances");

        insta::assert_yaml_snapshot!(instance_snapshot);
    }

    #[test]
    fn project_with_resolved_properties() {
        let _ = env_logger::try_init();

        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot(
            "/foo.project.json",
            VfsSnapshot::file(
                r#"
                    {
                        "name": "resolved-properties",
                        "tree": {
                            "$className": "StringValue",
                            "$properties": {
                                "Value": {
                                    "String": "Hello, world!"
                                }
                            }
                        }
                    }
                "#,
            ),
        )
        .unwrap();

        let mut vfs = Vfs::new(imfs);

        let instance_snapshot = ProjectMiddleware
            .snapshot(
                &InstanceContext::default(),
                &mut vfs,
                Path::new("/foo.project.json"),
            )
            .expect("snapshot error")
            .expect("snapshot returned no instances");

        insta::assert_yaml_snapshot!(instance_snapshot);
    }

    #[test]
    fn project_with_unresolved_properties() {
        let _ = env_logger::try_init();

        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot(
            "/foo.project.json",
            VfsSnapshot::file(
                r#"
                    {
                        "name": "unresolved-properties",
                        "tree": {
                            "$className": "StringValue",
                            "$properties": {
                                "Value": "Hi!"
                            }
                        }
                    }
                "#,
            ),
        )
        .unwrap();

        let mut vfs = Vfs::new(imfs);

        let instance_snapshot = ProjectMiddleware
            .snapshot(
                &InstanceContext::default(),
                &mut vfs,
                Path::new("/foo.project.json"),
            )
            .expect("snapshot error")
            .expect("snapshot returned no instances");

        insta::assert_yaml_snapshot!(instance_snapshot);
    }

    #[test]
    fn project_with_children() {
        let _ = env_logger::try_init();

        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot(
            "/foo.project.json",
            VfsSnapshot::file(
                r#"
                    {
                        "name": "children",
                        "tree": {
                            "$className": "Folder",

                            "Child": {
                                "$className": "Model"
                            }
                        }
                    }
                "#,
            ),
        )
        .unwrap();

        let mut vfs = Vfs::new(imfs);

        let instance_snapshot = ProjectMiddleware
            .snapshot(
                &InstanceContext::default(),
                &mut vfs,
                Path::new("/foo.project.json"),
            )
            .expect("snapshot error")
            .expect("snapshot returned no instances");

        insta::assert_yaml_snapshot!(instance_snapshot);
    }

    #[test]
    fn project_with_path_to_txt() {
        let _ = env_logger::try_init();

        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot(
            "/foo",
            VfsSnapshot::dir(hashmap! {
                "default.project.json" => VfsSnapshot::file(r#"
                    {
                        "name": "path-project",
                        "tree": {
                            "$path": "other.txt"
                        }
                    }
                "#),
                "other.txt" => VfsSnapshot::file("Hello, world!"),
            }),
        )
        .unwrap();

        let mut vfs = Vfs::new(imfs);

        let instance_snapshot = ProjectMiddleware
            .snapshot(
                &InstanceContext::default(),
                &mut vfs,
                Path::new("/foo/default.project.json"),
            )
            .expect("snapshot error")
            .expect("snapshot returned no instances");

        insta::assert_yaml_snapshot!(instance_snapshot);
    }

    #[test]
    fn project_with_path_to_project() {
        let _ = env_logger::try_init();

        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot(
            "/foo",
            VfsSnapshot::dir(hashmap! {
                "default.project.json" => VfsSnapshot::file(r#"
                    {
                        "name": "path-project",
                        "tree": {
                            "$path": "other.project.json"
                        }
                    }
                "#),
                "other.project.json" => VfsSnapshot::file(r#"
                    {
                        "name": "other-project",
                        "tree": {
                            "$className": "Model"
                        }
                    }
                "#),
            }),
        )
        .unwrap();

        let mut vfs = Vfs::new(imfs);

        let instance_snapshot = ProjectMiddleware
            .snapshot(
                &InstanceContext::default(),
                &mut vfs,
                Path::new("/foo/default.project.json"),
            )
            .expect("snapshot error")
            .expect("snapshot returned no instances");

        insta::assert_yaml_snapshot!(instance_snapshot);
    }

    #[test]
    fn project_with_path_to_project_with_children() {
        let _ = env_logger::try_init();

        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot(
            "/foo",
            VfsSnapshot::dir(hashmap! {
                "default.project.json" => VfsSnapshot::file(r#"
                    {
                        "name": "path-child-project",
                        "tree": {
                            "$path": "other.project.json"
                        }
                    }
                "#),
                "other.project.json" => VfsSnapshot::file(r#"
                    {
                        "name": "other-project",
                        "tree": {
                            "$className": "Folder",

                            "SomeChild": {
                                "$className": "Model"
                            }
                        }
                    }
                "#),
            }),
        )
        .unwrap();

        let mut vfs = Vfs::new(imfs);

        let instance_snapshot = ProjectMiddleware
            .snapshot(
                &InstanceContext::default(),
                &mut vfs,
                Path::new("/foo/default.project.json"),
            )
            .expect("snapshot error")
            .expect("snapshot returned no instances");

        insta::assert_yaml_snapshot!(instance_snapshot);
    }

    /// Ensures that if a property is defined both in the resulting instance
    /// from $path and also in $properties, that the $properties value takes
    /// precedence.
    #[test]
    fn project_path_property_overrides() {
        let _ = env_logger::try_init();

        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot(
            "/foo",
            VfsSnapshot::dir(hashmap! {
                "default.project.json" => VfsSnapshot::file(r#"
                    {
                        "name": "path-property-override",
                        "tree": {
                            "$path": "other.project.json",
                            "$properties": {
                                "Value": "Changed"
                            }
                        }
                    }
                "#),
                "other.project.json" => VfsSnapshot::file(r#"
                    {
                        "name": "other-project",
                        "tree": {
                            "$className": "StringValue",
                            "$properties": {
                                "Value": "Original"
                            }
                        }
                    }
                "#),
            }),
        )
        .unwrap();

        let mut vfs = Vfs::new(imfs);

        let instance_snapshot = ProjectMiddleware
            .snapshot(
                &InstanceContext::default(),
                &mut vfs,
                Path::new("/foo/default.project.json"),
            )
            .expect("snapshot error")
            .expect("snapshot returned no instances");

        insta::assert_yaml_snapshot!(instance_snapshot);
    }
}
