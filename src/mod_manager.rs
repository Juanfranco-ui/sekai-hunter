use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const MANIFEST_FILE: &str = "mods.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedMod {
    pub id: String,
    pub name: String,
    pub archive_name: String,
    pub enabled: bool,
    pub skipped_conflicts: usize,
    pub files: Vec<ManagedFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedFile {
    pub relative_path: String,
    #[serde(default)]
    pub has_backup: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default)]
    mods: Vec<ManagedMod>,
}

pub struct ModManager {
    data_dir: PathBuf,
    manifest: Manifest,
}

pub struct ModPreview {
    pub name: String,
    pub archive_name: String,
    pub files: Vec<PreviewFile>,
    pub conflicts: Vec<String>,
    pub ignored_entries: usize,
    staging_dir: PathBuf,
}

pub struct PreviewFile {
    pub relative_path: String,
    staged_path: PathBuf,
}

impl Drop for ModPreview {
    fn drop(&mut self) {
        let staging_dir = self.staging_dir.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("sekai-preview-cleanup".into())
            .spawn(move || {
                if let Err(error) = fs::remove_dir_all(&staging_dir) {
                    if error.kind() != io::ErrorKind::NotFound {
                        log::warn!(
                            "Could not remove temporary mod archive directory {}: {error}",
                            staging_dir.display()
                        );
                    }
                }
            })
        {
            log::warn!("Could not start temporary mod cleanup: {error}");
        }
    }
}

impl ModManager {
    pub fn load() -> Result<Self, String> {
        let data_dir = dirs::data_dir()
            .ok_or_else(|| "Could not determine the XDG data directory.".to_owned())?
            .join("sekai-hunter");
        Self::load_from(data_dir)
    }

