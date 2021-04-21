// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Handle file layout of PyOxidizer projects.

use {
    crate::environment::{PyOxidizerSource, BUILD_GIT_COMMIT, PYOXIDIZER_VERSION},
    anyhow::{anyhow, Context, Result},
    handlebars::Handlebars,
    once_cell::sync::Lazy,
    python_packaging::filesystem_scanning::walk_tree_files,
    serde::Serialize,
    std::{
        collections::BTreeMap,
        io::Write,
        path::{Path, PathBuf},
        str::FromStr,
    },
};

static HANDLEBARS: Lazy<Handlebars<'static>> = Lazy::new(|| {
    let mut handlebars = Handlebars::new();

    handlebars
        .register_template_string(
            "application-manifest.rc",
            include_str!("templates/application-manifest.rc.hbs"),
        )
        .unwrap();
    handlebars
        .register_template_string(
            "cargo-extra.toml",
            include_str!("templates/cargo-extra.toml.hbs"),
        )
        .unwrap();
    handlebars
        .register_template_string("exe.manifest", include_str!("templates/exe.manifest.hbs"))
        .unwrap();
    handlebars
        .register_template_string("new-build.rs", include_str!("templates/new-build.rs.hbs"))
        .unwrap();
    handlebars
        .register_template_string(
            "new-cargo-config",
            include_str!("templates/new-cargo-config.hbs"),
        )
        .unwrap();
    handlebars
        .register_template_string("new-main.rs", include_str!("templates/new-main.rs.hbs"))
        .unwrap();
    handlebars
        .register_template_string(
            "new-pyoxidizer.bzl",
            include_str!("templates/new-pyoxidizer.bzl.hbs"),
        )
        .unwrap();

    handlebars
});

/// Contents of Cargo.lock file for the new Rust projects.
const NEW_PROJECT_CARGO_LOCK: &str = include_str!("new-project-cargo.lock");

/// Package dependencies of new Rust projects to be recorded in the Cargo.lock.
const NEW_PROJECT_DEPENDENCIES: &[&str] = &[
    "embed-resource",
    "jemallocator",
    "mimalloc",
    "pyembed",
    "snmalloc-rs",
];

#[derive(Serialize)]
struct PythonDistribution {
    build_target: String,
    url: String,
    sha256: String,
}

#[derive(Serialize)]
struct TemplateData {
    pyoxidizer_version: Option<String>,
    pyoxidizer_commit: Option<String>,
    pyoxidizer_local_repo_path: Option<String>,
    pyoxidizer_git_url: Option<String>,
    pyoxidizer_git_commit: Option<String>,
    pyoxidizer_git_tag: Option<String>,

    python_distributions: Vec<PythonDistribution>,
    program_name: Option<String>,
    code: Option<String>,
    pip_install_simple: Vec<String>,
}

impl TemplateData {
    fn new() -> TemplateData {
        TemplateData {
            pyoxidizer_version: None,
            pyoxidizer_commit: None,
            pyoxidizer_local_repo_path: None,
            pyoxidizer_git_url: None,
            pyoxidizer_git_commit: None,
            pyoxidizer_git_tag: None,
            python_distributions: Vec::new(),
            program_name: None,
            code: None,
            pip_install_simple: Vec::new(),
        }
    }
}

fn populate_template_data(source: &PyOxidizerSource, data: &mut TemplateData) {
    data.pyoxidizer_version = Some(PYOXIDIZER_VERSION.to_string());
    data.pyoxidizer_commit = Some(
        BUILD_GIT_COMMIT
            .clone()
            .unwrap_or_else(|| "UNKNOWN".to_string()),
    );

    match source {
        PyOxidizerSource::LocalPath { path } => {
            data.pyoxidizer_local_repo_path = Some(path.display().to_string());
        }
        PyOxidizerSource::GitUrl { url, commit, tag } => {
            data.pyoxidizer_git_url = Some(url.clone());

            if let Some(commit) = commit {
                data.pyoxidizer_git_commit = Some(commit.clone());
            }
            if let Some(tag) = tag {
                data.pyoxidizer_git_tag = Some(tag.clone());
            }
        }
    }
}

/// Find existing PyOxidizer files in a project directory.
pub fn find_pyoxidizer_files(root: &Path) -> Vec<PathBuf> {
    let mut res: Vec<PathBuf> = Vec::new();

    for f in walk_tree_files(&root) {
        let path = f.path().strip_prefix(root).expect("unable to strip prefix");
        let path_s = path.to_str().expect("unable to convert path to str");

        if path_s.contains("pyoxidizer") || path_s.contains("pyembed") {
            res.push(path.to_path_buf());
        }
    }

    res
}

/// Write a new .cargo/config file for a project path.
pub fn write_new_cargo_config(project_path: &Path) -> Result<()> {
    let cargo_path = project_path.join(".cargo");

    if !cargo_path.is_dir() {
        std::fs::create_dir(&cargo_path)?;
    }

    let data: BTreeMap<String, String> = BTreeMap::new();
    let t = HANDLEBARS.render("new-cargo-config", &data)?;

    let config_path = cargo_path.join("config");
    println!("writing {}", config_path.display());
    std::fs::write(&config_path, t)?;

    Ok(())
}

