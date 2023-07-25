use std::{collections::HashSet, path::Path, str, sync::Arc};

use anyhow::{bail, Context};
use maplit::hashmap;
use memofs::{IoResultExt, Vfs};
use rbx_dom_weak::{
    types::{Ref, Variant},
    Instance, WeakDom,
};

use crate::snapshot::{
    DeepDiff, InstanceContext, InstanceMetadata, InstanceSnapshot, MiddlewareContextAny, RojoTree,
    SnapshotMiddleware, PRIORITY_SINGLE_READABLE,
};

use super::{
    meta_file::MetadataFile,
    util::{reconcile_meta_file, try_remove_file},
};

#[derive(Debug, PartialEq, Eq)]
pub struct LuaMiddleware;

impl SnapshotMiddleware for LuaMiddleware {
    fn middleware_id(&self) -> &'static str {
        "lua"
    }

    fn default_globs(&self) -> &[&'static str] {
        &["**/*.lua", "**/*.luau"]
    }

    fn init_names(&self) -> &[&'static str] {
        &[
            "init.lua",
            "init.luau",
            "init.server.lua",
            "init.server.luau",
            "init.client.lua",
            "init.client.luau",
        ]
    }

    fn snapshot(
        &self,
        context: &InstanceContext,
        vfs: &Vfs,
        path: &Path,
    ) -> anyhow::Result<Option<InstanceSnapshot>> {
        let (script_type, instance_name) = get_script_type_and_name(path);

        let class_name = match script_type {
            Some(ScriptType::Server) => "Script",
            Some(ScriptType::Client) => "LocalScript",
            Some(ScriptType::Module) => "ModuleScript",
            None => return Ok(None),
        };

        let contents = vfs.read(path)?;
        let contents_str = str::from_utf8(&contents)
            .with_context(|| format!("File was not valid UTF-8: {}", path.display()))?
            .to_owned();

        let meta_path = path.with_file_name(format!("{}.meta.json", instance_name));

        let mut snapshot = InstanceSnapshot::new()
            .name(instance_name)
            .class_name(class_name)
            .properties(hashmap! {
                "Source".to_owned() => contents_str.into(),
            })
            .metadata(
                InstanceMetadata::new()
                    .instigating_source(path)
                    .relevant_paths(vec![path.to_path_buf(), meta_path.clone()])
                    .context(context)
                    .middleware_id(self.middleware_id()),
            );

        if let Some(meta_contents) = vfs.read(&meta_path).with_not_found()? {
            let mut metadata = MetadataFile::from_slice(&meta_contents, meta_path)?;
            metadata.apply_all(&mut snapshot)?;
        }

        Ok(Some(snapshot))
    }

    fn syncback_priority(
        &self,
        _dom: &rbx_dom_weak::WeakDom,
        instance: &rbx_dom_weak::Instance,
        consider_descendants: bool,
    ) -> Option<i32> {
        if consider_descendants && !instance.children().is_empty() {
            return None;
        }

        if instance.class == "Script"
            || instance.class == "LocalScript"
            || instance.class == "ModuleScript"
        {
            Some(PRIORITY_SINGLE_READABLE)
        } else {
            None
        }
    }

    fn syncback_update(
        &self,
        vfs: &Vfs,
        old_path: &Path,
        diff: &DeepDiff,
        tree: &mut RojoTree,
        old_ref: Ref,
        new_dom: &WeakDom,
        context: &InstanceContext,
        middleware_context: Option<Arc<dyn MiddlewareContextAny>>,
    ) -> anyhow::Result<InstanceMetadata> {
        let old_inst = tree.get_instance(old_ref).unwrap();

        let new_ref = diff
            .get_matching_new_ref(old_ref)
            .with_context(|| "no matching new ref")?;
        let new_inst = new_dom.get_by_ref(new_ref).with_context(|| "missing ref")?;

        let my_metadata = old_inst.metadata().clone();

        let (_old_script_type, name) = get_script_type_and_name(old_path);

        let ext = match old_path.extension().map(|v| v.to_string_lossy()).as_deref() {
            Some("lua") => "lua",
            Some("luau") => "luau",
            _ => "lua",
        };

        let new_file_name = match new_inst.class.as_str() {
            "Script" => format!("{}.server.{}", name, ext),
            "LocalScript" => format!("{}.client.{}", name, ext),
            "ModuleScript" => format!("{}.{}", name, ext),
            _ => bail!("Bad class when syncing back Lua: {:?}", new_inst.class),
        };
        let new_path = old_path.with_file_name(new_file_name);

        vfs.write(&new_path, get_instance_contents(new_inst)?)?;

        reconcile_meta_file(
            vfs,
            &new_path.with_file_name(format!("{}.meta.json", name)),
            new_inst,
            ignore_props(),
            Some(&new_inst.class),
        )?;

        Ok(my_metadata
            .instigating_source(new_path.clone())
            .context(context)
            .relevant_paths(vec![
                new_path.clone(),
                new_path.with_file_name(format!("{}.meta.json", name)),
            ])
            .middleware_id(self.middleware_id()))
    }

    fn syncback_new(
        &self,
        vfs: &Vfs,
        parent_path: &Path,
        name: &str,
        new_dom: &WeakDom,
        new_ref: Ref,
        context: &InstanceContext,
    ) -> anyhow::Result<InstanceSnapshot> {
        let instance = new_dom.get_by_ref(new_ref).unwrap();

        let file_name = match instance.class.as_str() {
            "Script" => format!("{}.server.lua", name),
            "LocalScript" => format!("{}.client.lua", name),
            "ModuleScript" => format!("{}.lua", name),
            _ => bail!("Bad class when syncing back Lua: {:?}", instance.class),
        };

        let path = parent_path.join(file_name);

        vfs.write(&path, get_instance_contents(instance)?)?;

        reconcile_meta_file(
            vfs,
            &path.with_file_name(format!("{}.meta.json", name)),
            instance,
            ignore_props(),
            Some(&instance.class),
        )?;

        Ok(
            InstanceSnapshot::from_tree_copy(new_dom, new_ref, false).metadata(
                InstanceMetadata::new()
                    .context(context)
                    .instigating_source(path.clone())
                    .relevant_paths(vec![
                        path.clone(),
                        path.with_file_name(format!("{}.meta.json", name)),
                    ])
                    .middleware_id(self.middleware_id()),
            ),
        )
    }

    fn syncback_destroy(
        &self,
        vfs: &Vfs,
        path: &Path,
        _tree: &mut RojoTree,
        _old_ref: Ref,
    ) -> anyhow::Result<()> {
        let (_script_type, name) = get_script_type_and_name(path);

        vfs.remove_file(path)?;
        try_remove_file(vfs, &path.with_file_name(format!("{}.meta.json", name)))?;
        Ok(())
    }
}