    fn load_from(data_dir: PathBuf) -> Result<Self, String> {
        let manifest_path = data_dir.join(MANIFEST_FILE);
        let manifest: Manifest = match fs::read_to_string(&manifest_path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|error| format!("Could not read mod registry: {error}"))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Manifest::default(),
            Err(error) => {
                return Err(format!(
                    "Could not read mod registry {}: {error}",
                    manifest_path.display()
                ));
            }
        };
        let mut ids = HashSet::new();
        let mut paths = HashSet::new();
        for managed in &manifest.mods {
            if managed.id.is_empty()
                || !managed
                    .id
                    .chars()
                    .all(|character| character.is_ascii_digit() || character == '-')
                || !ids.insert(managed.id.as_str())
            {
                return Err("The mod registry contains an invalid or duplicate mod ID.".into());
            }
            for file in &managed.files {
                safe_relative_path(&file.relative_path)?;
                if !paths.insert(file.relative_path.as_str()) {
                    return Err(format!(
                        "The mod registry assigns {} to more than one mod.",
                        file.relative_path
                    ));
                }
            }
        }
        Ok(Self { data_dir, manifest })
    }

    pub fn mods(&self) -> &[ManagedMod] {
        &self.manifest.mods
    }

    pub fn preview_archive(path: &Path, game_dir: &Path) -> Result<ModPreview, String> {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if extension != "zip" && extension != "rar" {
            return Err("Choose a ZIP or RAR mod archive.".into());
        }
        if !game_dir.is_dir() {
            return Err("Select a valid Monster Hunter: World directory first.".into());
        }

        let staging_dir = make_staging_dir()?;
        let extraction = if extension == "zip" {
            extract_zip(path, &staging_dir)
        } else {
            extract_rar(path, &staging_dir)
        };
        let ignored_during_extract = match extraction {
            Ok(ignored) => ignored,
            Err(error) => {
                let _ = fs::remove_dir_all(&staging_dir);
                return Err(error);
            }
        };

        let mut staged_files = Vec::new();
        let mut ignored = ignored_during_extract;
        if let Err(error) =
            collect_files(&staging_dir, &staging_dir, &mut staged_files, &mut ignored)
        {
            let _ = fs::remove_dir_all(&staging_dir);
            return Err(format!("Could not inspect extracted archive: {error}"));
        }

        let (files, ignored_layout) = match map_game_files(staged_files) {
            Ok(mapped) => mapped,
            Err(error) => {
                let _ = fs::remove_dir_all(&staging_dir);
                return Err(error);
            }
        };
        ignored += ignored_layout;
        if files.is_empty() {
            let _ = fs::remove_dir_all(&staging_dir);
            return Err(
                "No installable MHW files were found. Expected a nativePC/ or AsteraLoader/ folder, optionally inside one archive folder.".into(),
            );
        }

        let mut conflicts = Vec::new();
        for file in &files {
            match destination_exists(game_dir, &file.relative_path) {
                Ok(true) => conflicts.push(file.relative_path.clone()),
                Ok(false) => {}
                Err(error) => {
                    let _ = fs::remove_dir_all(&staging_dir);
                    return Err(error);
                }
            }
        }
        let archive_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("mod archive")
            .to_owned();
        let name = Path::new(&archive_name)
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or(&archive_name)
            .to_owned();

        Ok(ModPreview {
            name,
            archive_name,
            files,
            conflicts,
            ignored_entries: ignored,
            staging_dir,
        })
    }

    pub fn install(
        &mut self,
        preview: &ModPreview,
        game_dir: &Path,
        overwrite_conflicts: bool,
    ) -> Result<(usize, usize), String> {
        let owned_paths: HashSet<&str> = self
            .manifest
            .mods
            .iter()
            .flat_map(|managed| managed.files.iter())
            .map(|file| file.relative_path.as_str())
            .collect();
        let mut selected = Vec::new();
        let mut skipped = 0;
        for file in &preview.files {
            let exists = destination_exists(game_dir, &file.relative_path)?;
            let owned = owned_paths.contains(file.relative_path.as_str());
            if owned || (exists && !overwrite_conflicts) {
                skipped += 1;
            } else {
                selected.push((file, exists));
            }
        }
        if selected.is_empty() {
            return Err("No files can be installed without replacing existing game files.".into());
        }

        let id = unique_id();
        let mod_data_dir = self.data_dir.join("mods").join(&id);
        let payload_dir = mod_data_dir.join("payload");
        fs::create_dir_all(&payload_dir)
            .map_err(|error| format!("Could not create managed mod storage: {error}"))?;

        let mut installed: Vec<(String, bool)> = Vec::new();
        let result = (|| {
            for (file, has_backup) in selected {
                let relative = safe_relative_path(&file.relative_path)?;
                let payload = payload_dir.join(&relative);
                create_parent_directories(&payload_dir, &relative)?;
                fs::copy(&file.staged_path, &payload)
                    .map_err(|error| format!("Could not store {}: {error}", file.relative_path))?;

                let destination = safe_game_destination(game_dir, &relative)?;
                if has_backup {
                    let metadata = fs::symlink_metadata(&destination).map_err(|error| {
                        format!("Could not inspect conflict {}: {error}", file.relative_path)
                    })?;
                    if metadata.file_type().is_symlink() || !metadata.is_file() {
                        return Err(format!(
                            "Refusing to overwrite a non-regular file: {}",
                            file.relative_path
                        ));
                    }
                    let backup = mod_data_dir.join("original").join(&relative);
                    fs::create_dir_all(mod_data_dir.join("original"))
                        .map_err(|error| format!("Could not create backup storage: {error}"))?;
                    create_parent_directories(&mod_data_dir.join("original"), &relative)?;
                    fs::copy(&destination, &backup)
                        .map_err(|error| format!("Could not back up existing file: {error}"))?;
                    copy_replace_file(&payload, &destination)?;
                } else {
                    copy_new_file(&payload, &destination)?;
                }
                installed.push((file.relative_path.clone(), has_backup));
            }

            let managed = ManagedMod {
                id: id.clone(),
                name: if preview.name.trim().is_empty() {
                    preview.archive_name.clone()
                } else {
                    preview.name.trim().to_owned()
                },
                archive_name: preview.archive_name.clone(),
                enabled: true,
                skipped_conflicts: skipped,
                files: installed
                    .iter()
                    .map(|(relative_path, has_backup)| ManagedFile {
                        relative_path: relative_path.clone(),
                        has_backup: *has_backup,
                    })
                    .collect(),
            };
            self.manifest.mods.push(managed);
            if let Err(error) = self.save_manifest() {
                self.manifest.mods.retain(|entry| entry.id != id);
                return Err(error);
            }
            Ok((installed.len(), skipped))
        })();

        if result.is_err() {
            for (relative, has_backup) in &installed {
                if let Ok(path) = safe_game_destination(game_dir, Path::new(relative)) {
                    let rollback = if *has_backup {
                        let backup = mod_data_dir.join("original").join(relative);
                        copy_replace_file(&backup, &path)
                    } else {
                        fs::remove_file(path)
                            .map_err(|error| format!("Could not roll back mod file: {error}"))
                    };
                    if let Err(error) = rollback {
                        log::warn!("Could not roll back installed mod file {relative}: {error}");
                    }
                }
            }
            if let Err(error) = fs::remove_dir_all(&mod_data_dir) {
                if error.kind() != io::ErrorKind::NotFound {
                    log::warn!("Could not remove failed mod install data: {error}");
                }
            }
        }
        result
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool, game_dir: &Path) -> Result<(), String> {
        let index = self
            .manifest
            .mods
            .iter()
            .position(|managed| managed.id == id)
            .ok_or_else(|| "That mod is no longer in the registry.".to_owned())?;
        if self.manifest.mods[index].enabled == enabled {
            return Ok(());
        }

        let managed = self.manifest.mods[index].clone();
        if enabled {
            for file in &managed.files {
                let relative = safe_relative_path(&file.relative_path)?;
                let destination = safe_game_destination(game_dir, &relative)?;
                let backup = self.backup_path(&managed.id, &relative);
                if file.has_backup {
                    if !files_match(&destination, &backup)? {
                        return Err(format!(
                            "Cannot enable '{}': original file {} is missing or changed. It was left untouched.",
                            managed.name, file.relative_path
                        ));
                    }
                } else if destination_exists(game_dir, &file.relative_path)? {
                    return Err(format!(
                        "Cannot enable '{}': {} already exists. It was left untouched.",
                        managed.name, file.relative_path
                    ));
                }
                let payload = self.payload_path(&managed.id, &relative);
                if !payload.is_file() {
                    return Err(format!(
                        "Managed copy is missing for {}. The mod was not enabled.",
                        file.relative_path
                    ));
                }
            }
            let mut restored: Vec<(PathBuf, bool, PathBuf)> = Vec::new();
            for file in &managed.files {
                let relative = safe_relative_path(&file.relative_path)?;
                let destination = safe_game_destination(game_dir, &relative)?;
                let payload = self.payload_path(&managed.id, &relative);
                let backup = self.backup_path(&managed.id, &relative);
                let result = if file.has_backup {
                    copy_replace_file(&payload, &destination)
                } else {
                    copy_new_file(&payload, &destination)
                };
                if let Err(error) = result {
                    for (path, had_backup, backup_path) in restored {
                        if had_backup {
                            let _ = copy_replace_file(&backup_path, &path);
                        } else {
                            let _ = fs::remove_file(path);
                        }
                    }
                    return Err(format!("Could not enable '{}': {error}", managed.name));
                }
                restored.push((destination, file.has_backup, backup));
            }
        } else {
            for file in &managed.files {
                let relative = safe_relative_path(&file.relative_path)?;
                let destination = safe_game_destination(game_dir, &relative)?;
                let payload = self.payload_path(&managed.id, &relative);
                if !files_match(&destination, &payload)? {
                    return Err(format!(
                        "Cannot disable '{}': {} is missing or was changed outside the manager. It was left untouched.",
                        managed.name, file.relative_path
                    ));
                }
            }
            let mut removed: Vec<(String, PathBuf)> = Vec::new();
            for file in &managed.files {
                let destination =
                    safe_game_destination(game_dir, &safe_relative_path(&file.relative_path)?)?;
                let result = if file.has_backup {
                    let relative = safe_relative_path(&file.relative_path)?;
                    copy_replace_file(&self.backup_path(&managed.id, &relative), &destination)
                } else {
                    fs::remove_file(&destination)
                        .map_err(|error| format!("Could not remove managed file: {error}"))
                };
                if let Err(error) = result {
                    for (relative, path) in removed {
                        let payload = self.payload_path(&managed.id, Path::new(&relative));
                        let _ = copy_replace_file(&payload, &path);
                    }
                    return Err(format!("Could not disable '{}': {error}", managed.name));
                }
                removed.push((file.relative_path.clone(), destination));
            }
        }

        self.manifest.mods[index].enabled = enabled;
        if let Err(error) = self.save_manifest() {
            self.manifest.mods[index].enabled = !enabled;
            if enabled {
                for file in &managed.files {
                    if let Ok(destination) =
                        safe_game_destination(game_dir, Path::new(&file.relative_path))
                    {
                        if file.has_backup {
                            if let Ok(relative) = safe_relative_path(&file.relative_path) {
                                let backup = self.backup_path(&managed.id, &relative);
                                let _ = copy_replace_file(&backup, &destination);
                            }
                        } else {
                            let _ = fs::remove_file(destination);
                        }
                    }
                }
            } else {
                for file in &managed.files {
                    if let Ok(relative) = safe_relative_path(&file.relative_path) {
                        if let Ok(destination) = safe_game_destination(game_dir, &relative) {
                            let payload = self.payload_path(&managed.id, &relative);
                            let _ = copy_replace_file(&payload, &destination);
                        }
                    }
                }
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn uninstall(&mut self, id: &str, game_dir: &Path) -> Result<(), String> {
        let index = self
            .manifest
            .mods
            .iter()
            .position(|managed| managed.id == id)
            .ok_or_else(|| "That mod is no longer in the registry.".to_owned())?;
        let managed = self.manifest.mods[index].clone();

        if managed.enabled {
            for file in &managed.files {
                let relative = safe_relative_path(&file.relative_path)?;
                let destination = safe_game_destination(game_dir, &relative)?;
                let payload = self.payload_path(&managed.id, &relative);
                if !files_match(&destination, &payload)? {
                    return Err(format!(
                        "Cannot uninstall '{}': {} is missing or was changed outside the manager. The file was left untouched.",
                        managed.name, file.relative_path
                    ));
                }
            }
        }

        let mut removed: Vec<(String, PathBuf)> = Vec::new();
        if managed.enabled {
            for file in &managed.files {
                let relative = safe_relative_path(&file.relative_path)?;
                let destination = safe_game_destination(game_dir, &relative)?;
                let result = if file.has_backup {
                    copy_replace_file(&self.backup_path(&managed.id, &relative), &destination)
                } else {
                    fs::remove_file(&destination)
                        .map_err(|error| format!("Could not remove managed file: {error}"))
                };
                if let Err(error) = result {
                    for (relative_path, path) in &removed {
                        if let Ok(relative) = safe_relative_path(relative_path) {
                            let payload = self.payload_path(&managed.id, &relative);
                            let _ = copy_replace_file(&payload, path);
                        }
                    }
                    return Err(format!("Could not uninstall '{}': {error}", managed.name));
                }
                removed.push((file.relative_path.clone(), destination));
            }
        }

        self.manifest.mods.remove(index);
        if let Err(error) = self.save_manifest() {
            self.manifest.mods.insert(index, managed.clone());
            for (relative_path, path) in removed {
                if let Ok(relative) = safe_relative_path(&relative_path) {
                    let payload = self.payload_path(&managed.id, &relative);
                    let _ = copy_replace_file(&payload, &path);
                }
            }
            return Err(error);
        }

        let mod_data_dir = self.data_dir.join("mods").join(&managed.id);
        if let Err(error) = fs::remove_dir_all(mod_data_dir) {
            if error.kind() != io::ErrorKind::NotFound {
                log::warn!(
                    "Could not clean up stored files for '{}': {error}",
                    managed.name
                );
            }
        }
        Ok(())
    }

    fn payload_path(&self, id: &str, relative: &Path) -> PathBuf {
        self.data_dir
            .join("mods")
            .join(id)
            .join("payload")
            .join(relative)
    }

    fn backup_path(&self, id: &str, relative: &Path) -> PathBuf {
        self.data_dir
            .join("mods")
            .join(id)
            .join("original")
            .join(relative)
    }

    fn save_manifest(&self) -> Result<(), String> {
        fs::create_dir_all(&self.data_dir)
            .map_err(|error| format!("Could not create mod registry directory: {error}"))?;
        let serialized = toml::to_string_pretty(&self.manifest)
            .map_err(|error| format!("Could not save mod registry: {error}"))?;
        let temporary = self.data_dir.join("mods.toml.tmp");
        fs::write(&temporary, serialized)
            .map_err(|error| format!("Could not write mod registry: {error}"))?;
        fs::rename(&temporary, self.data_dir.join(MANIFEST_FILE))
            .map_err(|error| format!("Could not update mod registry: {error}"))
    }
}

fn make_staging_dir() -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join(format!("sekai-hunter-{}", unique_id()));
    fs::create_dir(&path).map_err(|error| {
        format!(
            "Could not create temporary archive directory {}: {error}",
            path.display()
        )
    })?;
    Ok(path)
}

fn unique_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{nanos}", std::process::id())
}

