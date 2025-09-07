//! # OSRS Archive Release Updater
//!
//! This application automatically checks for updates to OSRS client files,
//! downloads them, extracts version information, and determines if a new
//! GitHub release should be created.
//!
//! ## Workflow
//!
//! 1. Downloads OSRS client files using the configured repository and build
//! 2. Compresses downloaded files into a ZIP archive
//! 3. Calculates SHA256 checksum of the archive
//! 4. Extracts version information from PE executables
//! 5. Checks GitHub for existing releases to determine if an update is needed
//! 6. Sets GitHub Actions outputs based on the update status
//!
//! ## Modules
//!
//! - [`actions`] - GitHub Actions output handling
//! - [`config`] - Configuration management for OSRS repositories
//! - [`downloader`] - File downloading and extraction logic
//! - [`file_ops`] - File operations (ZIP creation, checksums)
//! - [`github`] - GitHub API integration
//! - [`version`] - PE executable version extraction

use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use clap::Parser;
use log::LevelFilter;
use simple_logger::SimpleLogger;

use crate::actions::{ActionOutput, log_release_decision, set_github_actions_output};
use crate::downloader::download;
use crate::file_ops::{calculate_checksum, safe_remove_file, zip_directory};
use crate::github::{create_github_client, should_create_release};
use crate::version::extract_versions_from_directory;

pub mod actions;
pub mod config;
pub mod downloader;
pub mod file_ops;
pub mod github;
pub mod version;

/// Command line arguments for the OSRS Archive Release Updater
#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Repository identifier (e.g., "osrs-win", "osrs3-win")
    #[arg(long, default_value = "osrs-win")]
    repo: String,

    /// Build identifier (e.g., "production", "beta")
    #[arg(long, default_value = "production")]
    build: String,

    /// Name for the generated ZIP artifact
    #[arg(long, default_value = "osrs-win.production.zip")]
    artifact_name: String,

    /// Directory to download files to and create artifacts in
    #[arg(long, default_value = "downloads/")]
    output_dir: String,

    /// GitHub personal access token for API access
    #[arg(long, env = "GITHUB_TOKEN")]
    github_token: String,

    /// GitHub repository owner (username or organization)
    #[arg(long, default_value = "cozmoe0")]
    github_owner: String,

    /// GitHub repository name
    #[arg(long, default_value = "osrs-archive")]
    github_repo: String
}

#[tokio::main]
async fn main() -> Result<()> {
    init_logging()?;
    
    let args = Args::parse();
    let result = run_application(args).await;
    
    if let Err(ref e) = result {
        log::error!("Application failed: {:?}", e);
        std::process::exit(1);
    }
    
    result
}

/// Initializes logging with colors and info level
fn init_logging() -> Result<()> {
    SimpleLogger::new()
        .with_colors(true)
        .with_level(LevelFilter::Info)
        .init()
        .context("Failed to initialize logging")
}

/// Main application logic
async fn run_application(args: Args) -> Result<()> {
    let output_dir = PathBuf::from(&args.output_dir);
    
    // Download and package files
    let artifact_path = download_files(&args.repo, &args.build, &output_dir, &args.artifact_name).await?;
    log::info!("Created artifact: {}", artifact_path.display());

    // Calculate checksum and extract version
    let checksum = calculate_checksum(&artifact_path).await?;
    log::info!("Calculated artifact checksum: {}", checksum);

    let version = extract_versions_from_directory(&output_dir)?;
    log::info!("Extracted artifact version: {}", version);

    // Check if we should create a release
    let github = create_github_client(&args.github_token)?;
    let release_check = should_create_release(
        &github,
        &args.github_owner,
        &args.github_repo,
        &version,
        &checksum
    ).await?;

    // Set GitHub Actions output and clean up if needed
    if release_check.should_create {
        let output = ActionOutput::update_available(version.clone(), checksum, &artifact_path);
        set_github_actions_output(&output)?;
        log_release_decision(true, &release_check.reason, &version);
    } else {
        let output = ActionOutput::no_update();
        set_github_actions_output(&output)?;
        log_release_decision(false, &release_check.reason, &version);
        
        // Clean up artifact file since no release will be created
        safe_remove_file(&artifact_path).await;
    }

    Ok(())
}

/// Downloads and packages files into a ZIP archive
/// 
/// # Arguments
/// 
/// * `repo` - Repository identifier (e.g., "osrs-win")
/// * `build` - Build identifier (e.g., "production")
/// * `output_dir` - Directory to download files to
/// * `artifact_name` - Name of the resulting ZIP archive
/// 
/// # Returns
/// 
/// Returns the path to the created ZIP archive.
async fn download_files(repo: &str, build: &str, output_dir: &Path, artifact_name: &str) -> Result<PathBuf> {
    log::info!("Downloading files for {}.{}", repo, build);
    download(repo, build, &output_dir.to_path_buf()).await
        .context("Failed to download files")?;

    log::info!("Compressing files into artifact archive...");
    let artifact_path = output_dir.join(artifact_name);
    zip_directory(output_dir, &artifact_path)
        .context("Failed to create ZIP archive")?;
    
    log::info!("Successfully created artifact archive: {}", artifact_name);
    Ok(artifact_path)
}

