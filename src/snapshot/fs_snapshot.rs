use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Result;
use memofs::{IoResultExt, Vfs};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FsSnapshot {
    pub files: BTreeMap<PathBuf, Option<Arc<Vec<u8>>>>,
    pub dirs: BTreeSet<PathBuf>,
}

impl FsSnapshot {
    pub fn new() -> Self {
        Self {
            files: BTreeMap::new(),
            dirs: BTreeSet::new(),
        }
    }

    pub fn with_file_contents_arc<P: AsRef<Path>>(mut self, path: P, data: &Arc<Vec<u8>>) -> Self {
        self.files
            .insert(path.as_ref().to_path_buf(), Some(data.clone()));
        self
    }

    pub fn with_file_contents_owned<P: AsRef<Path>, D: Into<Vec<u8>>>(
        mut self,
        path: P,
        data: D,
    ) -> Self {
        self.files
            .insert(path.as_ref().to_path_buf(), Some(Arc::new(data.into())));
        self
    }

    pub fn with_file_contents_borrowed<P: AsRef<Path>, D: AsRef<[u8]>>(
        mut self,
        path: P,
        data: D,
    ) -> Self {
        self.files.insert(
            path.as_ref().to_path_buf(),
            Some(Arc::new(data.as_ref().to_vec())),
        );
        self
    }

    pub fn with_file_contents_opt<P: AsRef<Path>, D: Into<Vec<u8>>>(
        mut self,
        path: P,
        data: Option<D>,
    ) -> Self {
        if let Some(data) = data {
            self.files
                .insert(path.as_ref().to_path_buf(), Some(Arc::new(data.into())));
        }
        self
    }

    pub fn with_file<P: AsRef<Path>>(mut self, path: P) -> Self {
        self.files.insert(path.as_ref().to_path_buf(), None);
        self
    }

    pub fn with_files<P: AsRef<Path>>(mut self, paths: &[P]) -> Self {
        for path in paths {
            self.files.insert(path.as_ref().to_path_buf(), None);
        }
        self
    }

    pub fn with_dir<P: AsRef<Path>>(mut self, path: P) -> Self {
        self.dirs.insert(path.as_ref().to_path_buf());
        self
    }

    pub fn with_dirs<P: AsRef<Path>>(mut self, paths: &[P]) -> Self {
        for path in paths {
            self.dirs.insert(path.as_ref().to_path_buf());
        }
        self
    }

    pub fn lose_data(&self) -> Self {
        Self {
            files: self
                .files
                .iter()
                .map(|(path, _)| (path.clone(), None))
                .collect(),
            dirs: self.dirs.clone(),
        }
    }

    pub fn merge_with(&self, other: &Self) -> Self {
        Self {
            files: self
                .files
                .iter()
                .map(|(path, data)| (path.clone(), data.clone()))
                .chain(
                    other
                        .files
                        .iter()
                        .map(|(path, data)| (path.clone(), data.clone())),
                )
                .collect(),
            dirs: self.dirs.union(&other.dirs).cloned().collect(),
        }
    }

    pub fn reconcile(
        vfs: &Vfs,
        old_snapshot: Option<&FsSnapshot>,
        new_snapshot: Option<&FsSnapshot>,
    ) -> Result<()> {
        if let (Some(old_snapshot), Some(new_snapshot)) = (old_snapshot, new_snapshot) {
            for (old_path, _) in old_snapshot.files.iter() {
                if !new_snapshot.files.contains_key(old_path) {
                    vfs.remove_file(old_path).with_not_found()?;
                }
            }
            for old_path in old_snapshot.dirs.iter() {
                if !new_snapshot.dirs.contains(old_path) {
                    vfs.remove_dir_all(old_path).with_not_found()?;
                }
            }
            for path in new_snapshot.dirs.iter() {
                vfs.write_dir(path)?;
            }
            for (path, data) in new_snapshot.files.iter() {
                if let Some(data) = data {
                    vfs.write(path, data.as_slice())?;
                }
            }
        } else if let Some(new_snapshot) = new_snapshot {
            for path in new_snapshot.dirs.iter() {
                vfs.write_dir(path)?;
            }
            for (path, data) in new_snapshot.files.iter() {
                if let Some(data) = data {
                    vfs.write(path, data.as_slice())?;
                }
            }
        } else if let Some(old_snapshot) = old_snapshot {
            for path in old_snapshot.dirs.iter() {
                vfs.remove_dir_all(path).with_not_found()?;
            }
            for (path, _) in old_snapshot.files.iter() {
                vfs.remove_file(path).with_not_found()?;
            }
        }

        Ok(())
    }
}