fn extract_zip(archive_path: &Path, staging_dir: &Path) -> Result<usize, String> {
    let file = File::open(archive_path)
        .map_err(|error| format!("Could not open {}: {error}", archive_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("Could not read ZIP archive: {error}"))?;
    let mut ignored = 0;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("Could not read ZIP entry: {error}"))?;
        let Some(relative) = entry.enclosed_name() else {
            ignored += 1;
            continue;
        };
        if !is_safe_relative(&relative) {
            ignored += 1;
            continue;
        }
        let is_symlink = entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000);
        if is_symlink {
            ignored += 1;
            continue;
        }
        let target = staging_dir.join(&relative);
        if entry.is_dir() {
            fs::create_dir_all(&target)
                .map_err(|error| format!("Could not create archive directory: {error}"))?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("Could not create archive directory: {error}"))?;
            }
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map_err(|error| format!("Could not extract {}: {error}", relative.display()))?;
            io::copy(&mut entry, &mut output)
                .map_err(|error| format!("Could not extract {}: {error}", relative.display()))?;
        }
    }
    Ok(ignored)
}

fn extract_rar(archive_path: &Path, staging_dir: &Path) -> Result<usize, String> {
    for (program, args) in [
        ("bsdtar", vec!["-xf"]),
        ("unrar", vec!["x", "-o-", "-y"]),
        ("7z", vec!["x", "-y"]),
    ] {
        let mut command = Command::new(program);
        match program {
            "bsdtar" => {
                command
                    .args(&args)
                    .arg(archive_path)
                    .arg("-C")
                    .arg(staging_dir);
            }
            "unrar" => {
                let mut output_dir = staging_dir.as_os_str().to_os_string();
                output_dir.push(std::path::MAIN_SEPARATOR.to_string());
                command.args(&args).arg(archive_path).arg(output_dir);
            }
            "7z" => {
                command
                    .args(&args)
                    .arg(format!("-o{}", staging_dir.display()))
                    .arg(archive_path);
            }
            _ => unreachable!(),
        }

        match command.output() {
            Ok(output) if output.status.success() => return Ok(0),
            Ok(output) => {
                let _ = fs::remove_dir_all(staging_dir);
                fs::create_dir(staging_dir)
                    .map_err(|error| format!("Could not reset temporary extraction: {error}"))?;
                log::debug!(
                    "{program} could not extract RAR: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => log::debug!("Could not start {program}: {error}"),
        }
    }
    Err("Could not extract this RAR archive. Install bsdtar, unrar, or 7z and try again.".into())
}

fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(PathBuf, PathBuf)>,
    ignored: &mut usize,
) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            *ignored += 1;
        } else if metadata.is_dir() {
            collect_files(root, &path, files, ignored)?;
        } else if metadata.is_file() {
            files.push((path.strip_prefix(root).unwrap_or(&path).to_path_buf(), path));
        } else {
            *ignored += 1;
        }
    }
    Ok(())
}

