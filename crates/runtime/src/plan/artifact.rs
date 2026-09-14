//! Immutable blobs under `<plan dir>/artifacts/<hex>`, written once and read by digest.

use std::io::Write;
use std::path::{Path, PathBuf};

use yi_types::plan::canonical::{ArtifactRef, Digest};

pub const ARTIFACTS_DIR: &str = "artifacts";

#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("no artifact {digest} under {dir}")]
    Missing { digest: Digest, dir: PathBuf },
    #[error("artifact {path} does not hash to its name ({found})")]
    Damaged { path: PathBuf, found: Digest },
}

fn io_at(path: &Path) -> impl FnOnce(std::io::Error) -> ArtifactError + '_ {
    move |source| ArtifactError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[derive(Debug, Clone)]
pub struct Artifacts {
    dir: PathBuf,
}

impl Artifacts {
    pub fn under(plan_dir: &Path) -> Self {
        Self {
            dir: plan_dir.join(ARTIFACTS_DIR),
        }
    }

    pub fn path(&self, digest: &Digest) -> PathBuf {
        self.dir.join(digest.hex())
    }

    pub fn put(
        &self,
        bytes: &[u8],
        media_type: &str,
        nonce: &str,
    ) -> Result<ArtifactRef, ArtifactError> {
        let digest = Digest::of(bytes);
        let reference = ArtifactRef {
            digest,
            media_type: media_type.to_owned(),
            length: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        };
        let target = self.path(&digest);
        if target.is_file() {
            self.get(&digest)?;
            return Ok(reference);
        }
        std::fs::create_dir_all(&self.dir).map_err(io_at(&self.dir))?;
        let tmp = self.dir.join(format!(".{}.{nonce}.tmp", digest.hex()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(io_at(&tmp))?;
        let written = file
            .write_all(bytes)
            .and_then(|()| file.sync_all())
            .and_then(|()| std::fs::rename(&tmp, &target))
            .and_then(|()| std::fs::File::open(&self.dir)?.sync_all());
        if let Err(source) = written {
            let _removed_best_effort = std::fs::remove_file(&tmp);
            return Err(ArtifactError::Io {
                path: target,
                source,
            });
        }
        Ok(reference)
    }

    pub fn get(&self, digest: &Digest) -> Result<Vec<u8>, ArtifactError> {
        let path = self.path(digest);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Err(ArtifactError::Missing {
                    digest: *digest,
                    dir: self.dir.clone(),
                });
            }
            Err(source) => return Err(ArtifactError::Io { path, source }),
        };
        let found = Digest::of(&bytes);
        if &found != digest {
            return Err(ArtifactError::Damaged { path, found });
        }
        Ok(bytes)
    }
}
