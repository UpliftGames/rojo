use std::path::Path;

use anyhow::Context;
use maplit::hashmap;
use memofs::{IoResultExt, Vfs};

use crate::{
    lua_ast::{Expression, Statement},
    snapshot::{
        FsSnapshot, InstanceContext, InstanceMetadata, InstanceSnapshot, SnapshotMiddleware,
        SyncbackArgs, SyncbackNode,
    },
};

use super::{meta_file::MetadataFile, util::PathExt};

#[derive(Debug, PartialEq, Eq)]
pub struct JsonMiddleware;

impl SnapshotMiddleware for JsonMiddleware {
    fn middleware_id(&self) -> &'static str {
        "json"
    }

    fn default_globs(&self) -> &[&'static str] {
        &["**/*.json"]
    }

    fn exclude_globs(&self) -> &[&'static str] {
        &["**/*.meta.json", "**/*.project.json"]
    }

    fn init_names(&self) -> &[&'static str] {
        &["init.json"]
    }

    fn snapshot(
        &self,
        context: &InstanceContext,
        vfs: &Vfs,
        path: &Path,
    ) -> anyhow::Result<Option<InstanceSnapshot>> {
        let name = path.file_name_trim_extension()?;
        let contents = vfs.read(path)?;

        let value: serde_json::Value = serde_json::from_slice(&contents)
            .with_context(|| format!("File contains malformed JSON: {}", path.display()))?;

        let as_lua = json_to_lua(value).to_string();

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
        // TODO: implement lua ast _reading_ so we can convert lua to json
    }

    fn syncback_new_path(
        &self,
        _parent_path: &Path,
        _name: &str,
        _new_inst: &rbx_dom_weak::Instance,
    ) -> anyhow::Result<std::path::PathBuf> {
        todo!()
    }

    fn syncback(&self, _sync: &SyncbackArgs<'_, '_>) -> anyhow::Result<SyncbackNode> {
        todo!()
    }
}

fn json_to_lua(value: serde_json::Value) -> Statement {
    Statement::Return(json_to_lua_value(value))
}

fn json_to_lua_value(value: serde_json::Value) -> Expression {
    use serde_json::Value;

    match value {
        Value::Null => Expression::Nil,
        Value::Bool(value) => Expression::Bool(value),
        Value::Number(value) => Expression::Number(value.as_f64().unwrap()),
        Value::String(value) => Expression::String(value),
        Value::Array(values) => {
            Expression::Array(values.into_iter().map(json_to_lua_value).collect())
        }
        Value::Object(values) => Expression::table(
            values
                .into_iter()
                .map(|(key, value)| (key.into(), json_to_lua_value(value)))
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
            "/foo.json",
            VfsSnapshot::file(
                r#"{
                  "array": [1, 2, 3],
                  "object": {
                    "hello": "world"
                  },
                  "true": true,
                  "false": false,
                  "null": null,
                  "int": 1234,
                  "float": 1234.5452,
                  "1invalidident": "nice"
                }"#,
            ),
        )
        .unwrap();

        let mut vfs = Vfs::new(imfs.clone());

        let instance_snapshot = JsonMiddleware
            .snapshot(
                &InstanceContext::default(),
                &mut vfs,
                Path::new("/foo.json"),
            )
            .unwrap()
            .unwrap();

        insta::assert_yaml_snapshot!(instance_snapshot);
    }
}
