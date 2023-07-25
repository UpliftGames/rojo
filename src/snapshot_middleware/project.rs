use std::{borrow::Cow, collections::HashMap, path::Path, sync::Arc};

use anyhow::{bail, Context};
use memofs::Vfs;
use rbx_dom_weak::types::Attributes;
use rbx_reflection::ClassTag;

use crate::{
    project::{PathNode, Project, ProjectNode},
    snapshot::{
        get_best_syncback_middleware, InstanceContext, InstanceMetadata, InstanceSnapshot,
        InstigatingSource, MiddlewareContextAny, PathIgnoreRule, SnapshotMiddleware,
        TransformerRule,
    },
    snapshot_middleware::get_middleware,
};

use super::snapshot_from_vfs;

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ProjectMiddlewareContext {
    node_middleware: Option<&'static str>,
    node_context: Option<Arc<dyn MiddlewareContextAny>>,
}

impl MiddlewareContextAny for ProjectMiddlewareContext {}

#[derive(Debug, PartialEq, Eq)]
pub struct ProjectMiddleware;

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

        let transformer_rules = project
            .transformer_rules
            .iter()
            .map(|rule| TransformerRule {
                pattern: rule.pattern.clone(),
                transformer_name: rule.transformer_name.clone(),
                base_path: project.folder_location().to_path_buf(),
            });

        context.add_transformer_rules(transformer_rules);

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

    fn syncback_update(
        &self,
        vfs: &Vfs,
        path: &Path,
        diff: &crate::snapshot::DeepDiff,
        tree: &mut crate::snapshot::RojoTree,
        old_ref: rbx_dom_weak::types::Ref,
        new_dom: &rbx_dom_weak::WeakDom,
        context: &InstanceContext,
        middleware_context: Option<Arc<dyn MiddlewareContextAny>>,
    ) -> anyhow::Result<InstanceMetadata> {
        let my_metadata = tree.get_metadata(old_ref).unwrap().clone();
        let project = Project::load_from_slice(&vfs.read(path)?, path)
            .with_context(|| format!("File was not a valid Rojo project: {}", path.display()))?;

        let mut processing = vec![(&project.tree, old_ref)];
        while !processing.is_empty() {
            let (node, node_ref) = processing.pop().unwrap();
            if diff.has_changed_properties(node_ref) {
                log::warn!(
                    "Cannot update project files from syncback. Skipping property changes. ({})",
                    path.display()
                )
            }
            if !diff.has_changed_descendants(node_ref) {
                continue;
            }

            let node_inst = tree.get_instance(node_ref);
            let node_inst = match node_inst {
                Some(inst) => inst,
                None => {
                    log::warn!(
                        "Missing instance for node. Was it deleted in a previous syncback change? Skipping. ({:?} of {})",
                        node.path,
                        path.display()
                    );
                    continue;
                }
            };

            log::trace!("checking {}", node_inst.name());

            for child_ref in node_inst.children() {
                let child_inst = tree.get_instance(*child_ref).unwrap();
                if let Some(InstigatingSource::ProjectNode(_, _, _, _)) =
                    child_inst.metadata().instigating_source
                {
                    let matching_node = node.children.get(child_inst.name());
                    if let Some(matching_node) = matching_node {
                        processing.push((matching_node, *child_ref));
                    } else {
                        log::warn!("Could not find project node for instance even though it was created by a project. Skipping. ({:?} of {})", node.path, path.display());
                    }
                }
            }

            if let Some(path_node) = &node.path {
                let new_ref = diff
                    .get_matching_new_ref(node_ref)
                    .with_context(|| "no matching new ref (diff)")?;
                let new_inst = new_dom
                    .get_by_ref(new_ref)
                    .with_context(|| "missing ref (tree)")?;

                let project_context = node_inst.metadata().syncback_context.clone().unwrap();
                let project_context: ProjectMiddlewareContext = project_context
                    .context_as_any()
                    .downcast_ref()
                    .cloned()
                    .unwrap();

                let node_path = path_node.path();
                let existing_middleware_id = project_context.node_middleware;
                let existing_middleware_context = project_context.node_context;

                let new_middleware_id =
                    get_best_syncback_middleware(new_dom, new_inst, true, existing_middleware_id);

                if let Some(new_middleware_id) = new_middleware_id {
                    if Some(new_middleware_id) == existing_middleware_id {
                        let mut new_metadata = get_middleware(new_middleware_id).syncback_update(
                            vfs,
                            node_path,
                            diff,
                            tree,
                            node_ref,
                            new_dom,
                            context,
                            existing_middleware_context,
                        )?;

                        new_metadata.syncback_context = Some(Arc::new(ProjectMiddlewareContext {
                            node_middleware: new_metadata.snapshot_middleware,
                            node_context: new_metadata.syncback_context,
                        }));
                        new_metadata.snapshot_middleware = Some(self.middleware_id());

                        tree.update_metadata(node_ref, new_metadata)
                    } else {
                        let parent_ref = node_inst.parent();

                        if let Some(existing_middleware_id) = existing_middleware_id {
                            get_middleware(existing_middleware_id)
                                .syncback_destroy(vfs, node_path, tree, node_ref)?;

                            tree.remove(node_ref);
                        }

                        // TODO: don't unwrap these, bubble up results
                        let parent_path = node_path.parent().unwrap_or_else(|| Path::new("."));
                        let name = node_path.file_name().unwrap().to_str().unwrap();

                        let mut new_snapshot = get_middleware(new_middleware_id).syncback_new(
                            vfs,
                            parent_path,
                            name,
                            new_dom,
                            new_ref,
                            context,
                        )?;

                        new_snapshot.metadata.syncback_context =
                            Some(Arc::new(ProjectMiddlewareContext {
                                node_middleware: new_snapshot.metadata.snapshot_middleware,
                                node_context: new_snapshot.metadata.syncback_context,
                            }));
                        new_snapshot.metadata.snapshot_middleware = Some(self.middleware_id());

                        tree.insert_instance(parent_ref, new_snapshot);
                    }
                } else {
                    log::warn!(
                        "Project node cannot be updated from syncback. Skipping. ({:?} of {})",
                        node.path,
                        path.display()
                    );
                }
            }
        }

        Ok(my_metadata)
    }

    fn syncback_new(
        &self,
        _vfs: &Vfs,
        _parent_path: &Path,
        _name: &str,
        _new_dom: &rbx_dom_weak::WeakDom,
        _new_ref: rbx_dom_weak::types::Ref,
        _context: &InstanceContext,
    ) -> anyhow::Result<InstanceSnapshot> {
        bail!("Cannot create new project files from syncback")
    }

    fn syncback_destroy(
        &self,
        _vfs: &Vfs,
        _path: &Path,
        _tree: &mut crate::snapshot::RojoTree,
        _old_ref: rbx_dom_weak::types::Ref,
    ) -> anyhow::Result<()> {
        bail!("Cannot destroy project files from syncback")
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
    let mut metadata = InstanceMetadata::default();

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
            metadata.syncback_context = Some(Arc::new(ProjectMiddlewareContext {
                node_middleware: metadata.snapshot_middleware,
                node_context: metadata.syncback_context,
            }));
            metadata.snapshot_middleware = Some("project");
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