/// Write a Cargo.lock file for a project path.
///
/// The Cargo.lock content is under version control and is automatically
/// updated as part of the release automation. The file is generated in
/// `--offline` mode and the contents of this `Cargo.lock` should closely
/// resemble those in this repository's `Cargo.lock`. This helps ensure that
/// the crate versions used by generated Rust projects match those of the
/// build/commit of PyOxidizer used to generate the project.
pub fn write_new_cargo_lock(project_path: &Path, project_name: &str) -> Result<()> {
    // Add this project's entry to the lock file contents, otherwise the
    // lock file will need updating on first use.
    let mut lock_file = cargo_lock::Lockfile::from_str(NEW_PROJECT_CARGO_LOCK)?;

    let dependencies = NEW_PROJECT_DEPENDENCIES
        .iter()
        .map(|dep| cargo_lock::Dependency {
            name: cargo_lock::Name::from_str(dep)
                .expect("could not convert dependency name to Name"),
            version: lock_file
                .packages
                .iter()
                .filter_map(|package| {
                    if package.name.as_str() == *dep {
                        Some(package.version.clone())
                    } else {
                        None
                    }
                })
                .next()
                .expect("unable to find dependency in frozen Cargo lock; something is out of sync"),
            source: None,
        })
        .collect::<Vec<_>>();

    lock_file.packages.push(cargo_lock::Package {
        name: cargo_lock::Name::from_str(project_name)?,
        version: semver::Version::new(0, 1, 0),
        source: None,
        checksum: None,
        dependencies,
        replace: None,
    });

    // cargo_lock isn't smart enough to sort the packages. So do that here.
    lock_file
        .packages
        .sort_by(|a, b| a.name.as_str().cmp(&b.name.as_str()));

    let lock_path = project_path.join("Cargo.lock");
    println!("writing {}", lock_path.display());
    std::fs::write(&lock_path, &lock_file.to_string())?;

    Ok(())
}

pub fn write_new_build_rs(path: &Path, program_name: &str) -> Result<()> {
    let mut data = TemplateData::new();
    data.program_name = Some(program_name.to_string());
    let t = HANDLEBARS.render("new-build.rs", &data)?;

    println!("writing {}", path.display());
    std::fs::write(path, t)?;

    Ok(())
}

/// Write a new main.rs file that runs the embedded Python interpreter.
///
/// `windows_subsystem` is the value of the `windows_subsystem` Rust attribute.
pub fn write_new_main_rs(path: &Path, windows_subsystem: &str) -> Result<()> {
    let mut data: BTreeMap<String, String> = BTreeMap::new();
    data.insert(
        "windows_subsystem".to_string(),
        windows_subsystem.to_string(),
    );
    let t = HANDLEBARS.render("new-main.rs", &data)?;

    println!("writing {}", path.to_str().unwrap());
    let mut fh = std::fs::File::create(path)?;
    fh.write_all(t.as_bytes())?;

    Ok(())
}

/// Writes default PyOxidizer config files into a project directory.
pub fn write_new_pyoxidizer_config_file(
    source: &PyOxidizerSource,
    project_dir: &Path,
    name: &str,
    code: Option<&str>,
    pip_install: &[&str],
) -> Result<()> {
    let path = project_dir.join("pyoxidizer.bzl");

    let mut data = TemplateData::new();
    populate_template_data(source, &mut data);
    data.program_name = Some(name.to_string());

    if let Some(code) = code {
        // Replace " with \" to work around
        // https://github.com/google/starlark-rust/issues/230.
        data.code = Some(code.replace("\"", "\\\""));
    }

    data.pip_install_simple = pip_install.iter().map(|v| (*v).to_string()).collect();

    let t = HANDLEBARS.render("new-pyoxidizer.bzl", &data)?;

    println!("writing {}", path.to_str().unwrap());
    let mut fh = std::fs::File::create(path)?;
    fh.write_all(t.as_bytes())?;

    Ok(())
}

/// Write an application manifest and corresponding resource file.
///
/// This is used on Windows to allow the built executable to use long paths.
///
/// Windows 10 version 1607 and above enable long paths by default. So we
/// might be able to remove this someday. It isn't clear if you get long
/// paths support if using that version of the Windows SDK or if you have
/// to be running on a modern Windows version as well.
pub fn write_application_manifest(project_dir: &Path, program_name: &str) -> Result<()> {
    let mut data = TemplateData::new();
    data.program_name = Some(program_name.to_string());

    let manifest_path = project_dir.join(format!("{}.exe.manifest", program_name));
    let manifest_data = HANDLEBARS.render("exe.manifest", &data)?;
    println!("writing {}", manifest_path.display());
    let mut fh = std::fs::File::create(&manifest_path)?;
    fh.write_all(manifest_data.as_bytes())?;

    let rc_path = project_dir.join(format!("{}-manifest.rc", program_name));
    let rc_data = HANDLEBARS.render("application-manifest.rc", &data)?;
    println!("writing {}", rc_path.display());
    let mut fh = std::fs::File::create(&rc_path)?;
    fh.write_all(rc_data.as_bytes())?;

    Ok(())
}

