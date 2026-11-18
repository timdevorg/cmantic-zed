//! Zed extension for C-Mantic.
//!
//! Its only job is to locate (or download) the `cmantic-ls` language server
//! and tell Zed how to launch it. All refactoring logic lives in `server/`.

use std::fs;

use zed_extension_api::{self as zed, LanguageServerId, Result, settings::LspSettings};

const SERVER_NAME: &str = "cmantic";
const BINARY_NAME: &str = "cmantic-ls";
const GITHUB_REPO: &str = "timdevorg/cmantic-zed";

struct CmanticExtension {
    cached_binary_path: Option<String>,
}

impl CmanticExtension {
    fn server_binary_path(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<String> {
        // 1. Explicit path from settings: `lsp.cmantic.binary.path`.
        if let Some(path) = LspSettings::for_worktree(SERVER_NAME, worktree)
            .ok()
            .and_then(|settings| settings.binary)
            .and_then(|binary| binary.path)
        {
            return Ok(path);
        }

        // 2. A `cmantic-ls` on the PATH.
        if let Some(path) = worktree.which(BINARY_NAME) {
            return Ok(path);
        }

        // 3. Previously downloaded binary.
        if let Some(path) = &self.cached_binary_path
            && fs::metadata(path).is_ok_and(|stat| stat.is_file())
        {
            return Ok(path.clone());
        }

        // 4. Download the latest release from GitHub.
        self.download_server(language_server_id)
    }

    fn download_server(&mut self, language_server_id: &LanguageServerId) -> Result<String> {
        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );

        let release = zed::latest_github_release(
            GITHUB_REPO,
            zed::GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        )?;

        let (os, arch) = zed::current_platform();
        let target = match (os, arch) {
            (zed::Os::Mac, zed::Architecture::Aarch64) => "aarch64-apple-darwin",
            (zed::Os::Mac, zed::Architecture::X8664) => "x86_64-apple-darwin",
            (zed::Os::Linux, zed::Architecture::Aarch64) => "aarch64-unknown-linux-gnu",
            (zed::Os::Linux, zed::Architecture::X8664) => "x86_64-unknown-linux-gnu",
            (zed::Os::Windows, zed::Architecture::X8664) => "x86_64-pc-windows-msvc",
            _ => return Err(format!("unsupported platform: {os:?} {arch:?}")),
        };
        let (extension, file_type, exe) = match os {
            zed::Os::Windows => ("zip", zed::DownloadedFileType::Zip, ".exe"),
            _ => ("tar.gz", zed::DownloadedFileType::GzipTar, ""),
        };

        let asset_name = format!("{BINARY_NAME}-{target}.{extension}");
        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == asset_name)
            .ok_or_else(|| format!("no release asset named {asset_name}"))?;

        let version_dir = format!("{BINARY_NAME}-{}", release.version);
        let binary_path = format!("{version_dir}/{BINARY_NAME}{exe}");

        if !fs::metadata(&binary_path).is_ok_and(|stat| stat.is_file()) {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );
            zed::download_file(&asset.download_url, &version_dir, file_type)
                .map_err(|e| format!("failed to download {asset_name}: {e}"))?;
            zed::make_file_executable(&binary_path)?;

            // Remove older downloaded versions.
            if let Ok(entries) = fs::read_dir(".") {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if name.starts_with(BINARY_NAME) && name != version_dir {
                        fs::remove_dir_all(entry.path()).ok();
                    }
                }
            }
        }

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::None,
        );
        self.cached_binary_path = Some(binary_path.clone());
        Ok(binary_path)
    }
}

impl zed::Extension for CmanticExtension {
    fn new() -> Self {
        Self {
            cached_binary_path: None,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let binary = LspSettings::for_worktree(SERVER_NAME, worktree)
            .ok()
            .and_then(|settings| settings.binary);

        Ok(zed::Command {
            command: self.server_binary_path(language_server_id, worktree)?,
            args: binary
                .as_ref()
                .and_then(|b| b.arguments.clone())
                .unwrap_or_default(),
            env: binary
                .and_then(|b| b.env)
                .map(|env| env.into_iter().collect())
                .unwrap_or_default(),
        })
    }

    fn language_server_initialization_options(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        Ok(LspSettings::for_worktree(SERVER_NAME, worktree)
            .ok()
            .and_then(|settings| settings.initialization_options))
    }

    fn language_server_workspace_configuration(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        Ok(LspSettings::for_worktree(SERVER_NAME, worktree)
            .ok()
            .and_then(|settings| settings.settings))
    }
}

zed::register_extension!(CmanticExtension);
