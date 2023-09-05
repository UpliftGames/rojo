use std::path::Path;

use anyhow::Context;
use maplit::hashmap;
use memofs::{IoResultExt, Vfs};

use crate::{
    lua_ast::{Expression, Statement},
    snapshot::{
        FsSnapshot, InstanceContext, InstanceMetadata, InstanceSnapshot, OptOldTuple,
        SnapshotMiddleware, SyncbackArgs, SyncbackNode,
    },
};

use super::{meta_file::MetadataFile, util::PathExt};

#[derive(Debug, PartialEq, Eq)]
pub struct TomlMiddleware;

impl SnapshotMiddleware for TomlMiddleware {
    fn middleware_id(&self) -> &'static str {
        "toml"
    }

    fn default_globs(&self) -> &[&'static str] {
        &["**/*.toml"]
    }

    fn init_names(&self) -> &[&'static str] {
        &["init.toml"]
    }

    fn snapshot(
        &self,
        context: &InstanceContext,
        vfs: &Vfs,
        path: &Path,
    ) -> anyhow::Result<Option<InstanceSnapshot>> {
        let name = path.file_name_trim_extension()?;
        let contents = vfs.read(path)?;

        let value: toml::Value = toml::from_slice(&contents)
            .with_context(|| format!("File contains malformed TOML: {}", path.display()))?;

        let as_lua = toml_to_lua(value).to_string();

        let properties = hashmap! {
            "Source".to_owned() => as_lua.into(),
        };

        let meta_path = path.with_file_name(format!("{}.meta.json", name));

        let mut snapshot = InstanceSnapshot::new()
            .name(name)
            .class_name("ModuleScript")
            .properties(properties)
            .metadata(
                InstanceMetadata::new()
                    .instigating_source(path)
                    .relevant_paths(vec![path.to_path_buf(), meta_path.clone()])
                    .context(context)
                    .middleware_id(self.middleware_id())
                    .fs_snapshot(FsSnapshot::new().with_files(&[path, &meta_path])),
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
        _instance: &rbx_dom_weak::Instance,
        _consider_descendants: bool,
    ) -> Option<i32> {
        None
        // TODO: implement lua ast _reading_ so we can convert lua to toml
    }

    fn syncback_always_preserve_middleware(&self) -> bool {
        true
    }

    fn syncback_new_path(
        &self,
        parent_path: &Path,
        name: &str,
        _new_inst: &rbx_dom_weak::Instance,
    ) -> anyhow::Result<std::path::PathBuf> {
        Ok(parent_path.join(format!("{}.toml", name)))
    }

    fn syncback(&self, sync: &SyncbackArgs<'_, '_>) -> anyhow::Result<SyncbackNode> {
        log::error!(
            "Syncback for toml files not implemented; skipping syncback for {}",
            sync.path.display()
        );

        Ok(SyncbackNode::new(
            (sync.old.opt_id(), sync.ref_for_save()),
            sync.path,
            InstanceSnapshot::from_tree_copy(sync.new.0, sync.new.1, false)
                .metadata(
                    sync.metadata
                        .clone()
                        .instigating_source(sync.path.to_path_buf())
                        .relevant_paths(vec![
                            sync.path.to_path_buf(),
                            sync.path.with_extension("meta.json"),
                        ])
                        .middleware_id(self.middleware_id())
                        .fs_snapshot(
                            FsSnapshot::new()
                                .with_file(sync.path)
                                .with_file(sync.path.with_extension("meta.json")),
                            // data-less files are kept un-modified if they exist
                        ),
                )
                .preferred_ref(sync.ref_for_save()),
        ))
    }
}

fn toml_to_lua(value: toml::Value) -> Statement {
    Statement::Return(toml_to_lua_value(value))
}

fn toml_to_lua_value(value: toml::Value) -> Expression {
    use toml::Value;

    match value {
        Value::Datetime(value) => Expression::String(value.to_string()),
        Value::Boolean(value) => Expression::Bool(value),
        Value::Float(value) => Expression::Number(value),
        Value::Integer(value) => Expression::Number(value as f64),
        Value::String(value) => Expression::String(value),
        Value::Array(values) => {
            Expression::Array(values.into_iter().map(toml_to_lua_value).collect())
        }
        Value::Table(values) => Expression::table(
            values
                .into_iter()
                .map(|(key, value)| (key.into(), toml_to_lua_value(value)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use memofs::{InMemoryFs, VfsSnapshot};

    #[test]
    fn instance_from_vfs() {
        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot(
            "/foo.toml",
            VfsSnapshot::file(
                r#"
                  array = [1, 2, 3]
                  true = true
                  false = false
                  int = 1234
                  float = 1234.5452
                  "1invalidident" = "nice"

                  [object]
                  hello = "world"

                  [dates]
                  offset1 = 1979-05-27T00:32:00.999999-07:00
                  offset2 = 1979-05-27 07:32:00Z
                  localdatetime = 1979-05-27T07:32:00
                  localdate = 1979-05-27
                  localtime = 00:32:00.999999
                "#,
            ),
        )
        .unwrap();

        let mut vfs = Vfs::new(imfs.clone());

        let instance_snapshot = TomlMiddleware
            .snapshot(
                &InstanceContext::default(),
                &mut vfs,
                Path::new("/foo.toml"),
            )
            .unwrap()
            .unwrap();

        insta::assert_yaml_snapshot!(instance_snapshot);
    }
}
