use std::{fs, io};
use std::fs::File;
use std::io::ErrorKind::NotFound;
use std::path::{Path, PathBuf};
use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use log::LevelFilter;
use octocrab::models::{Repository, RepositoryId};
use octocrab::Octocrab;
use pelite::FileMap;
use pelite::pe32::Pe as Pe32;
use pelite::pe64::Pe as Pe64;
use pelite::resources::version_info::Language;
use sha2::{Digest, Sha256};
use simple_logger::SimpleLogger;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;
use crate::downloader::download;

pub mod downloader;
pub mod config;

#[derive(Debug, Parser)]
struct Args {
    #[arg(long, default_value = "osrs-win")]
    repo: String,

    #[arg(long, default_value = "production")]
    build: String,

    #[arg(long, default_value = "osrs-win.production.zip")]
    artifact_name: String,

    #[arg(long, default_value = "downloads/")]
    output_dir: String,

    #[arg(long, env = "GITHUB_TOKEN")]
    github_token: String,

    #[arg(long, default_value = "cozmoe0")]
    github_owner: String,

    #[arg(long, default_value = "osrs-archive")]
    github_repo: String
}

#[tokio::main]
async fn main() -> Result<()> {
    SimpleLogger::new()
        .with_colors(true)
        .with_level(LevelFilter::Info)
        .init()?;

    let args = Args::parse();

    let output_dir = PathBuf::from(&args.output_dir);
    let artifact_path = download_files(args.repo, args.build, &output_dir, args.artifact_name).await?;

    let checksum = calculate_checksum(&artifact_path).await?;
    log::info!("Calculated artifact checksum: {}", checksum);

    let version = extract_versions_from_directory(&output_dir)?;
    log::info!("Extracted artifact version: {}", version);

    let github = Octocrab::builder()
        .personal_token(args.github_token.clone())
        .build()?;

    let should_create_release = should_create_release(
        &github,
        args.github_owner,
        args.github_repo,
        version.clone(),
        checksum.clone()
    ).await?;

    if should_create_release {
        log::info!("Update detected - setting outputs for GitHub actions");

        println!("::set-output name=update_available::true");
        println!("::set-output name=version::{}", version.clone());
        println!("::set-output name=checksum::{}", checksum.clone());
        println!("::set-output name=artifact_path::{}", artifact_path.display());

        if let Ok(output_file) = std::env::var("GITHUB_OUTPUT") {
            let output = format!(
                "update_available=true\nversion={}\nchecksum={}\nartifact_path={}\n",
                version,
                checksum,
                artifact_path.display()
            );
            fs::write(output_file, output)?;
        }

        log::info!("Release will be created by Github Actions");
    } else {
        log::info!("No update detected - Latest release is already at version {}", version);

        println!("::set-output name=update_available::false");

        if let Ok(output_file) = std::env::var("GITHUB_OUTPUT") {
            fs::write(output_file, "update_available=false\n")?;
        }

        if let Err(e) = tokio::fs::remove_file(&artifact_path).await {
            log::warn!("Failed to clean up downloaded artifact file: {}", e);
        }
    }

    Ok(())
}

async fn download_files(repo: String, build: String, output_dir: &Path, artifact_name: String) -> Result<PathBuf> {
    download(repo.as_str(), build.as_str(), &output_dir.to_path_buf()).await?;

    log::info!("Compressing files into artifact archive...");

    zip_directory(&output_dir, &output_dir.join(&artifact_name))?;
    log::info!("Compressed files into artifact archive {}", &artifact_name);

    Ok(output_dir.join(artifact_name))
}

async fn should_create_release(
    github: &Octocrab,
    owner: String,
    repo: String,
    version: String,
    checksum: String
) -> Result<bool> {
    let latest_release = github
        .repos(&owner, &repo)
        .releases()
        .get_latest()
        .await;

    match latest_release {
        Ok(release) => {
            log::info!("Latest release: {}", release.tag_name);

            if release.tag_name != version {
                log::info!("Version different from latest release version. ({} vs {})", version, release.tag_name);
                return Ok(true);
            }

            if let Some(body) = release.body {
                if body.contains(&checksum) {
                    log::info!("Checksum found in release body. No updated files.");
                    return Ok(false)
                }
            }

            let assets = github
                .repos(&owner, &repo)
                .releases()
                .get_latest()
                .await?
                .assets;

            if assets.is_empty() {
                log::info!("No assets found in latest release. Creating new release.");
                return Ok(true);
            }

            log::info!("Same version but different content (checksum), creating new release.");
            return Ok(true);
        }
        Err(e) => {
            log::warn!("Failed to get latest release, assuming this is the first release: {}", e);
            return Ok(true);
        }
    }

    panic!("Not implemented");
}

fn extract_versions_from_directory(dir: &Path) -> Result<String> {
    log::info!("Scanning directory for executable files: {}", dir.display());
    
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        
        if path.is_file() {
            if let Some(extension) = path.extension() {
                if extension.eq_ignore_ascii_case("exe") || extension.eq_ignore_ascii_case("dll") {
                    match extract_version_info(&path) {
                        Ok(Some(version_info)) => {
                            return Ok(version_info.file_version.unwrap());
                        }
                        Ok(None) => {
                            continue;
                        }
                        Err(e) => {
                            continue;
                        }
                    }
                }
            }
        }
    }
    
    Ok("NONE".to_string())
}

