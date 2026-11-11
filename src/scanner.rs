use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Overall game status summary.
#[derive(Debug, Clone)]
pub struct GameStatus {
    pub game_dir: Option<PathBuf>,
    pub mhw_exe_found: bool,
    pub astera_exe_found: bool,
    pub astera_dir_found: bool,
    pub native_pc_plugins_found: bool,
    pub astera_plugins_found: bool,
    pub plugin_count: usize,
    pub stracker_plugin_count: usize,
    pub astera_plugin_count: usize,
    pub plugin_names: Vec<String>,
    pub legacy_dinput8: bool,
    pub legacy_loader_dll: bool,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoaderStatus {
    Astera,
    Stracker,
    Conflict,
    AsteraIncomplete,
    StrackerIncomplete,
    Missing,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub level: DiagLevel,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagLevel {
    Info,
    Warning,
    Error,
}

impl fmt::Display for DiagLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiagLevel::Info => write!(f, "INFO"),
            DiagLevel::Warning => write!(f, "WARN"),
            DiagLevel::Error => write!(f, "ERROR"),
        }
    }
}

impl GameStatus {
    pub fn empty() -> Self {
        Self {
            game_dir: None,
            mhw_exe_found: false,
            astera_exe_found: false,
            astera_dir_found: false,
            native_pc_plugins_found: false,
            astera_plugins_found: false,
            plugin_count: 0,
            stracker_plugin_count: 0,
            astera_plugin_count: 0,
            plugin_names: Vec::new(),
            legacy_dinput8: false,
            legacy_loader_dll: false,
            diagnostics: Vec::new(),
        }
    }

    /// Scan a game directory and build a complete status report.
    pub fn scan(game_dir: &Path) -> Self {
        let mut status = Self::empty();
        status.game_dir = Some(game_dir.to_path_buf());

        if !game_dir.is_dir() {
            status.diagnostics.push(Diagnostic {
                level: DiagLevel::Error,
                message: format!("Directory does not exist: {}", game_dir.display()),
            });
            return status;
        }

        // MonsterHunterWorld.exe
        status.mhw_exe_found = game_dir.join("MonsterHunterWorld.exe").exists();
        if !status.mhw_exe_found {
            status.diagnostics.push(Diagnostic {
                level: DiagLevel::Error,
                message: "MonsterHunterWorld.exe not found in game directory.".into(),
            });
        }

        // Loader files
        status.astera_exe_found = game_dir.join("AsteraLoader.exe").exists();
        status.astera_dir_found = game_dir.join("AsteraLoader").is_dir();
        status.legacy_dinput8 = game_dir.join("dinput8.dll").exists();
        status.legacy_loader_dll = game_dir.join("loader.dll").exists();

        let loader_status = status.loader_status();
        match loader_status {
            LoaderStatus::Astera => {}
            LoaderStatus::Stracker => {}
            LoaderStatus::Conflict => status.diagnostics.push(Diagnostic {
                level: DiagLevel::Error,
                message: "Astera Loader and Stracker's Loader files are both present. Astera's documentation says the legacy loader can start alongside it; disable/remove Stracker's Loader manually before launching. Keep nativePC/plugins.".into(),
            }),
            LoaderStatus::AsteraIncomplete => {
                if !status.astera_exe_found {
                    status.diagnostics.push(Diagnostic {
                        level: DiagLevel::Error,
                        message: "AsteraLoader.exe not found. Download it from the Astera Loader GitHub.".into(),
                    });
                }
                if !status.astera_dir_found {
                    status.diagnostics.push(Diagnostic {
                        level: DiagLevel::Error,
                        message: "AsteraLoader/ directory not found. It should be extracted alongside AsteraLoader.exe.".into(),
                    });
                }
            }
            LoaderStatus::StrackerIncomplete => status.diagnostics.push(Diagnostic {
                level: DiagLevel::Error,
                message: "Only part of Stracker's legacy loader was detected (dinput8.dll and loader.dll). Verify the installation before launching.".into(),
            }),
            LoaderStatus::Missing => status.diagnostics.push(Diagnostic {
                level: DiagLevel::Error,
                message: "No supported loader detected. Install Astera Loader or Stracker's Loader.".into(),
            }),
        }

        status.scan_plugin_directory(&game_dir.join("nativePC").join("plugins"), "Stracker");
        status.scan_plugin_directory(&game_dir.join("AsteraLoader").join("plugins"), "Astera");

        if loader_status == LoaderStatus::Stracker && status.astera_plugin_count > 0 {
            status.diagnostics.push(Diagnostic {
                level: DiagLevel::Warning,
                message: "Astera-specific plugins were found, but Stracker's Loader does not load from AsteraLoader/plugins/. Use Astera Loader for these plugins.".into(),
            });
        }

        if status.is_ready() {
            status.diagnostics.push(Diagnostic {
                level: DiagLevel::Info,
                message: "Game and detected loader are ready.".into(),
            });
        }

        status
    }

