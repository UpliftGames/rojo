use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{bail, Context};
use memofs::{DirEntry, IoResultExt, Vfs};
use rbx_dom_weak::{types::Ref, Instance, WeakDom};

use crate::{
    snapshot::{
        get_best_syncback_middleware, get_best_syncback_middleware_must_not_serialize_children,
        get_best_syncback_middleware_sorted, DeepDiff, FsSnapshot, GetChildren, InstanceContext,
        InstanceMetadata, InstanceSnapshot, InstigatingSource, MiddlewareContextAny,
        MiddlewareContextArc, RojoTree, SnapshotMiddleware, SnapshotOverride,
        SnapshotOverrideTrait, SyncbackNode, SyncbackPlanner, PRIORITY_DIRECTORY_CHECK_FALLBACK,
        PRIORITY_MANY_READABLE, PRIORITY_MODEL_DIRECTORY,
    },
    snapshot_middleware::{get_middleware, get_middleware_inits},
};

use super::{
    get_middlewares, meta_file::MetadataFile, snapshot_from_vfs, util::reconcile_meta_file,
};

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct DirectoryMiddlewareContext {
    init_middleware: Option<&'static str>,
    init_context: Option<Arc<dyn MiddlewareContextAny>>,
    init_path: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct DirectoryMiddleware;

impl SnapshotMiddleware for DirectoryMiddleware {
    fn middleware_id(&self) -> &'static str {
        "directory"
    }

    fn match_only_directories(&self) -> bool {
        true
    }

    fn default_globs(&self) -> &[&'static str] {
        &["**/"]
    }

    fn init_names(&self) -> &[&'static str] {
        &[]
    }

    fn snapshot(
        &self,
        context: &InstanceContext,
        vfs: &Vfs,
        path: &Path,
    ) -> anyhow::Result<Option<InstanceSnapshot>> {
        let mut snapshot = match snapshot_dir_no_meta(context, vfs, path)? {
            Some(snapshot) => snapshot,
            None => return Ok(None),
        };

        if let Some(mut meta) = dir_meta(vfs, path)? {
            meta.apply_all(&mut snapshot)?;
        }

        snapshot.metadata.middleware_id = Some(self.middleware_id());

        Ok(Some(snapshot))
    }

    fn syncback_serializes_children(&self) -> bool {
        true
    }

    fn syncback_priority(
        &self,
        _dom: &WeakDom,
        instance: &Instance,
        consider_descendants: bool,
    ) -> Option<i32> {
        if instance.class == "Folder" {
            if consider_descendants {
                Some(PRIORITY_MANY_READABLE)
            } else {
                Some(PRIORITY_DIRECTORY_CHECK_FALLBACK)
            }
        } else {
            Some(PRIORITY_MODEL_DIRECTORY)
        }
    }

    // fn syncback_update(
    //     &self,
    //     vfs: &Vfs,
    //     path: &Path,
    //     diff: &DeepDiff,
    //     tree: &mut RojoTree,
    //     old_ref: Ref,
    //     new_dom: &WeakDom,
    //     context: &InstanceContext,
    //     middleware_context: Option<Arc<dyn MiddlewareContextAny>>,
    //     overrides: Option<SnapshotOverride>,
    // ) -> anyhow::Result<InstanceMetadata> {
    //     log::trace!("Updating dir {}", path.display());
    //     let mut my_metadata = tree.get_metadata(old_ref).unwrap().clone();
    //     let mut sub_middleware_id = None;
    //     if diff.has_changed_properties(old_ref) {
    //         let _old_inst = tree.get_instance(old_ref).unwrap();

    //         let new_ref = diff
    //             .get_matching_new_ref(old_ref)
    //             .with_context(|| "no matching new ref")?;
    //         let new_inst = new_dom.get_by_ref(new_ref).with_context(|| "missing ref")?;

    //         let syncback_context = if let Some(middleware_context) = middleware_context {
    //             let middleware_context = middleware_context.as_ref();

    //             let middleware_context =
    //                 middleware_context.downcast_ref::<DirectoryMiddlewareContext>();

    //             middleware_context.cloned()
    //         } else {
    //             None
    //         };

    //         let syncback_context = syncback_context.as_ref();

    //         let init_middleware = syncback_context.map(|v| v.init_middleware).flatten();

    //         let best_middleware =
    //             get_best_syncback_middleware_sorted(tree.inner(), new_inst, false, init_middleware)
    //                 .map(|mut iter| {
    //                     iter.find(|&v| !get_middlewares()[v].syncback_serializes_children())
    //                 })
    //                 .flatten();

    //         sub_middleware_id = best_middleware;

    //         if best_middleware == init_middleware && best_middleware.is_some() {
    //             let best_middleware = best_middleware.unwrap();

    //             let syncback_context = syncback_context.unwrap();
    //             let init_path = syncback_context
    //                 .init_path
    //                 .as_ref()
    //                 .with_context(|| "missing existing init path")?;

    //             // todo: pass in correct context. right now it will grab the wrong one (using code like above)!
    //             let new_init_metadata = get_middlewares()[best_middleware]
    //                 .syncback_update(
    //                     vfs,
    //                     &init_path,
    //                     diff,
    //                     tree,
    //                     old_ref,
    //                     new_dom,
    //                     context,
    //                     syncback_context.init_context.clone(),
    //                     None,
    //                 )
    //                 .with_context(|| "failed to create instance on filesystem")?;

    //             tree.update_props(old_ref, new_inst);

    //             my_metadata.middleware_context = Some(Arc::new(DirectoryMiddlewareContext {
    //                 init_middleware: new_init_metadata.middleware_id.clone(),
    //                 init_context: new_init_metadata.middleware_context.clone(),
    //                 init_path: new_init_metadata
    //                     .snapshot_source_path()
    //                     .map(|v| v.to_path_buf()),
    //             }));
    //         } else {
    //             // tear down fs via syncback
    //             if let Some(existing_middleware) = init_middleware {
    //                 let syncback_context = syncback_context.unwrap();
    //                 let init_path = syncback_context
    //                     .init_path
    //                     .as_ref()
    //                     .with_context(|| "missing exiting init path")?;

    //                 get_middlewares()[existing_middleware]
    //                     .syncback_destroy(vfs, &init_path, tree, old_ref)?;
    //             }

    //             if let Some(best_middleware) = best_middleware {
    //                 // reconstruct fs via syncback
    //                 let new_init_snapshot = get_middlewares()[best_middleware]
    //                     .syncback_new(vfs, path, "init", new_dom, new_ref, &context, None)
    //                     .with_context(|| "failed to create instance on filesystem")?;

    //                 let new_init_snapshot = match new_init_snapshot {
    //                     Some(v) => v,
    //                     None => bail!("failed to create instance on filesystem: target is disallowed by ignore paths"),
    //                 };

    //                 let new_init_metadata = new_init_snapshot.metadata;

    //                 tree.update_props(old_ref, new_inst);

    //                 my_metadata.middleware_context = Some(Arc::new(DirectoryMiddlewareContext {
    //                     init_middleware: new_init_metadata.middleware_id.clone(),
    //                     init_context: new_init_metadata.middleware_context.clone(),
    //                     init_path: new_init_metadata
    //                         .snapshot_source_path()
    //                         .map(|v| v.to_path_buf()),
    //                 }));
    //             } else {
    //                 my_metadata.middleware_context = None;

    //                 reconcile_meta_file(
    //                     vfs,
    //                     &path.join("init.meta.json"),
    //                     new_inst,
    //                     HashSet::new(),
    //                     Some(overrides.known_class_or("Folder")),
    //                     &context.syncback.property_filters_save,
    //                 )?;
    //             }
    //         }
    //     }

    //     if diff.has_changed_descendants(old_ref) && sub_middleware_id != Some("project") {
    //         tree.syncback_children(vfs, diff, old_ref, path, new_dom, context)?;
    //     }

    //     Ok(my_metadata)
    // }

    fn syncback_new_path(
        &self,
        parent_path: &Path,
        name: &str,
        _instance: &Instance,
    ) -> anyhow::Result<std::path::PathBuf> {
        Ok(parent_path.join(name))
    }

    fn syncback(
        &self,
        vfs: &Vfs,
        new_path: &Path,
        old: Option<(&mut RojoTree, Ref, Option<MiddlewareContextArc>)>,
        new: (&WeakDom, Ref),
        metadata: &InstanceMetadata,
        overrides: Option<SnapshotOverride>,
    ) -> anyhow::Result<SyncbackNode> {
        let mut metadata = metadata.clone();

        let (new_dom, new_ref, _) = new;
        let new_inst = new_dom.get_by_ref(new_ref).with_context(|| "missing ref")?;

        log::trace!("New dir {}", new_path.display());

        metadata.middleware_id = Some(self.middleware_id());
        metadata.instigating_source = Some(InstigatingSource::Path(new_path.to_path_buf()));
        metadata.relevant_paths = get_middleware_inits()
            .iter()
            .map(|(&init_name, _)| new_path.join(init_name))
            .collect();

        let mut fs_snapshot = FsSnapshot::new().with_dir(new_path);
        let mut sub_children = None;

        let sub_middleware = get_best_syncback_middleware_must_not_serialize_children(
            new_dom, new_inst, false, None,
        );

        if let Some(sub_middleware) = sub_middleware {
            let new_file_path =
                get_middleware(sub_middleware).syncback_new_path(new_path, "init", new_inst)?;

            let sub_node = get_middlewares()[sub_middleware]
                .syncback(
                    vfs,
                    &new_file_path,
                    None,
                    (new_dom, new_ref),
                    &InstanceMetadata::new().context(&metadata.context),
                    None,
                )
                .with_context(|| "failed to create instance on filesystem")?;

            metadata.middleware_context = Some(Arc::new(DirectoryMiddlewareContext {
                init_middleware: sub_node.instance_snapshot.metadata.middleware_id.clone(),
                init_context: sub_node
                    .instance_snapshot
                    .metadata
                    .middleware_context
                    .clone(),
                init_path: sub_node
                    .instance_snapshot
                    .metadata
                    .snapshot_source_path()
                    .map(|v| v.to_path_buf()),
            }));

            sub_children = sub_node.get_children;

            if let Some(sub_fs_snapshot) = &sub_node.instance_snapshot.metadata.fs_snapshot {
                fs_snapshot = fs_snapshot.merge_with(sub_fs_snapshot);
            }
        } else {
            let meta = reconcile_meta_file(
                vfs,
                &new_path.join("init.meta.json"),
                new_inst,
                HashSet::new(),
                Some(overrides.known_class_or("Folder")),
                &metadata.context.syncback.property_filters_save,
            )?;

            fs_snapshot =
                fs_snapshot.with_file_contents_opt(&new_path.join("init.meta.json"), meta);
        }

        metadata.fs_snapshot = Some(fs_snapshot);

        Ok(SyncbackNode::new(
            InstanceSnapshot::new()
                .class_name(&new_inst.class)
                .metadata(metadata)
                .name(&new_inst.name)
                .properties(new_inst.properties.clone()),
        )
        .with_children(move || {
            let children = Vec::new();

            if let Some(sub_children) = sub_children {
                children.extend(sub_children()?);
            }

            if sub_middleware != Some("project") {
                for child_ref in new_inst.children() {
                    let child_inst = new_dom
                        .get_by_ref(*child_ref)
                        .with_context(|| "missing ref")?;
                    let child_middleware =
                        get_best_syncback_middleware(new_dom, child_inst, true, None);

                    if let Some(child_middleware) = child_middleware {
                        let child_path = get_middleware(child_middleware).syncback_new_path(
                            new_path,
                            &child_inst.name,
                            child_inst,
                        )?;

                        let child_snapshot = get_middlewares()[child_middleware]
                            .syncback(
                                vfs,
                                &child_path,
                                None,
                                (new_dom, *child_ref),
                                &InstanceMetadata::new().context(&metadata.context),
                                None,
                            )
                            .with_context(|| "failed to create instance on filesystem")?;

                        children.push(child_snapshot);
                    } // TODO: warn on skipping (or fail early?)
                }
            }

            Ok(children)
        }))
    }
}