fn zip_directory(src_dir: &Path, zip_file_path: &Path) -> io::Result<()> {
    let file = File::create(zip_file_path)?;
    let mut zip = ZipWriter::new(file);

    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o755);

    for entry in fs::read_dir(src_dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = path.strip_prefix(src_dir).unwrap().to_str().unwrap();

        if path.extension().unwrap() == "zip" {
            continue;
        }

        if path.is_file() {
            zip.start_file(name, options)?;
            let mut f = File::open(&path)?;
            io::copy(&mut f, &mut zip)?;
        } else if path.is_dir() {
            zip.add_directory(name, options)?;
        }
    }

    zip.finish()?;

    Ok(())
}

async fn calculate_checksum(file_path: &Path) -> Result<String> {
    let file_data = tokio::fs::read(file_path)
        .await
        .context("Failed to create directory for checksum file")?;

    let mut hasher = Sha256::new();
    hasher.update(&file_data);
    let checksum = hex::encode(hasher.finalize());

    Ok(checksum)
}

#[derive(Debug, Clone)]
pub struct ExecutableVersionInfo {
    pub file_version: Option<String>,
    pub product_version: Option<String>,
    pub company_name: Option<String>,
    pub product_name: Option<String>,
    pub file_description: Option<String>,
    pub copyright: Option<String>,
    pub original_filename: Option<String>,
    pub internal_name: Option<String>,
}

impl ExecutableVersionInfo {
    pub fn new() -> Self {
        Self {
            file_version: None,
            product_version: None,
            company_name: None,
            product_name: None,
            file_description: None,
            copyright: None,
            original_filename: None,
            internal_name: None,
        }
    }
}

impl std::fmt::Display for ExecutableVersionInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        
        if let Some(ref product_name) = self.product_name {
            parts.push(format!("Product: {}", product_name));
        }
        if let Some(ref file_version) = self.file_version {
            parts.push(format!("File Version: {}", file_version));
        }
        if let Some(ref product_version) = self.product_version {
            parts.push(format!("Product Version: {}", product_version));
        }
        if let Some(ref company_name) = self.company_name {
            parts.push(format!("Company: {}", company_name));
        }
        if let Some(ref file_description) = self.file_description {
            parts.push(format!("Description: {}", file_description));
        }
        if let Some(ref copyright) = self.copyright {
            parts.push(format!("Copyright: {}", copyright));
        }
        
        write!(f, "{}", parts.join(", "))
    }
}

fn extract_version_info(file_path: &Path) -> Result<Option<ExecutableVersionInfo>> {
    let map = FileMap::open(file_path)
        .with_context(|| format!("Failed to open PE file: {}", file_path.display()))?;
    
    // Try PE64 first, then fall back to PE32
    let version_info = match pelite::pe64::PeFile::from_bytes(&map) {
        Ok(pe) => {
            extract_version_from_pe_resources(pe.resources()?)
        }
        Err(pelite::Error::PeMagic) => {
            // Not a PE64 file, try PE32
            let pe = pelite::pe32::PeFile::from_bytes(&map)
                .context("File is neither a valid PE32 nor PE64 executable")?;
            extract_version_from_pe_resources(pe.resources()?)
        }
        Err(e) => {
            return Err(anyhow::anyhow!("Failed to parse PE file: {}", e));
        }
    };
    
    Ok(version_info)
}

fn extract_version_from_pe_resources(
    resources: pelite::resources::Resources
) -> Option<ExecutableVersionInfo> {
    let version_info = resources.version_info().ok()?;
    let file_info = version_info.file_info();
    
    // Try to get the default language first, or use the first available
    let lang = version_info.translation()
        .first()
        .copied()
        .unwrap_or(Language::default());
    
    let mut exe_info = ExecutableVersionInfo::new();
    
    // Extract common version information fields
    if let Some(strings) = file_info.strings.get(&lang) {
        exe_info.file_version = strings.get("FileVersion").map(|s| s.to_string());
        exe_info.product_version = strings.get("ProductVersion").map(|s| s.to_string());
        exe_info.company_name = strings.get("CompanyName").map(|s| s.to_string());
        exe_info.product_name = strings.get("ProductName").map(|s| s.to_string());
        exe_info.file_description = strings.get("FileDescription").map(|s| s.to_string());
        exe_info.copyright = strings.get("LegalCopyright").map(|s| s.to_string());
        exe_info.original_filename = strings.get("OriginalFilename").map(|s| s.to_string());
        exe_info.internal_name = strings.get("InternalName").map(|s| s.to_string());
    }
    
    Some(exe_info)
}

/// Convenience function that extracts just the file version as a string
pub fn extract_file_version(file_path: &Path) -> Result<Option<String>> {
    match extract_version_info(file_path)? {
        Some(version_info) => Ok(version_info.file_version),
        None => Ok(None),
    }
}

/// Convenience function that extracts just the product version as a string
pub fn extract_product_version(file_path: &Path) -> Result<Option<String>> {
    match extract_version_info(file_path)? {
        Some(version_info) => Ok(version_info.product_version),
        None => Ok(None),
    }
}