fn map_game_files(
    staged_files: Vec<(PathBuf, PathBuf)>,
) -> Result<(Vec<PreviewFile>, usize), String> {
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    let mut ignored = 0;

    for (archive_relative, staged_path) in staged_files {
        let components: Vec<_> = archive_relative
            .components()
            .filter_map(|component| match component {
                Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        let marker = components
            .iter()
            .position(|component| component == "nativePC" || component == "AsteraLoader");
        let relative = if let Some(index) = marker {
            components[index..].join("/")
        } else {
            let root_file = components.len() == 1
                && components[0]
                    .rsplit_once('.')
                    .is_some_and(|(_, ext)| is_installable_root_extension(ext));
            let wrapper_file = components.len() == 2
                && components[1]
                    .rsplit_once('.')
                    .is_some_and(|(_, ext)| is_installable_root_extension(ext));
            if root_file || wrapper_file {
                components.last().cloned().unwrap_or_default()
            } else {
                ignored += 1;
                continue;
            }
        };

        let safe = safe_relative_path(&relative)?;
        let relative = safe.to_string_lossy().replace('\\', "/");
        if !seen.insert(relative.clone()) {
            return Err(format!(
                "The archive contains duplicate destination paths: {relative}"
            ));
        }
        files.push(PreviewFile {
            relative_path: relative,
            staged_path,
        });
    }
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok((files, ignored))
}

fn is_documentation_extension(extension: &str) -> bool {
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "txt" | "md" | "html" | "htm" | "pdf" | "url" | "png" | "jpg" | "jpeg"
    )
}

fn is_installable_root_extension(extension: &str) -> bool {
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "dll" | "ini" | "cfg" | "json" | "xml"
    ) && !is_documentation_extension(extension)
}