fn syncback_update(
    vfs: &Vfs,
    diff: &DeepDiff,
    path: &Path,
    old: (&mut RojoTree, Ref, Option<MiddlewareContextArc>),
    new: (&WeakDom, Ref),
    metadata: &InstanceMetadata,
    overrides: Option<SnapshotOverride>,
) -> anyhow::Result<SyncbackNode> {
    let mut metadata = metadata.clone();

    let (old_dom, old_ref, dir_context) = old;
    let old_inst = old_dom
        .get_instance(old_ref)
        .with_context(|| "missing ref")?;

    let dir_context = match dir_context {
        Some(middleware_context) => Some(
            middleware_context
                .downcast_ref::<DirectoryMiddlewareContext>()
                .with_context(|| "middleware context was of wrong type")?,
        ),
        None => None,
    };

    let (new_dom, new_ref) = new;
    let new_inst = new_dom.get_by_ref(new_ref).with_context(|| "missing ref")?;

    log::trace!("Update dir {}", path.display());

    metadata.middleware_id = Some("directory");
    metadata.instigating_source = Some(InstigatingSource::Path(path.to_path_buf()));
    metadata.relevant_paths = get_middleware_inits()
        .iter()
        .map(|(&init_name, _)| path.join(init_name))
        .collect();

    let mut fs_snapshot = FsSnapshot::new().with_dir(path);

    let mut init_children = None;
    let mut init_middleware = None;

    {
        let mut init_old = None;
        let mut init_path = None;

        let old_init_middleware_pack = match dir_context {
            Some(middleware_context) => (
                middleware_context.init_middleware,
                middleware_context.init_path,
                middleware_context.init_context,
            ),
            None => (None, None, None),
        };

        match old_init_middleware_pack {
            (Some(old_init_middleware), Some(old_init_path), old_init_context) => {
                init_middleware = get_best_syncback_middleware_must_not_serialize_children(
                    new_dom,
                    new_inst,
                    false,
                    Some(old_init_middleware),
                );

                if let Some(init_middleware) = init_middleware {
                    init_old = Some((old_dom, old_ref, old_init_context));
                    init_path = Some(old_init_path);
                }
            }
            (Some(init_middleware), None, _) => {
                bail!("Missing path for existing middleware")
            }
            (None, _, _) => {
                // TODO: deduplicate this
                init_middleware = get_best_syncback_middleware_must_not_serialize_children(
                    new_dom,
                    new_inst,
                    false,
                    old_init_middleware_pack.0,
                );

                if let Some(init_middleware) = init_middleware {
                    let init_file_path = get_middleware(init_middleware)
                        .syncback_new_path(path, "init", new_inst)?;
                }
            }
        }

        if let Some(init_middleware) = init_middleware {
            let init_path = init_path.unwrap();
            let init_node = get_middleware(init_middleware)
                .syncback(
                    vfs,
                    &init_path,
                    init_old,
                    (new_dom, new_ref),
                    &InstanceMetadata::new().context(&metadata.context),
                    None,
                )
                .with_context(|| "failed to create instance on filesystem")?;

            metadata.middleware_context = Some(Arc::new(DirectoryMiddlewareContext {
                init_middleware: init_node.instance_snapshot.metadata.middleware_id.clone(),
                init_context: init_node
                    .instance_snapshot
                    .metadata
                    .middleware_context
                    .clone(),
                init_path: init_node
                    .instance_snapshot
                    .metadata
                    .snapshot_source_path()
                    .map(|v| v.to_path_buf()),
            }));

            init_children = init_node.get_children;

            if let Some(sub_fs_snapshot) = &init_node.instance_snapshot.metadata.fs_snapshot {
                fs_snapshot = fs_snapshot.merge_with(sub_fs_snapshot);
            }
        } else {
            let meta = reconcile_meta_file(
                vfs,
                &path.join("init.meta.json"),
                new_inst,
                HashSet::new(),
                Some(overrides.known_class_or("Folder")),
                &metadata.context.syncback.property_filters_save,
            )?;

            fs_snapshot = fs_snapshot.with_file_contents_opt(&path.join("init.meta.json"), meta);
        }
    }

    metadata.fs_snapshot = Some(fs_snapshot);

    Ok(SyncbackNode::new(
        InstanceSnapshot::new()
            .class_name(&new_inst.class)
            .metadata(metadata)
            .name(&new_inst.name)
            .properties(new_inst.properties.clone()),
    )
    .with_children(move || {
        let children = Vec::new();

        if let Some(sub_children) = init_children {
            children.extend(sub_children()?);
        }

        if init_middleware != Some("project") {
            let (added, removed, changed, unchanged) = diff
                .get_children(old_dom.inner(), new_dom, old_ref)
                .with_context(|| "diff failed")?;

            let plans = Vec::new();

            for child_ref in added {
                if let Some(plan) = SyncbackPlanner::from_new(path, new_dom, child_ref)? {
                    children.push(plan.syncback(vfs, diff, overrides)?);
                }
            }

            for old_child_ref in changed {
                let new_child_ref = diff
                    .get_matching_new_ref(old_child_ref)
                    .with_context(|| "missing ref")?;
                if let Some(plan) =
                    SyncbackPlanner::from_update(old_dom, old_child_ref, new_dom, new_child_ref)?
                {
                    children.push(plan.syncback(vfs, diff, overrides)?);
                }
            }

            Ok((children, removed))
        }
    }))
}

