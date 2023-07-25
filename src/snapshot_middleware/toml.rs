use std::{path::Path, sync::Arc};

use anyhow::Context;
use maplit::hashmap;
use memofs::{IoResultExt, Vfs};

use crate::{
    lua_ast::{Expression, Statement},
    snapshot::{
        InstanceContext, InstanceMetadata, InstanceSnapshot, MiddlewareContextAny,
        SnapshotMiddleware,
    },
};

use super::{
    meta_file::MetadataFile,
    util::{try_remove_file, PathExt},
};

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
                    .snapshot_middleware(self.middleware_id()),
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

    fn syncback_update(
        &self,
        _vfs: &Vfs,
        _path: &Path,
        _diff: &crate::snapshot::DeepDiff,
        _tree: &mut crate::snapshot::RojoTree,
        _old_ref: rbx_dom_weak::types::Ref,
        _new_dom: &rbx_dom_weak::WeakDom,
        _context: &InstanceContext,
        _middleware_context: Option<Arc<dyn MiddlewareContextAny>>,
    ) -> anyhow::Result<InstanceMetadata> {
        todo!()
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
        todo!()
    }

    fn syncback_destroy(
        &self,
        vfs: &Vfs,
        path: &Path,
        _tree: &mut crate::snapshot::RojoTree,
        _old_ref: rbx_dom_weak::types::Ref,
    ) -> anyhow::Result<()> {
        vfs.remove_file(path)?;
        try_remove_file(vfs, &path.with_extension("meta.json"))?;
        Ok(())
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