fn safe_relative_path(path: &str) -> Result<PathBuf, String> {
    let relative = PathBuf::from(path);
    if path.contains('\\') || path.contains('\0') || !is_safe_relative(&relative) {
        return Err(format!("Unsafe archive path was rejected: {path}"));
    }
    Ok(relative)
}

fn is_safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn destination_exists(game_dir: &Path, relative: &str) -> Result<bool, String> {
    let relative = safe_relative_path(relative)?;
    let path = game_dir.join(relative);
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("Could not inspect existing game files: {error}")),
    }
}

fn safe_game_destination(game_dir: &Path, relative: &Path) -> Result<PathBuf, String> {
    if !is_safe_relative(relative) {
        return Err(format!(
            "Unsafe game path was rejected: {}",
            relative.display()
        ));
    }
    let root = fs::canonicalize(game_dir)
        .map_err(|error| format!("Could not access game directory: {error}"))?;
    let mut current = root.clone();
    let components: Vec<_> = relative.components().collect();
    for component in &components[..components.len().saturating_sub(1)] {
        let Component::Normal(name) = component else {
            return Err("Unsafe game path was rejected.".into());
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(format!(
                    "Refusing to install through a non-directory or symbolic link: {}",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&current)
                    .map_err(|error| format!("Could not create game folder: {error}"))?;
            }
            Err(error) => return Err(format!("Could not inspect game folder: {error}")),
        }
    }
    let Component::Normal(name) = components
        .last()
        .ok_or_else(|| "Empty game path was rejected.".to_owned())?
    else {
        return Err("Unsafe game path was rejected.".into());
    };
    current.push(name);
    if !current.starts_with(&root) {
        return Err("Unsafe game path was rejected.".into());
    }
    Ok(current)
}

