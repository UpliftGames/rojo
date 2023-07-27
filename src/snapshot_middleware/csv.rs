use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
};

use anyhow::{bail, Context};
use maplit::hashmap;
use memofs::{IoResultExt, Vfs};
use rbx_dom_weak::{types::Variant, Instance, WeakDom};
use serde::{Deserialize, Serialize};

use crate::snapshot::{
    FsSnapshot, InstanceContext, InstanceMetadata, InstanceSnapshot, OptOldTuple,
    SnapshotMiddleware, SyncbackContextX, SyncbackNode, PRIORITY_SINGLE_READABLE,
};

use super::{
    meta_file::MetadataFile,
    util::{reconcile_meta_file, PathExt},
};

#[derive(Debug, PartialEq, Eq)]
pub struct CsvMiddleware;

impl SnapshotMiddleware for CsvMiddleware {
    fn middleware_id(&self) -> &'static str {
        "csv"
    }

    fn default_globs(&self) -> &[&'static str] {
        &["**/*.csv"]
    }

    fn init_names(&self) -> &[&'static str] {
        &["init.csv"]
    }

    fn snapshot(
        &self,
        _context: &InstanceContext,
        vfs: &Vfs,
        path: &Path,
    ) -> anyhow::Result<Option<InstanceSnapshot>> {
        let name = path.file_name_trim_extension()?;

        let meta_path = path.with_file_name(format!("{}.meta.json", name));
        let contents = vfs.read(path)?;

        let table_contents = convert_localization_csv(&contents).with_context(|| {
            format!(
                "File was not a valid LocalizationTable CSV file: {}",
                path.display()
            )
        })?;

        let mut snapshot = InstanceSnapshot::new()
            .name(name)
            .class_name("LocalizationTable")
            .properties(hashmap! {
                "Contents".to_owned() => table_contents.into(),
            })
            .metadata(
                InstanceMetadata::new()
                    .instigating_source(path)
                    .relevant_paths(vec![path.to_path_buf(), meta_path.clone()])
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
        _dom: &WeakDom,
        instance: &rbx_dom_weak::Instance,
        consider_descendants: bool,
    ) -> Option<i32> {
        if consider_descendants && !instance.children().is_empty() {
            return None;
        }

        if instance.class == "LocalizationTable" {
            Some(PRIORITY_SINGLE_READABLE)
        } else {
            None
        }
    }

    fn syncback_new_path(
        &self,
        parent_path: &Path,
        name: &str,
        _instance: &Instance,
    ) -> anyhow::Result<std::path::PathBuf> {
        Ok(parent_path.join(format!("{}.csv", name)))
    }

    fn syncback(&self, sync: &SyncbackContextX<'_, '_>) -> anyhow::Result<SyncbackNode> {
        let vfs = sync.vfs;
        let path = sync.path;
        let old = &sync.old;
        let new = sync.new;
        let metadata = sync.metadata;

        let (new_dom, new_ref) = new;

        let instance = new_dom.get_by_ref(new_ref).unwrap();

        let meta = reconcile_meta_file(
            vfs,
            &path.with_extension("meta.json"),
            instance,
            HashSet::from(["Contents", "ClassName"]),
            Some("LocalizationTable"),
            &metadata.context.syncback.property_filters_save,
        )?;

        Ok(SyncbackNode::new(
            (old.opt_id(), new_ref),
            InstanceSnapshot::from_tree_copy(new_dom, new_ref, false).metadata(
                metadata
                    .clone()
                    .instigating_source(path.to_path_buf())
                    .relevant_paths(vec![path.to_path_buf(), path.with_extension("meta.json")])
                    .middleware_id(self.middleware_id())
                    .fs_snapshot(
                        FsSnapshot::new()
                            .with_file_contents_owned(&path, get_instance_contents(instance)?)
                            .with_file_contents_opt(&path.with_extension("meta.json"), meta),
                    ),
            ),
        ))
    }
}

/// Struct that holds any valid row from a Roblox CSV translation table.
///
/// We manually deserialize into this table from CSV, but let serde_json handle
/// serialization.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalizationEntry<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<&'a str>,

    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<&'a str>,

    #[serde(skip_serializing_if = "Option::is_none")]
    example: Option<&'a str>,

    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<&'a str>,

    // We use a BTreeMap here to get deterministic output order.
    values: BTreeMap<&'a str, &'a str>,
}

