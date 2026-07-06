use anyhow::{Context, Result, bail};
use s3::Region;
use s3::bucket::Bucket;
use s3::creds::Credentials;
use std::path::Path;

use super::Store;

pub struct S3Store {
    bucket: Bucket,
    prefix: String,
}

impl S3Store {
    pub fn new(
        bucket_name: String,
        prefix: String,
        region: String,
        endpoint: Option<String>,
        access_key: Option<String>,
        secret_key: Option<String>,
    ) -> Result<Self> {
        let region = if let Some(ep) = endpoint {
            Region::Custom {
                region: region.into(),
                endpoint: ep,
            }
        } else {
            region
                .parse()
                .context(format!("invalid S3 region: {region}"))?
        };

        let credentials = if let (Some(ak), Some(sk)) = (access_key, secret_key) {
            Credentials::new(Some(&ak), Some(&sk), None, None, None)
                .context("invalid S3 credentials")?
        } else {
            Credentials::from_env()
                .or_else(|_| Credentials::default())
                .context("could not load S3 credentials from environment or defaults")?
        };

        let bucket = Bucket::new(&bucket_name, region, credentials)
            .context("invalid S3 bucket configuration")?
            .with_path_style();

        Ok(Self {
            bucket: *bucket,
            prefix: prefix.trim_end_matches('/').to_string(),
        })
    }

    fn key(&self, rel_path: &str) -> String {
        if self.prefix.is_empty() {
            rel_path.to_string()
        } else {
            format!("{}/{rel_path}", self.prefix)
        }
    }
}

impl Store for S3Store {
    fn write_file(&self, rel_path: &str, data: &[u8]) -> Result<()> {
        let key = self.key(rel_path);
        let response = self.bucket.put_object(&key, data)?;
        if response.status_code() >= 300 {
            bail!("S3 put failed for {key}: status {}", response.status_code());
        }
        Ok(())
    }

    fn read_file(&self, rel_path: &str) -> Result<Vec<u8>> {
        let key = self.key(rel_path);
        let response = self.bucket.get_object(&key)?;
        if response.status_code() == 404 {
            bail!("S3 object not found: {key}");
        }
        if response.status_code() >= 300 {
            bail!("S3 get failed for {key}: status {}", response.status_code());
        }
        Ok(response.to_vec())
    }

    fn exists(&self, rel_path: &str) -> bool {
        let key = self.key(rel_path);
        match self.bucket.head_object(&key) {
            Ok((_, code)) => code < 300,
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("404") || msg.contains("NoSuchKey") || msg.contains("Not Found") {
                    false
                } else {
                    eprintln!("warning: S3 head_object failed for {key}: {e}");
                    false
                }
            }
        }
    }

    fn list_files(&self, prefix: &str) -> Result<Vec<String>> {
        let full_prefix = self.key(prefix);
        let results = self.bucket.list(format!("{full_prefix}/"), None)?;
        let mut files = Vec::new();
        for result in results {
            for obj in result.contents {
                let rel = if self.prefix.is_empty() {
                    obj.key
                } else {
                    obj.key
                        .strip_prefix(&format!("{}/", self.prefix))
                        .unwrap_or(&obj.key)
                        .to_string()
                };
                files.push(rel);
            }
        }
        Ok(files)
    }

    fn file_size(&self, rel_path: &str) -> Result<u64> {
        let key = self.key(rel_path);
        let (head, code) = self.bucket.head_object(&key)?;
        if code >= 300 {
            bail!("S3 head failed for {key}: status {code}");
        }
        let len = head.content_length.unwrap_or(0);
        Ok(u64::try_from(len).unwrap_or(0))
    }

    fn atomic_write(&self, rel_path: &str, data: &[u8]) -> Result<()> {
        self.write_file(rel_path, data)
    }

    fn sync_down(&self) -> Result<()> {
        Ok(())
    }

    fn sync_up(&self, _message: &str) -> Result<()> {
        Ok(())
    }

    // S3 has no native locking mechanism. Concurrent writes from multiple
    // machines can corrupt the manifest. Single-user or externally
    // coordinated access is assumed.
    fn lock(&self) -> Result<Box<dyn std::any::Any>> {
        Ok(Box::new(()))
    }

    fn try_lock(&self) -> Result<Box<dyn std::any::Any>> {
        Ok(Box::new(()))
    }

    fn local_path(&self) -> Option<&Path> {
        None
    }
}