fn create_parent_directories(base: &Path, relative: &Path) -> Result<(), String> {
    let mut current = base.to_path_buf();
    for component in relative.parent().into_iter().flat_map(Path::components) {
        let Component::Normal(name) = component else {
            return Err("Unsafe managed mod path was rejected.".into());
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err("Managed mod storage contains an unsafe path.".into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&current)
                    .map_err(|error| format!("Could not create managed mod folder: {error}"))?;
            }
            Err(error) => return Err(format!("Could not inspect managed mod folder: {error}")),
        }
    }
    Ok(())
}

fn copy_new_file(source: &Path, destination: &Path) -> Result<(), String> {
    let mut input =
        File::open(source).map_err(|error| format!("Could not read managed mod file: {error}"))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| format!("Could not create game file: {error}"))?;
    if let Err(error) = io::copy(&mut input, &mut output) {
        drop(output);
        let _ = fs::remove_file(destination);
        return Err(format!("Could not copy game file: {error}"));
    }
    Ok(())
}

fn copy_replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("managed-file");
    let temporary = destination.with_file_name(format!(".{file_name}.{}.tmp", unique_id()));
    if let Err(error) = copy_new_file(source, &temporary) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temporary, destination) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("Could not replace game file: {error}"));
    }
    Ok(())
}

