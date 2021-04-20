// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use {
    anyhow::{anyhow, Context, Result},
    std::{
        io::Read,
        path::{Path, PathBuf},
    },
    tugger_file_manifest::{FileEntry, FileManifest},
};

#[derive(Clone, Copy, Debug)]
pub enum CompressionFormat {
    Gzip,
    Xz,
    Zstd,
}

fn get_decompression_stream(format: CompressionFormat, data: Vec<u8>) -> Result<Box<dyn Read>> {
    let reader = std::io::Cursor::new(data);

    match format {
        CompressionFormat::Zstd => Ok(Box::new(zstd::stream::read::Decoder::new(reader)?)),
        CompressionFormat::Xz => Ok(Box::new(xz2::read::XzDecoder::new(reader))),
        CompressionFormat::Gzip => Ok(Box::new(flate2::read::GzDecoder::new(reader))),
    }
}

/// Represents an extracted Rust package archive.
///
/// File contents exist in memory.
pub struct PackageArchive {
    manifest: FileManifest,
    components: Vec<String>,
}

impl PackageArchive {
    /// Construct a new instance with compressed tar data.
    pub fn new(format: CompressionFormat, data: Vec<u8>) -> Result<Self> {
        let mut archive = tar::Archive::new(
            get_decompression_stream(format, data).context("obtaining decompression stream")?,
        );

        let mut manifest = FileManifest::default();

        for entry in archive.entries().context("obtaining tar archive entries")? {
            let mut entry = entry.context("resolving tar archive entry")?;

            let path = entry.path().context("resolving entry path")?;

            let first_component = path
                .components()
                .next()
                .ok_or_else(|| anyhow!("unable to get first path component"))?;

            let path = path
                .strip_prefix(first_component)
                .context("stripping path prefix")?
                .to_path_buf();

            let mut entry_data = Vec::new();
            entry.read_to_end(&mut entry_data)?;

            manifest.add_file_entry(
                path,
                FileEntry {
                    data: entry_data.into(),
                    executable: entry.header().mode()? & 0o111 != 0,
                },
            )?;
        }

        if manifest
            .get("rust-installer-version")
            .ok_or_else(|| anyhow!("archive does not contain rust-installer-version"))?
            .data
            .resolve()?
            != b"3\n"
        {
            return Err(anyhow!("rust-installer-version has unsupported version"));
        }

        let components = manifest
            .get("components")
            .ok_or_else(|| anyhow!("archive does not contain components file"))?
            .data
            .resolve()?;
        let components =
            String::from_utf8(components).context("converting components file to string")?;
        let components = components
            .lines()
            .map(|l| l.to_string())
            .collect::<Vec<_>>();

        Ok(Self {
            manifest,
            components,
        })
    }

    /// Materialize files from this manifest into the specified destination directory.
    pub fn install(&self, dest_dir: &Path) -> Result<()> {
        for component in &self.components {
            let component_path = PathBuf::from(component);
            let manifest_path = component_path.join("manifest.in");

            let manifest = self
                .manifest
                .get(&manifest_path)
                .ok_or_else(|| anyhow!("{} not found", manifest_path.display()))?;

            let (dirs, files) = Self::parse_manifest(manifest.data.resolve()?)?;

            if !dirs.is_empty() {
                return Err(anyhow!("support for copying directories not implemented"));
            }

            for file in files {
                let manifest_path = component_path.join(&file);
                let entry = self.manifest.get(&manifest_path).ok_or_else(|| {
                    anyhow!(
                        "could not locate file {} in manifest",
                        manifest_path.display()
                    )
                })?;

                let dest_path = dest_dir.join(file);

                entry.write_to_path(&dest_path).with_context(|| {
                    format!(
                        "writing {} to {}",
                        manifest_path.display(),
                        dest_path.display(),
                    )
                })?;
            }
        }

        Ok(())
    }

    fn parse_manifest(data: Vec<u8>) -> Result<(Vec<String>, Vec<String>)> {
        let mut files = vec![];
        let mut dirs = vec![];

        let data = String::from_utf8(data)?;

        for line in data.lines() {
            if let Some(pos) = line.find(':') {
                let action = &line[0..pos];
                let path = &line[pos + 1..];

                match action {
                    "file" => {
                        files.push(path.to_string());
                    }
                    "dir" => {
                        dirs.push(path.to_string());
                    }
                    _ => return Err(anyhow!("unhandled action in manifest.in: {}", action)),
                }
            }
        }

        Ok((dirs, files))
    }
}
