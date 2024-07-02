use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, VecDeque},
    path::Path,
};

use anyhow::{bail, Context};
use memofs::Vfs;
use rbx_dom_weak::{
    types::{Attributes, Ref, Variant},
    Instance,
};
use rbx_reflection::ClassTag;

use crate::{
    project::{PathNode, Project, ProjectNode},
    resolution::UnresolvedValue,
    snapshot::{
        InstanceContext, InstanceMetadata, InstanceSnapshot, InstanceWithMeta, InstigatingSource,
        PathIgnoreRule, SyncRule,
    },
    snapshot_middleware::Middleware,
    syncback::{FsSnapshot, SyncbackReturn, SyncbackSnapshot},
    RojoRef,
};

use super::{emit_legacy_scripts_default, snapshot_from_vfs};

pub fn snapshot_project(
    context: &InstanceContext,
    vfs: &Vfs,
    path: &Path,
    name: &str,
) -> anyhow::Result<Option<InstanceSnapshot>> {
    let project = Project::load_from_slice(&vfs.read(path)?, path)
        .with_context(|| format!("File was not a valid Rojo project: {}", path.display()))?;
    let project_name = project.name.as_deref().unwrap_or(name);

    let mut context = context.clone();
    context.clear_sync_rules();

    let rules = project.glob_ignore_paths.iter().map(|glob| PathIgnoreRule {
        glob: glob.clone(),
        base_path: project.folder_location().to_path_buf(),
    });

    let sync_rules = project.sync_rules.iter().map(|rule| SyncRule {
        base_path: project.folder_location().to_path_buf(),
        ..rule.clone()
    });

    context.add_sync_rules(sync_rules);
    context.add_path_ignore_rules(rules);
    context.set_emit_legacy_scripts(
        project
            .emit_legacy_scripts
            .or_else(emit_legacy_scripts_default)
            .unwrap(),
    );

    match snapshot_project_node(&context, path, project_name, &project.tree, vfs, None)? {
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
    let mut metadata = InstanceMetadata::new().context(context);

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

    if let Some(id) = &node.id {
        metadata.specified_id = Some(RojoRef::new(id.clone()))
    }

    if let Some(id) = &node.id {
        metadata.specified_id = Some(RojoRef::new(id.clone()))
    }

    metadata.instigating_source = Some(InstigatingSource::ProjectNode {
        path: project_path.to_path_buf(),
        name: instance_name.to_string(),
        node: node.clone(),
        parent_class: parent_class.map(|name| name.to_owned()),
    });

    Ok(Some(InstanceSnapshot {
        snapshot_id: Ref::none(),
        name,
        class_name,
        properties,
        children,
        metadata,
    }))
}

pub fn syncback_project<'sync>(
    snapshot: &SyncbackSnapshot<'sync>,
) -> anyhow::Result<SyncbackReturn<'sync>> {
    let old_inst = snapshot
        .old_inst()
        .expect("projects should always exist in both trees");
    // Generally, the path of a project is the first thing added to the relevant
    // paths. So, we take the last one.
    let project_path = old_inst
        .metadata()
        .relevant_paths
        .last()
        .expect("all projects should have a relevant path");
    let vfs = snapshot.vfs();

    log::debug!(
        "Reloading project {} from vfs (name: {})",
        project_path.display(),
        snapshot.name
    );
    let mut project = Project::load_from_slice(&vfs.read(project_path)?, project_path)?;
    let base_path = project.folder_location().to_path_buf();

    // Sync rules for this project do not have their base rule set but it is
    // important when performing syncback on other projects.
    for rule in &mut project.sync_rules {
        rule.base_path = base_path.clone()
    }

    let mut descendant_snapshots = Vec::new();
    let mut removed_descendants = Vec::new();

    let mut ref_to_path_map = HashMap::new();
    let mut old_child_map = HashMap::new();
    let mut new_child_map = HashMap::new();

    let mut node_queue = VecDeque::with_capacity(1);
    node_queue.push_back((&mut project.tree, old_inst, snapshot.new_inst()));

    while let Some((node, old_inst, new_inst)) = node_queue.pop_front() {
        log::debug!("Processing node {}", old_inst.name());
        if old_inst.class_name() != new_inst.class {
            anyhow::bail!(
                "Cannot change the class of {} in project file {}",
                old_inst.name(),
                project_path.display()
            );
        }

        let filtered_properties = snapshot
            .get_filtered_properties(new_inst.referent())
            .unwrap();
        let properties = &mut node.properties;
        let mut attributes = BTreeMap::new();

        // We would ideally have different behavior here based on whether a
        // node has `$path` set. However, due to an issue with Middleware not
        // knowing whether they originate from a project or not, we just skip
        // writing metadata for things from projects. So to avoid properties
        // being dropped, we don't filter them specially.
        // TODO: We should handle this branching somehow so that meta.json files
        // are respected.
        for (name, value) in filtered_properties {
            match value {
                Variant::Attributes(attrs) => {
                    for (attr_name, attr_value) in attrs.iter() {
                        attributes.insert(
                            attr_name.clone(),
                            UnresolvedValue::from_variant_unambiguous(attr_value.clone()),
                        );
                    }
                }
                Variant::SharedString(_) => {
                    log::warn!(
                        "Rojo cannot serialize the property {}.{name} in project files.\n\
                        If this is not acceptable, resave the Instance at '{}' manually as an RBXM or RBXMX.", new_inst.class, snapshot.get_new_inst_path(snapshot.new)
                    );
                }
                _ => {
                    properties.insert(
                        name.to_string(),
                        UnresolvedValue::from_variant(value.clone(), &new_inst.class, name),
                    );
                }
            }
        }
        node.attributes = attributes;

        if node.path.is_some() {
            // Since the node has a path, we have to run syncback on it.
            let node_path = node.path.as_ref().map(PathNode::path).expect(
                "Project nodes with a path must have a path \
                If you see this message, something went seriously wrong. Please report it.",
            );
            let full_path = if node_path.is_absolute() {
                node_path.to_path_buf()
            } else {
                base_path.join(node_path)
            };

            let snapshot = syncback_project_node(
                snapshot,
                &project.sync_rules,
                &full_path,
                new_inst,
                old_inst,
            )?;
            descendant_snapshots.push(snapshot);

            ref_to_path_map.insert(new_inst.referent(), full_path);
        }

        for child_ref in new_inst.children() {
            let child = snapshot
                .get_new_instance(*child_ref)
                .expect("all children of Instances should be in new DOM");
            // As of writing (Feb 27 2024) Roblox serializes several Instances
            // with the class 'Instance' that all have the same name. To avoid
            // difficulties, we just ignore them. :-)
            if child.class == "Instance" {
                continue;
            }
            if new_child_map.insert(&child.name, child).is_some() {
                anyhow::bail!(
                    "Instances that are direct children of an Instance that is made by a project file \
                    must have a unique name.\nThe child '{}' of '{}' is duplicated.", child.name, old_inst.name()
                );
            }
        }
        for child_ref in old_inst.children() {
            let child = snapshot
                .get_old_instance(*child_ref)
                .expect("all children of Instances should be in old DOM");
            if old_child_map.insert(child.name(), child).is_some() {
                anyhow::bail!(
                    "Instances that are direct children of an Instance that is made by a project file \
                    must have a unique name.\nThe child '{}' of '{}' is duplicated.", child.name(), old_inst.name()
                );
            }
        }

        // This loop does basic matching of Instance children to the node's
        // children. It ensures that `new_child_map` and `old_child_map` will
        // only contain Instances that don't belong to the project after this.
        for (child_name, child_node) in &mut node.children {
            // If a node's path is optional, we want to skip it if the path
            // doesn't exist since it isn't in the current old DOM.
            if let Some(path) = &child_node.path {
                if path.is_optional() {
                    let real_path = if path.path().is_absolute() {
                        path.path().to_path_buf()
                    } else {
                        base_path.join(path.path())
                    };
                    if !real_path.exists() {
                        log::warn!(
                            "Skipping node '{child_name}' of project because it is optional and not present on the disk.\n\
                            If this is not deliberate, please create a file or directory at {}", real_path.display()
                        );
                        continue;
                    }
                }
            }
            let new_equivalent = new_child_map.remove(child_name);
            let old_equivalent = old_child_map.remove(child_name.as_str());
            // The panic below should never happen. If it does, something's gone
            // wrong with the Instance matching for nodes.
            match (new_equivalent, old_equivalent) {
                (Some(new), Some(old)) => node_queue.push_back((child_node, old, new)),
                (_, None) => anyhow::bail!(
                    "The child '{child_name}' of Instance '{}' would be removed.\n\
                    Syncback cannot add or remove Instances from project {}", old_inst.name(), project_path.display()),
                (None, _) => panic!(
                    "Invariant violated: the Instance matching of project nodes is flawed somehow.\n\
                    Specifically, a child of the node did not exist in the old tree."
                ),
            }
        }

        // All of the children in this loop are by their nature not in the
        // project, so we just need to run syncback on them.
        for (name, new_child) in new_child_map.drain() {
            let parent_path = match ref_to_path_map.get(&new_child.parent()) {
                Some(path) => path.clone(),
                None => {
                    log::debug!("Skipping child {name} of node because it has no parent_path");
                    continue;
                }
            };

            // If a child also exists in the old tree, it will be caught in the
            // syncback on the project node path above (or is itself a node).
            // So the only things we need to run seperately is new children.
            if old_child_map.remove(name.as_str()).is_none() {
                descendant_snapshots.push(snapshot.with_new_parent(
                    parent_path,
                    new_child.name.clone(),
                    new_child.referent(),
                    None,
                ));
            }
        }
        removed_descendants.extend(old_child_map.drain().map(|(_, v)| v));
    }

    Ok(SyncbackReturn {
        inst_snapshot: InstanceSnapshot::from_instance(snapshot.new_inst()),
        fs_snapshot: FsSnapshot::new()
            .with_added_file(project_path, serde_json::to_vec_pretty(&project)?),
        children: descendant_snapshots,
        removed_children: removed_descendants,
    })
}