/// Normally, we'd be able to let the csv crate construct our struct for us.
///
/// However, because of a limitation with Serde's 'flatten' feature, it's not
/// possible presently to losslessly collect extra string values while using
/// csv+Serde.
///
/// https://github.com/BurntSushi/rust-csv/issues/151
///
/// This function operates in one step in order to minimize data-copying.
fn convert_localization_csv(contents: &[u8]) -> Result<String, csv::Error> {
    let mut reader = csv::Reader::from_reader(contents);

    let headers = reader.headers()?.clone();

    let mut records = Vec::new();

    for record in reader.into_records() {
        records.push(record?);
    }

    let mut entries = Vec::new();

    for record in &records {
        let mut entry = LocalizationEntry::default();

        for (header, value) in headers.iter().zip(record.into_iter()) {
            if header.is_empty() || value.is_empty() {
                continue;
            }

            match header {
                "Key" => entry.key = Some(value),
                "Source" => entry.source = Some(value),
                "Context" => entry.context = Some(value),
                "Example" => entry.example = Some(value),
                _ => {
                    entry.values.insert(header, value);
                }
            }
        }

        if entry.key.is_none() && entry.source.is_none() {
            continue;
        }

        entries.push(entry);
    }

    let encoded =
        serde_json::to_string(&entries).expect("Could not encode JSON for localization table");

    Ok(encoded)
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalizationEntryOwned {
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    example: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,

    // We use a BTreeMap here to get deterministic output order.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    values: BTreeMap<String, String>,
}

fn get_instance_contents(instance: &Instance) -> anyhow::Result<Vec<u8>> {
    let json_contents = get_raw_contents(instance)?;
    read_table_to_csv(json_contents)
}

fn get_raw_contents(instance: &Instance) -> anyhow::Result<&str> {
    Ok(match instance.properties.get("Contents") {
        Some(Variant::String(contents)) => contents.as_str(),
        Some(Variant::BinaryString(contents)) => core::str::from_utf8(&contents.as_ref())?,
        Some(Variant::SharedString(contents)) => core::str::from_utf8(&contents.data())?,
        _ => bail!("LocalizationTable.Contents was not a string or was missing"),
    })
}

fn read_table_to_csv(contents: &str) -> anyhow::Result<Vec<u8>> {
    let mut result: Vec<u8> = Vec::new();
    let mut writer = csv::Writer::from_writer(&mut result);

    let contents: Vec<LocalizationEntryOwned> = serde_json::from_str(contents)?;

    let mut headers = vec!["Key", "Source", "Context", "Example"];

    let mut extra_headers = HashMap::new();
    for entry in &contents {
        for (key, _) in &entry.values {
            if !extra_headers.contains_key(key.as_str()) {
                extra_headers.insert(key.as_str(), headers.len());
                headers.push(key);
            }
        }
    }

    let extra_headers_iter = headers.iter().skip(4).map(|v| *v);

    writer.write_record(&headers)?;
    for entry in &contents {
        let values = [&entry.key, &entry.source, &entry.context, &entry.example]
            .into_iter()
            .map(|v| v.as_ref().map(|v| v.as_str()))
            .chain(
                extra_headers_iter
                    .clone()
                    .map(|key| entry.values.get(key).map(|v| v.as_str())),
            )
            .map(|v| v.unwrap_or(""));
        writer.write_record(values)?;
    }
    writer.flush()?;

    drop(writer); // release borrow so we can return result

    Ok(result)
}

#[cfg(test)]
mod test {
    use super::*;

    use memofs::{InMemoryFs, VfsSnapshot};

    #[test]
    fn csv_from_vfs() {
        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot(
            "/foo.csv",
            VfsSnapshot::file(
                r#"
Key,Source,Context,Example,es
Ack,Ack!,,An exclamation of despair,¡Ay!"#,
            ),
        )
        .unwrap();

        let mut vfs = Vfs::new(imfs);

        let instance_snapshot = CsvMiddleware
            .snapshot(&InstanceContext::default(), &mut vfs, Path::new("/foo.csv"))
            .unwrap()
            .unwrap();

        insta::assert_yaml_snapshot!(instance_snapshot);
    }

    #[test]
    fn csv_with_meta() {
        let mut imfs = InMemoryFs::new();
        imfs.load_snapshot(
            "/foo.csv",
            VfsSnapshot::file(
                r#"
Key,Source,Context,Example,es
Ack,Ack!,,An exclamation of despair,¡Ay!"#,
            ),
        )
        .unwrap();
        imfs.load_snapshot(
            "/foo.meta.json",
            VfsSnapshot::file(r#"{ "ignoreUnknownInstances": true }"#),
        )
        .unwrap();

        let mut vfs = Vfs::new(imfs);

        let instance_snapshot = CsvMiddleware
            .snapshot(&InstanceContext::default(), &mut vfs, Path::new("/foo.csv"))
            .unwrap()
            .unwrap();

        insta::assert_yaml_snapshot!(instance_snapshot);
    }
}