    pub fn loader_status(&self) -> LoaderStatus {
        let astera_present = self.astera_exe_found || self.astera_dir_found;
        let astera_complete = self.astera_exe_found && self.astera_dir_found;
        let stracker_present = self.legacy_dinput8 || self.legacy_loader_dll;
        let stracker_complete = self.legacy_dinput8 && self.legacy_loader_dll;

        match (
            astera_present,
            astera_complete,
            stracker_present,
            stracker_complete,
        ) {
            (true, _, true, _) => LoaderStatus::Conflict,
            (_, true, false, _) => LoaderStatus::Astera,
            (true, false, false, _) => LoaderStatus::AsteraIncomplete,
            (false, _, true, true) => LoaderStatus::Stracker,
            (false, _, true, false) => LoaderStatus::StrackerIncomplete,
            (false, _, false, _) => LoaderStatus::Missing,
        }
    }

    fn scan_plugin_directory(&mut self, plugins_dir: &Path, loader: &str) {
        let is_astera = loader == "Astera";
        if is_astera {
            self.astera_plugins_found = plugins_dir.is_dir();
        } else {
            self.native_pc_plugins_found = plugins_dir.is_dir();
        }

        if !plugins_dir.is_dir() {
            return;
        }
        let count_before = self.plugin_count;
        if let Err(error) = self.scan_plugin_entries(plugins_dir, plugins_dir, loader) {
            self.diagnostics.push(Diagnostic {
                level: DiagLevel::Error,
                message: format!("Could not read {}: {error}", plugins_dir.display()),
            });
        }
        let count = self.plugin_count - count_before;
        if is_astera {
            self.astera_plugin_count = count;
        } else {
            self.stracker_plugin_count = count;
        }
        self.plugin_names.sort();
    }