fn syncback_project_node<'sync>(
    snapshot: &SyncbackSnapshot<'sync>,
    sync_rules: &[SyncRule],
    node_path: &Path,
    new_inst: &Instance,
    old_inst: InstanceWithMeta,
) -> anyhow::Result<SyncbackSnapshot<'sync>> {
    let (middleware, _, mut file_path) =
        Middleware::middleware_and_path(snapshot.vfs(), sync_rules, node_path)?
            .context("middleware_and_path should not fail for project nodes that exist")?;

    if !file_path.pop() {
        anyhow::bail!(
            "cannot syncback node {} using middleware {:?} because it is at the disk root",
            old_inst.name(),
            middleware
        )
    }
    Ok(snapshot
        .with_new_parent(
            file_path,
            old_inst.name().to_owned(),
            new_inst.referent(),
            Some(old_inst.id()),
        )
        .middleware(middleware))
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
    } else if parent_class == "Workspace" {
        // Workspace has a special Terrain class inside it
        if name == "Terrain" {
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

        let vfs = Vfs::new(imfs);

        let instance_snapshot = snapshot_project(
            &InstanceContext::default(),
            &vfs,
            Path::new("/foo"),
            "NOT_IN_SNAPSHOT",
        )
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

        let vfs = Vfs::new(imfs);

        let instance_snapshot = snapshot_project(
            &InstanceContext::default(),
            &vfs,
            Path::new("/foo/hello.project.json"),
            "NOT_IN_SNAPSHOT",
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

        let vfs = Vfs::new(imfs);

        let instance_snapshot = snapshot_project(
            &InstanceContext::default(),
            &vfs,
            Path::new("/foo.project.json"),
            "NOT_IN_SNAPSHOT",
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

        let vfs = Vfs::new(imfs);

        let instance_snapshot = snapshot_project(
            &InstanceContext::default(),
            &vfs,
            Path::new("/foo.project.json"),
            "NOT_IN_SNAPSHOT",
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

        let vfs = Vfs::new(imfs);

        let instance_snapshot = snapshot_project(
            &InstanceContext::default(),
            &vfs,
            Path::new("/foo.project.json"),
            "NOT_IN_SNAPSHOT",
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

        let vfs = Vfs::new(imfs);

        let instance_snapshot = snapshot_project(
            &InstanceContext::default(),
            &vfs,
            Path::new("/foo/default.project.json"),
            "NOT_IN_SNAPSHOT",
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

        let vfs = Vfs::new(imfs);

        let instance_snapshot = snapshot_project(
            &InstanceContext::default(),
            &vfs,
            Path::new("/foo/default.project.json"),
            "NOT_IN_SNAPSHOT",
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

        let vfs = Vfs::new(imfs);

        let instance_snapshot = snapshot_project(
            &InstanceContext::default(),
            &vfs,
            Path::new("/foo/default.project.json"),
            "NOT_IN_SNAPSHOT",
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

        let vfs = Vfs::new(imfs);

        let instance_snapshot = snapshot_project(
            &InstanceContext::default(),
            &vfs,
            Path::new("/foo/default.project.json"),
            "NOT_IN_SNAPSHOT",
        )
        .expect("snapshot error")
        .expect("snapshot returned no instances");

        insta::assert_yaml_snapshot!(instance_snapshot);
    }

    #[test]
    fn no_name_project() {
        let _ = env_logger::try_init();

        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot(
            "/foo",
            VfsSnapshot::dir(hashmap! {
                "default.project.json" => VfsSnapshot::file(r#"
                    {
                        "tree": {
                            "$className": "Model"
                        }
                    }
                "#),
            }),
        )
        .unwrap();

        let vfs = Vfs::new(imfs);

        let instance_snapshot = snapshot_project(
            &InstanceContext::default(),
            &vfs,
            Path::new("/foo/default.project.json"),
            "no_name_project",
        )
        .expect("snapshot error")
        .expect("snapshot returned no instances");

        insta::assert_yaml_snapshot!(instance_snapshot);
    }
}
