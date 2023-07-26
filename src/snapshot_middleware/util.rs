use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
};

use anyhow::Context;
use memofs::Vfs;
use rbx_dom_weak::Instance;

use crate::snapshot::PropertyFilter;

use super::MetadataFile;

pub fn try_remove_file(vfs: &Vfs, path: &Path) -> anyhow::Result<()> {
    vfs.remove_file(path).or_else(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(e),
    })?;
    Ok(())
}

pub fn reconcile_meta_file_empty(vfs: &Vfs, path: &Path) -> anyhow::Result<()> {
    let existing = {
        let contents = vfs.read(path).map(Some).or_else(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Ok(None),
            _ => Err(e),
        })?;
        if let Some(contents) = contents {
            Some(MetadataFile::from_slice(&contents, path.to_path_buf())?)
        } else {
            None
        }
    };

    let mut new_file = if let Some(existing) = existing {
        existing.clone()
    } else {
        MetadataFile {
            ignore_unknown_instances: None,
            properties: HashMap::new(),
            attributes: HashMap::new(),
            class_name: None,
            path: path.to_path_buf(),
        }
    };

    new_file.properties.clear();
    new_file.attributes.clear();
    new_file.class_name = None;

    if new_file.is_empty() {
        try_remove_file(vfs, path)?;
    } else {
        vfs.write(path, serde_json::to_string_pretty(&new_file)?)?;
    }

    Ok(())
}

pub fn reconcile_meta_file(
    vfs: &Vfs,
    path: &Path,
    instance: &Instance,
    skip_props: HashSet<&str>,
    base_class: Option<&str>,
    filters: &BTreeMap<String, PropertyFilter>,
) -> anyhow::Result<()> {
    let existing = {
        let contents = vfs.read(path).map(Some).or_else(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Ok(None),
            _ => Err(e),
        })?;
        if let Some(contents) = contents {
            Some(MetadataFile::from_slice(&contents, path.to_path_buf())?)
        } else {
            None
        }
    };

    let mut new_file = if let Some(existing) = &existing {
        existing.clone()
    } else {
        MetadataFile {
            ignore_unknown_instances: None,
            properties: HashMap::new(),
            attributes: HashMap::new(),
            class_name: None,
            path: path.to_path_buf(),
        }
    };

    new_file = new_file.with_instance_props(instance, skip_props, filters);

    if Some(instance.class.as_str()) == base_class {
        new_file.class_name = None;
    } else {
        new_file.class_name = Some(instance.class.clone());
    }

    if let Some(existing) = &existing {
        new_file.minimize_diff(existing, base_class);
    }

    if new_file.is_empty() {
        try_remove_file(vfs, path)?;
    } else {
        vfs.write(path, serde_json::to_string_pretty(&new_file)?)?;
    }

    Ok(())
}

/// If the given string ends up with the given suffix, returns the portion of
/// the string before the suffix.
pub fn match_trailing<'a>(input: &'a str, suffix: &str) -> Option<&'a str> {
    if input.ends_with(suffix) {
        let end = input.len().saturating_sub(suffix.len());
        Some(&input[..end])
    } else {
        None
    }
}

pub trait PathExt {
    fn file_name_ends_with(&self, suffix: &str) -> bool;
    fn file_name_trim_end<'a>(&'a self, suffix: &str) -> anyhow::Result<&'a str>;
    fn file_name_trim_extension<'a>(&'a self) -> anyhow::Result<String>;
}

impl<P> PathExt for P
where
    P: AsRef<Path>,
{
    fn file_name_ends_with(&self, suffix: &str) -> bool {
        self.as_ref()
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.ends_with(suffix))
            .unwrap_or(false)
    }

    fn file_name_trim_end<'a>(&'a self, suffix: &str) -> anyhow::Result<&'a str> {
        let path = self.as_ref();
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .with_context(|| format!("Path did not have a file name: {}", path.display()))?;

        match_trailing(&file_name, suffix)
            .with_context(|| format!("Path did not end in {}: {}", suffix, path.display()))
    }

    fn file_name_trim_extension(&self) -> anyhow::Result<String> {
        self.as_ref()
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(|string| string.to_owned())
            .with_context(|| format!("Path did not have a file name: {}", self.as_ref().display()))
    }
}