fn syncback_new(
    vfs: &Vfs,
    diff: &DeepDiff,
    path: &Path,
    new: (&WeakDom, Ref),
    metadata: &InstanceMetadata,
    overrides: Option<SnapshotOverride>,
) -> anyhow::Result<SyncbackNode> {
    let mut metadata = metadata.clone();

    let (new_dom, new_ref) = new;
    let new_inst = new_dom.get_by_ref(new_ref).with_context(|| "missing ref")?;

    log::trace!("New dir {}", path.display());

    metadata.middleware_id = Some("directory");
    metadata.instigating_source = Some(InstigatingSource::Path(path.to_path_buf()));
    metadata.relevant_paths = get_middleware_inits()
        .iter()
        .map(|(&init_name, _)| path.join(init_name))
        .collect();

    let mut fs_snapshot = FsSnapshot::new().with_dir(path);
    let mut init_children = None;

    let init_middleware =
        get_best_syncback_middleware_must_not_serialize_children(new_dom, new_inst, false, None);

    if let Some(init_middleware) = init_middleware {
        let init_file_path =
            get_middleware(init_middleware).syncback_new_path(path, "init", new_inst)?;

        let init_node = get_middlewares()[init_middleware]
            .syncback(
                vfs,
                &init_file_path,
                None,
                (new_dom, new_ref),
                &InstanceMetadata::new().context(&metadata.context),
                None,
            )
            .with_context(|| "failed to create instance on filesystem")?;

        metadata.middleware_context = Some(Arc::new(DirectoryMiddlewareContext {
            init_middleware: init_node.instance_snapshot.metadata.middleware_id.clone(),
            init_context: init_node
                .instance_snapshot
                .metadata
                .middleware_context
                .clone(),
            init_path: init_node
                .instance_snapshot
                .metadata
                .snapshot_source_path()
                .map(|v| v.to_path_buf()),
        }));

        init_children = init_node.get_children;

        if let Some(sub_fs_snapshot) = &init_node.instance_snapshot.metadata.fs_snapshot {
            fs_snapshot = fs_snapshot.merge_with(sub_fs_snapshot);
        }
    } else {
        let meta = reconcile_meta_file(
            vfs,
            &path.join("init.meta.json"),
            new_inst,
            HashSet::new(),
            Some(overrides.known_class_or("Folder")),
            &metadata.context.syncback.property_filters_save,
        )?;

        fs_snapshot = fs_snapshot.with_file_contents_opt(&path.join("init.meta.json"), meta);
    }

    metadata.fs_snapshot = Some(fs_snapshot);

    Ok(SyncbackNode::new(
        InstanceSnapshot::new()
            .class_name(&new_inst.class)
            .metadata(metadata)
            .name(&new_inst.name)
            .properties(new_inst.properties.clone()),
    )
    .with_children(move || {
        let children = Vec::new();

        if let Some(sub_children) = init_children {
            children.extend(sub_children()?);
        }

        if init_middleware != Some("project") {
            for child_ref in new_inst.children() {
                let child_inst = new_dom
                    .get_by_ref(*child_ref)
                    .with_context(|| "missing ref")?;
                let child_middleware =
                    get_best_syncback_middleware(new_dom, child_inst, true, None);

                if let Some(child_middleware) = child_middleware {
                    let child_path = get_middleware(child_middleware).syncback_new_path(
                        path,
                        &child_inst.name,
                        child_inst,
                    )?;

                    let child_snapshot = get_middlewares()[child_middleware]
                        .syncback(
                            vfs,
                            &child_path,
                            None,
                            (new_dom, *child_ref),
                            &InstanceMetadata::new().context(&metadata.context),
                            None,
                        )
                        .with_context(|| "failed to create instance on filesystem")?;

                    children.push(child_snapshot);
                } // TODO: warn on skipping (or fail early?)
            }
        }

        Ok(children)
    }))
}