/// Add PyOxidizer to an existing Rust project on the filesystem.
///
/// The target directory must not already have PyOxidizer files. This
/// will be verified during execution.
///
/// When called, various Rust source files required to embed Python
/// are created at the target directory. Instructions for finalizing the
/// configuration are also printed to stdout.
///
/// The Rust source files added to the target project are installed into
/// a sub-directory defined by ``module_name``.
pub fn add_pyoxidizer(project_dir: &Path, _suppress_help: bool) -> Result<()> {
    let existing_files = find_pyoxidizer_files(&project_dir);

    if !existing_files.is_empty() {
        return Err(anyhow!("existing PyOxidizer files found; cannot add"));
    }

    let cargo_toml = project_dir.to_path_buf().join("Cargo.toml");

    if !cargo_toml.exists() {
        return Err(anyhow!("Cargo.toml does not exist at destination"));
    }

    let cargo_toml_data = std::fs::read(cargo_toml)?;
    let manifest = cargo_toml::Manifest::from_slice(&cargo_toml_data)?;

    let _package = match &manifest.package {
        Some(package) => package,
        None => panic!("no [package]; that's weird"),
    };

    // TODO look for pyembed dependency and print message about adding it.

    Ok(())
}

/// How to define the ``pyembed`` crate dependency.
pub enum PyembedLocation {
    /// Use a specific version, installed from the crate registry.
    ///
    /// (This is how most Rust dependencies are defined.)
    Version(String),

    /// Use a local filesystem path.
    Path(PathBuf),

    /// A git repository URL and revision hash.
    Git(String, String),
}

impl PyembedLocation {
    /// Convert the location to a string holding Cargo manifest location info.
    pub fn cargo_manifest_fields(&self) -> String {
        match self {
            Self::Version(version) => format!("version = \"{}\"", version),
            Self::Path(path) => format!("path = \"{}\"", path.display()),
            Self::Git(url, commit) => format!("git = \"{}\", rev = \"{}\"", url, commit),
        }
    }
}

/// Update the Cargo.toml of a new Rust project to use pyembed.
pub fn update_new_cargo_toml(path: &Path, pyembed_location: &PyembedLocation) -> Result<()> {
    let content = std::fs::read_to_string(path)?;

    // Insert a `build = build.rs` line after the `version = *\n` line. We key off
    // version because it should always be present.
    let version_start = match content.find("version =") {
        Some(off) => off,
        None => return Err(anyhow!("could not find version line in Cargo.toml")),
    };

    let nl_off = match &content[version_start..content.len()].find('\n') {
        Some(off) => version_start + off + 1,
        None => return Err(anyhow!("could not find newline after version line")),
    };

    let (before, after) = content.split_at(nl_off);

    let mut content = before.to_string();
    content.push_str("build = \"build.rs\"\n");
    content.push_str(after);

    content.push_str(&format!(
        "pyembed = {{ {}, default-features = false }}\n",
        pyembed_location.cargo_manifest_fields()
    ));
    content.push('\n');

    let data = TemplateData::new();
    content.push_str(
        &HANDLEBARS
            .render("cargo-extra.toml", &data)
            .context("rendering cargo-extra.toml template")?,
    );

    std::fs::write(path, content)?;

    Ok(())
}

/// Initialize a new Rust project using PyOxidizer.
///
/// The created binary application will have the name of the final
/// path component.
///
/// `windows_subsystem` is the value of the `windows_subsystem` compiler
/// attribute.
pub fn initialize_project(
    source: &PyOxidizerSource,
    project_path: &Path,
    cargo_exe: &Path,
    code: Option<&str>,
    pip_install: &[&str],
    windows_subsystem: &str,
) -> Result<()> {
    let status = std::process::Command::new(cargo_exe)
        .arg("init")
        .arg("--bin")
        .arg(project_path)
        .status()
        .context("invoking cargo init")?;

    if !status.success() {
        return Err(anyhow!("cargo init failed"));
    }

    let path = PathBuf::from(project_path);
    let name = path.iter().last().unwrap().to_str().unwrap();
    add_pyoxidizer(&path, true).context("adding PyOxidizer to Rust project")?;
    update_new_cargo_toml(&path.join("Cargo.toml"), &source.as_pyembed_location())
        .context("updating Cargo.toml")?;
    write_new_cargo_config(&path).context("writing cargo config")?;
    write_new_cargo_lock(&path, name).context("writing Cargo.lock")?;
    write_new_build_rs(&path.join("build.rs"), name).context("writing build.rs")?;
    write_new_main_rs(&path.join("src").join("main.rs"), windows_subsystem)
        .context("writing main.rs")?;
    write_new_pyoxidizer_config_file(source, &path, &name, code, pip_install)
        .context("writing PyOxidizer config file")?;
    write_application_manifest(&path, &name).context("writing application manifest")?;

    Ok(())
}