fn get_instance_contents(instance: &Instance) -> anyhow::Result<&str> {
    Ok(match instance.properties.get("Source") {
        Some(Variant::String(contents)) => contents.as_str(),
        Some(Variant::BinaryString(contents)) => str::from_utf8(&contents.as_ref())?,
        Some(Variant::SharedString(contents)) => str::from_utf8(&contents.data())?,
        _ => bail!("Script.Source was not a string or was missing"),
    })
}

fn ignore_props() -> HashSet<&'static str> {
    HashSet::from(["Source", "ClassName", "ScriptGuid", "LinkedSource"])
}

pub enum ScriptType {
    Client,
    Server,
    Module,
}

fn get_script_type_and_name(path: &Path) -> (Option<ScriptType>, String) {
    let file_name = path.file_name().unwrap().to_string_lossy();
    let file_name_parts: Vec<&str> = file_name.split(".").collect();

    if file_name_parts.len() >= 3 {
        let ext_prefix = file_name_parts[file_name_parts.len() - 2].to_lowercase();
        let name = file_name_parts[0..(file_name_parts.len() - 2)].join(".");
        match ext_prefix.as_str() {
            "client" => return (Some(ScriptType::Client), name),
            "server" => return (Some(ScriptType::Server), name),
            _ => return (Some(ScriptType::Module), format!("{}.{}", name, ext_prefix)),
        }
    }

    (
        Some(ScriptType::Module),
        path.file_stem().unwrap().to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod test {
    use super::*;

    use memofs::{InMemoryFs, VfsSnapshot};

    #[test]
    fn module_from_vfs() {
        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot("/foo.lua", VfsSnapshot::file("Hello there!"))
            .unwrap();

        let mut vfs = Vfs::new(imfs);

        let instance_snapshot = LuaMiddleware
            .snapshot(&InstanceContext::default(), &mut vfs, Path::new("/foo.lua"))
            .unwrap()
            .unwrap();

        insta::assert_yaml_snapshot!(instance_snapshot);
    }

    #[test]
    fn server_from_vfs() {
        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot("/foo.server.lua", VfsSnapshot::file("Hello there!"))
            .unwrap();

        let mut vfs = Vfs::new(imfs);

        let instance_snapshot = LuaMiddleware
            .snapshot(
                &InstanceContext::default(),
                &mut vfs,
                Path::new("/foo.server.lua"),
            )
            .unwrap()
            .unwrap();

        insta::assert_yaml_snapshot!(instance_snapshot);
    }

    #[test]
    fn client_from_vfs() {
        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot("/foo.client.lua", VfsSnapshot::file("Hello there!"))
            .unwrap();

        let mut vfs = Vfs::new(imfs);

        let instance_snapshot = LuaMiddleware
            .snapshot(
                &InstanceContext::default(),
                &mut vfs,
                Path::new("/foo.client.lua"),
            )
            .unwrap()
            .unwrap();

        insta::assert_yaml_snapshot!(instance_snapshot);
    }

    #[ignore = "init.lua functionality has moved to the root snapshot function"]
    #[test]
    fn init_module_from_vfs() {
        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot(
            "/root",
            VfsSnapshot::dir(hashmap! {
                "init.lua" => VfsSnapshot::file("Hello!"),
            }),
        )
        .unwrap();

        let mut vfs = Vfs::new(imfs);

        let instance_snapshot = LuaMiddleware
            .snapshot(&InstanceContext::default(), &mut vfs, Path::new("/root"))
            .unwrap()
            .unwrap();

        insta::assert_yaml_snapshot!(instance_snapshot);
    }

    #[test]
    fn module_with_meta() {
        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot("/foo.lua", VfsSnapshot::file("Hello there!"))
            .unwrap();
        imfs.load_snapshot(
            "/foo.meta.json",
            VfsSnapshot::file(
                r#"
                    {
                        "ignoreUnknownInstances": true
                    }
                "#,
            ),
        )
        .unwrap();

        let mut vfs = Vfs::new(imfs);

        let instance_snapshot = LuaMiddleware
            .snapshot(&InstanceContext::default(), &mut vfs, Path::new("/foo.lua"))
            .unwrap()
            .unwrap();

        insta::assert_yaml_snapshot!(instance_snapshot);
    }

    #[test]
    fn script_with_meta() {
        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot("/foo.server.lua", VfsSnapshot::file("Hello there!"))
            .unwrap();
        imfs.load_snapshot(
            "/foo.meta.json",
            VfsSnapshot::file(
                r#"
                    {
                        "ignoreUnknownInstances": true
                    }
                "#,
            ),
        )
        .unwrap();

        let mut vfs = Vfs::new(imfs);

        let instance_snapshot = LuaMiddleware
            .snapshot(
                &InstanceContext::default(),
                &mut vfs,
                Path::new("/foo.server.lua"),
            )
            .unwrap()
            .unwrap();

        insta::assert_yaml_snapshot!(instance_snapshot);
    }

    #[test]
    fn script_disabled() {
        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot("/bar.server.lua", VfsSnapshot::file("Hello there!"))
            .unwrap();
        imfs.load_snapshot(
            "/bar.meta.json",
            VfsSnapshot::file(
                r#"
                    {
                        "properties": {
                            "Disabled": true
                        }
                    }
                "#,
            ),
        )
        .unwrap();

        let mut vfs = Vfs::new(imfs);

        let instance_snapshot = LuaMiddleware
            .snapshot(
                &InstanceContext::default(),
                &mut vfs,
                Path::new("/bar.server.lua"),
            )
            .unwrap()
            .unwrap();

        insta::with_settings!({ sort_maps => true }, {
            insta::assert_yaml_snapshot!(instance_snapshot);
        });
    }
}