/// Retrieves the meta file that should be applied for this directory, if it
/// exists.
pub fn dir_meta(vfs: &Vfs, path: &Path) -> anyhow::Result<Option<MetadataFile>> {
    let meta_path = path.join("init.meta.json");

    if let Some(meta_contents) = vfs.read(&meta_path).with_not_found()? {
        let metadata = MetadataFile::from_slice(&meta_contents, meta_path)?;
        Ok(Some(metadata))
    } else {
        Ok(None)
    }
}

/// Snapshot a directory without applying meta files; useful for if the
/// directory's ClassName will change before metadata should be applied. For
/// example, this can happen if the directory contains an `init.client.lua`
/// file.
pub fn snapshot_dir_no_meta(
    context: &InstanceContext,
    vfs: &Vfs,
    path: &Path,
) -> anyhow::Result<Option<InstanceSnapshot>> {
    let middlewares = get_middlewares();

    let passes_filter_rules = |child: &DirEntry| {
        context
            .path_ignore_rules
            .iter()
            .all(|rule| rule.passes(child.path()))
    };

    let mut init_names: HashMap<&str, &str> = HashMap::new();
    for (_, middleware) in middlewares.iter() {
        for &name in middleware.init_names() {
            init_names.insert(name, middleware.middleware_id());
        }
    }

    let mut snapshot_children = Vec::new();

    let mut snapshot_parent = None;
    let mut skip_default_children = false;

    for (&middleware_id, middleware) in middlewares.iter() {
        for &name in middleware.init_names() {
            let init_path = path.join(name);
            let metadata = vfs
                .metadata(&init_path)
                .map(Some)
                .or_else(|e| match e.kind() {
                    std::io::ErrorKind::NotFound => Ok(None),
                    _ => Err(e),
                })?;
            if let Some(_metadata) = metadata {
                if let Some(init_snapshot) = middleware.snapshot(context, vfs, &init_path)? {
                    if middleware_id == "project" {
                        skip_default_children = true;
                        snapshot_children = init_snapshot.children.clone(); // TODO: don't do this
                    }
                    snapshot_parent = Some(init_snapshot);
                    break;
                }
            }
        }
    }

    if !skip_default_children {
        for entry in vfs.read_dir(path)? {
            let entry = entry?;

            if !passes_filter_rules(&entry) {
                continue;
            }

            let init_middleware_id =
                init_names.get(entry.path().file_name().unwrap().to_string_lossy().as_ref());
            if let Some(&_init_middleware_id) = init_middleware_id {
                continue;
            }

            if let Some(child_snapshot) = snapshot_from_vfs(context, vfs, entry.path())? {
                snapshot_children.push(child_snapshot);
            }
        }
    }

    let instance_name = path
        .file_name()
        .expect("Could not extract file name")
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("File name was not valid UTF-8: {}", path.display()))?
        .to_string();

    let meta_path = path.join("init.meta.json");

    let mut relevant_paths = vec![path.to_path_buf(), meta_path.clone()];

    for (_, middleware) in middlewares {
        for &name in middleware.init_names() {
            relevant_paths.push(path.join(name));
        }
    }

    let snapshot = match snapshot_parent {
        None => InstanceSnapshot::new()
            .name(instance_name)
            .class_name("Folder")
            .children(snapshot_children)
            .metadata(
                InstanceMetadata::new()
                    .instigating_source(path)
                    .relevant_paths(relevant_paths)
                    .context(context),
            ),
        Some(init_snapshot) => {
            let mut syncback_context = None;
            if let Some(init_middleware_id) = init_snapshot.metadata.middleware_id {
                let init_path = match &init_snapshot.metadata.instigating_source {
                    Some(InstigatingSource::Path(init_path)) => init_path.clone(),
                    _ => bail!("Invalid InstigatingSource from init snapshot"),
                };

                syncback_context = Some(Arc::new(DirectoryMiddlewareContext {
                    init_middleware: Some(init_middleware_id),
                    init_context: init_snapshot.metadata.middleware_context.clone(),
                    init_path: Some(init_path),
                }) as Arc<dyn MiddlewareContextAny>);
            }

            init_snapshot
                .name(instance_name)
                .children(snapshot_children)
                .metadata(
                    InstanceMetadata::new()
                        .instigating_source(path)
                        .relevant_paths(relevant_paths)
                        .middleware_context(syncback_context)
                        .context(&context),
                )
        }
    };

    Ok(Some(snapshot))
}

#[cfg(test)]
mod test {
    use super::*;

    use maplit::hashmap;
    use memofs::{InMemoryFs, VfsSnapshot};

    #[test]
    fn empty_folder() {
        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot("/foo", VfsSnapshot::empty_dir())
            .unwrap();

        let mut vfs = Vfs::new(imfs);

        let instance_snapshot = DirectoryMiddleware
            .snapshot(&InstanceContext::default(), &mut vfs, Path::new("/foo"))
            .unwrap()
            .unwrap();

        insta::assert_yaml_snapshot!(instance_snapshot);
    }

    #[test]
    fn folder_in_folder() {
        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot(
            "/foo",
            VfsSnapshot::dir(hashmap! {
                "Child" => VfsSnapshot::empty_dir(),
            }),
        )
        .unwrap();

        let mut vfs = Vfs::new(imfs);

        let instance_snapshot = DirectoryMiddleware
            .snapshot(&InstanceContext::default(), &mut vfs, Path::new("/foo"))
            .unwrap()
            .unwrap();

        insta::assert_yaml_snapshot!(instance_snapshot);
    }
}