    fn scan_plugin_entries(
        &mut self,
        root: &Path,
        directory: &Path,
        loader: &str,
    ) -> io::Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                self.scan_plugin_entries(root, &path, loader)?;
            } else if metadata.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"))
            {
                if let Ok(name) = path.strip_prefix(root) {
                    let components: Vec<_> = name.components().collect();
                    let is_plugin = match components.as_slice() {
                        [file] => file
                            .as_os_str()
                            .to_string_lossy()
                            .rsplit_once('.')
                            .is_some(),
                        [folder, file] => {
                            let folder = folder.as_os_str().to_string_lossy();
                            let file_stem = file
                                .as_os_str()
                                .to_string_lossy()
                                .rsplit_once('.')
                                .map(|(stem, _)| stem.to_owned());
                            file_stem.is_some_and(|stem| stem.eq_ignore_ascii_case(&folder))
                        }
                        _ => false,
                    };
                    if is_plugin {
                        self.plugin_names
                            .push(format!("{loader}: {}", name.to_string_lossy()));
                        self.plugin_count += 1;
                    }
                }
            }
        }
        Ok(())
    }

    /// Whether the game is ready to launch.
    pub fn is_ready(&self) -> bool {
        self.mhw_exe_found
            && matches!(
                self.loader_status(),
                LoaderStatus::Astera | LoaderStatus::Stracker
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{DiagLevel, GameStatus, LoaderStatus};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_directory() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after UNIX epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("sekai-hunter-test-{}-{unique}", std::process::id()))
    }

    #[test]
    fn detects_loader_plugins_and_legacy_conflicts() {
        let root = test_directory();
        fs::create_dir_all(root.join("AsteraLoader")).unwrap();
        fs::create_dir_all(root.join("nativePC/plugins/Example")).unwrap();
        fs::create_dir_all(root.join("AsteraLoader/plugins/AsteraExample")).unwrap();
        fs::write(root.join("MonsterHunterWorld.exe"), []).unwrap();
        fs::write(root.join("AsteraLoader.exe"), []).unwrap();
        fs::write(root.join("nativePC/plugins/Example/Example.dll"), []).unwrap();
        fs::write(
            root.join("AsteraLoader/plugins/AsteraExample/AsteraExample.dll"),
            [],
        )
        .unwrap();
        fs::write(root.join("dinput8.dll"), []).unwrap();
        fs::write(root.join("loader.dll"), []).unwrap();

        let status = GameStatus::scan(&root);

        assert!(!status.is_ready());
        assert_eq!(status.loader_status(), LoaderStatus::Conflict);
        assert_eq!(status.plugin_count, 2);
        assert_eq!(status.stracker_plugin_count, 1);
        assert_eq!(status.astera_plugin_count, 1);
        assert_eq!(
            status.plugin_names,
            [
                "Astera: AsteraExample/AsteraExample.dll",
                "Stracker: Example/Example.dll"
            ]
        );
        assert!(status.legacy_dinput8);
        assert!(status.legacy_loader_dll);
        assert!(status
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.level == DiagLevel::Error
                && diagnostic.message.contains("both present")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_stracker_without_requiring_astera() {
        let root = test_directory();
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("MonsterHunterWorld.exe"), []).unwrap();
        fs::write(root.join("dinput8.dll"), []).unwrap();
        fs::write(root.join("loader.dll"), []).unwrap();

        let status = GameStatus::scan(&root);

        assert!(status.is_ready());
        assert_eq!(status.loader_status(), LoaderStatus::Stracker);
        assert!(!status.diagnostics.iter().any(|diagnostic| {
            diagnostic.level == DiagLevel::Error && diagnostic.message.contains("AsteraLoader.exe")
        }));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_astera_without_legacy_stracker_files() {
        let root = test_directory();
        fs::create_dir_all(root.join("AsteraLoader")).unwrap();
        fs::write(root.join("MonsterHunterWorld.exe"), []).unwrap();
        fs::write(root.join("AsteraLoader.exe"), []).unwrap();

        let status = GameStatus::scan(&root);

        assert!(status.is_ready());
        assert_eq!(status.loader_status(), LoaderStatus::Astera);
        assert!(!status.legacy_dinput8);
        assert!(!status.legacy_loader_dll);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn incomplete_astera_install_is_not_ready() {
        let root = test_directory();
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("MonsterHunterWorld.exe"), []).unwrap();
        fs::write(root.join("AsteraLoader.exe"), []).unwrap();

        let status = GameStatus::scan(&root);

        assert!(!status.is_ready());
        assert!(status
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("AsteraLoader/ directory")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_and_lists_more_than_one_hundred_plugins() {
        let root = test_directory();
        let plugins = root.join("nativePC/plugins");
        fs::create_dir_all(&plugins).unwrap();

        for index in 0..105 {
            let name = format!("Plugin{index:03}");
            let plugin_dir = plugins.join(&name);
            fs::create_dir_all(&plugin_dir).unwrap();
            fs::write(plugin_dir.join(format!("{name}.dll")), []).unwrap();
        }

        let status = GameStatus::scan(&root);

        assert_eq!(status.plugin_count, 105);
        assert_eq!(status.stracker_plugin_count, 105);
        assert_eq!(status.plugin_names.len(), 105);
        assert!(status
            .plugin_names
            .first()
            .unwrap()
            .starts_with("Stracker: Plugin000/"));
        assert!(status
            .plugin_names
            .last()
            .unwrap()
            .starts_with("Stracker: Plugin104/"));

        fs::remove_dir_all(root).unwrap();
    }
}