fn files_match(first: &Path, second: &Path) -> Result<bool, String> {
    let first_metadata = match fs::symlink_metadata(first) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => metadata,
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("Could not inspect game file: {error}")),
    };
    let second_metadata = fs::metadata(second)
        .map_err(|error| format!("Could not inspect stored mod file: {error}"))?;
    if first_metadata.len() != second_metadata.len() {
        return Ok(false);
    }
    let mut first =
        File::open(first).map_err(|error| format!("Could not read game file: {error}"))?;
    let mut second =
        File::open(second).map_err(|error| format!("Could not read stored mod file: {error}"))?;
    let mut first_buffer = [0; 16 * 1024];
    let mut second_buffer = [0; 16 * 1024];
    loop {
        let first_len = first
            .read(&mut first_buffer)
            .map_err(|error| format!("Could not read game file: {error}"))?;
        let second_len = second
            .read(&mut second_buffer)
            .map_err(|error| format!("Could not read stored mod file: {error}"))?;
        if first_len != second_len || first_buffer[..first_len] != second_buffer[..second_len] {
            return Ok(false);
        }
        if first_len == 0 {
            return Ok(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{safe_relative_path, ModManager};
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
    use zip::write::SimpleFileOptions;

    fn test_directory(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{name}-{}-{unique}", std::process::id()))
    }

    fn make_zip(path: &std::path::Path, files: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        for (name, data) in files {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn installs_disables_enables_and_uninstalls_only_managed_files() {
        let root = test_directory("sekai-manager-test");
        let game = root.join("game");
        let data = root.join("data");
        fs::create_dir_all(&game).unwrap();
        let archive = root.join("sample.zip");
        make_zip(
            &archive,
            &[
                ("Mod/nativePC/plugins/test.dll", b"mod payload"),
                ("Mod/README.txt", b"install instructions"),
            ],
        );

        let mut manager = ModManager::load_from(data).unwrap();
        let preview = ModManager::preview_archive(&archive, &game).unwrap();
        assert_eq!(preview.files[0].relative_path, "nativePC/plugins/test.dll");
        assert_eq!(preview.ignored_entries, 1);
        manager.install(&preview, &game, true).unwrap();
        drop(preview);

        let installed_path = game.join("nativePC/plugins/test.dll");
        assert_eq!(fs::read(&installed_path).unwrap(), b"mod payload");
        let data_dir = manager.data_dir.clone();
        drop(manager);
        let mut manager = ModManager::load_from(data_dir).unwrap();
        assert_eq!(manager.mods().len(), 1);
        let id = manager.mods()[0].id.clone();
        manager.set_enabled(&id, false, &game).unwrap();
        assert!(!installed_path.exists());
        manager.set_enabled(&id, true, &game).unwrap();
        assert_eq!(fs::read(&installed_path).unwrap(), b"mod payload");
        manager.uninstall(&id, &game).unwrap();
        assert!(!installed_path.exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn never_overwrites_conflicts_or_removes_manually_changed_files() {
        let root = test_directory("sekai-manager-safety-test");
        let game = root.join("game");
        let data = root.join("data");
        fs::create_dir_all(game.join("nativePC/plugins")).unwrap();
        let installed_path = game.join("nativePC/plugins/test.dll");
        fs::write(&installed_path, b"existing file").unwrap();
        let archive = root.join("sample.zip");
        make_zip(&archive, &[("nativePC/plugins/test.dll", b"mod payload")]);

        let mut manager = ModManager::load_from(data).unwrap();
        let preview = ModManager::preview_archive(&archive, &game).unwrap();
        assert_eq!(preview.conflicts.len(), 1);
        assert!(manager.install(&preview, &game, false).is_err());
        assert_eq!(fs::read(&installed_path).unwrap(), b"existing file");
        drop(preview);

        make_zip(&archive, &[("nativePC/plugins/new.dll", b"mod payload")]);
        let preview = ModManager::preview_archive(&archive, &game).unwrap();
        manager.install(&preview, &game, true).unwrap();
        drop(preview);
        let id = manager.mods()[0].id.clone();
        let managed_path = game.join("nativePC/plugins/new.dll");
        fs::write(&managed_path, b"user edit").unwrap();

        assert!(manager.uninstall(&id, &game).is_err());
        assert_eq!(fs::read(&managed_path).unwrap(), b"user edit");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_unsafe_paths() {
        assert!(safe_relative_path("../outside").is_err());
        assert!(safe_relative_path("/absolute/path").is_err());
        assert!(safe_relative_path("nativePC/plugins/mod.dll").is_ok());
    }

    #[test]
    fn archive_paths_cannot_escape_the_game_or_staging_directory() {
        let root = test_directory("sekai-manager-traversal-test");
        let game = root.join("game");
        fs::create_dir_all(&game).unwrap();
        let archive = root.join("unsafe.zip");
        make_zip(&archive, &[("../outside.dll", b"must not escape")]);

        assert!(ModManager::preview_archive(&archive, &game).is_err());
        assert!(!root.join("outside.dll").exists());
        assert!(!game.join("outside.dll").exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restores_existing_files_when_overwrite_is_confirmed_and_mod_uninstalled() {
        let root = test_directory("sekai-manager-backup-test");
        let game = root.join("game");
        let data = root.join("data");
        fs::create_dir_all(game.join("nativePC/plugins")).unwrap();
        let installed_path = game.join("nativePC/plugins/test.dll");
        fs::write(&installed_path, b"original").unwrap();
        let archive = root.join("sample.zip");
        make_zip(&archive, &[("nativePC/plugins/test.dll", b"mod payload")]);

        let mut manager = ModManager::load_from(data).unwrap();
        let preview = ModManager::preview_archive(&archive, &game).unwrap();
        let (installed, skipped) = manager.install(&preview, &game, true).unwrap();
        assert_eq!((installed, skipped), (1, 0));
        drop(preview);
        assert_eq!(fs::read(&installed_path).unwrap(), b"mod payload");

        let id = manager.mods()[0].id.clone();
        manager.set_enabled(&id, false, &game).unwrap();
        assert_eq!(fs::read(&installed_path).unwrap(), b"original");
        manager.set_enabled(&id, true, &game).unwrap();
        assert_eq!(fs::read(&installed_path).unwrap(), b"mod payload");
        manager.uninstall(&id, &game).unwrap();
        assert_eq!(fs::read(&installed_path).unwrap(), b"original");

        fs::remove_dir_all(root).unwrap();
    }
}
