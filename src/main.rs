use std::path::PathBuf;
use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use futures_util::StreamExt;
use log::{info};
use octocrab::models::ReleaseId;
use octocrab::Octocrab;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long, default_value = "http://oldschool1.runescape.com/client.jar")]
    download_url: String,

    #[arg(long, default_value = "client.jar")]
    artifact_name: String,

    #[arg(long, env = "GITHUB_TOKEN")]
    github_token: String,

    #[arg(long, env = "GITHUB_REPOSITORY")]
    github_repository: String,

    #[arg(long, default_value = "hash")]
    version_method: String,

    #[arg(long, default_value = "v")]
    version_prefix: String,

    #[arg(long, default_value = ".")]
    output_dir: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();

    let (owner, repo) = args.github_repository.split_once("/")
        .ok_or_else(|| anyhow!("Invalid GITHUB_REPOSITORY format."))?;

    let download_path = download_file(&args.download_url, &args.output_dir, &args.artifact_name).await?;
    info!("Downloaded file to: {:?}", download_path);

    let checksum = calculate_checksum(&download_path).await?;
    info!("Calculated checksum (SHA-256): {}", checksum);

    let version = extract_version(
        &download_path,
        &args.version_method,
        &args.version_prefix,
        &args.download_url,
        &checksum
    ).await?;
    info!("Detected version: {}", version);

    let github = Octocrab::builder()
        .personal_token(args.github_token.clone())
        .build()?;

    let should_create_release = should_create_new_release(
        &github,
        owner,
        repo,
        &version,
        &checksum
    ).await?;

    if should_create_release {
        info!("Update detected - setting outputs for GitHub Actions.");

        println!("::set-output name=update_available::true");
        println!("::set-output name=version::{}", version);
        println!("::set-output name=checksum::{}", checksum);
        println!("::set-output name=artifact_path::{}", download_path.display());

        if let Ok(output_file) = std::env::var("GITHUB_OUTPUT") {
            let output = format!(
                "update_available=true\nversion={}\nchecksum={}\nartifact_path={}\n",
                version,
                checksum,
                download_path.display()
            );
            fs::write(output_file, output).await?;
        }

        info!("Release will be created by GitHub Actions!");
    } else {
        info!("No update needed - latest release is already at version {}", version);

        println!("::set-output name=update_available::false");

        if let Ok(output_file) = std::env::var("GITHUB_OUTPUT") {
            fs::write(output_file, "update-available=false\n").await?;
        }

        if let Err(e) = fs::remove_file(&download_path).await {
            log::warn!("Failed to clean up downloaded file: {}", e);
        }
    }

    Ok(())
}

async fn download_file(url: &str, output_dir: &str, filename: &str) -> Result<PathBuf> {
    info!("Downloading file from: {}", url);

    let client = reqwest::Client::new();
    let res = client.get(url)
        .send()
        .await
        .context("Failed to send HTTP request.")?;

    let status = res.status();
    if !status.is_success() {
        bail!("Download failed with the status: {}", status);
    }

    let output_path = PathBuf::from(output_dir).join(filename);
    let mut file = File::create(&output_path)
        .await
        .context("Failed to create output file")?;

    let mut stream = res.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Failed to download file chunk")?;
        file.write_all(&chunk)
            .await
            .context("Failed to write file chunk")?;
    }

    Ok(output_path)
}

async fn calculate_checksum(path: &PathBuf) -> Result<String> {
    let file_data = tokio::fs::read(path)
        .await
        .context("Failed to read file for checksum calculation")?;

    let mut hasher = Sha256::new();
    hasher.update(&file_data);
    let hash = hex::encode(hasher.finalize());

    Ok(hash)
}

async fn extract_version(
    path: &PathBuf,
    method: &str,
    prefix: &str,
    url: &str,
    checksum: &str
) -> Result<String> {
    match method {
        "hash" => Ok("232".to_string()),
        _ => {
            bail!("Unknown version extraction method.")
        }
    }
}

async fn should_create_new_release(
    github: &Octocrab,
    owner: &str,
    repo: &str,
    version: &str,
    checksum: &str
) -> Result<bool> {
    let latest_release_result = github
        .repos(owner, repo)
        .releases()
        .get_latest()
        .await;

    match latest_release_result {
        Ok(release) => {
            info!("Latest GitHub release tag: {}", release.tag_name);

            if release.tag_name != version {
                info!("Version different than latest Github release. ({} vs {})", version, release.tag_name);
                return Ok(true);
            }

            if let Some(body) = release.body {
                if body.contains(checksum) {
                    info!("Checksum found in the release body, file is identical");
                    return Ok(false);
                }
            }

            let assets = github
                .repos(owner.to_string(), repo.to_string())
                .releases()
                .get_latest()
                .await?
                .assets;

            if assets.is_empty() {
                info!("Latest release has no assets. Creating new release.");
                return Ok(true);
            }

            info!("Latest release has the same version but different content (checksum), create new release.");
            return Ok(true);
        },
        Err(err) => {
            log::warn!("Failed to get the latest release, assuming first release: {}", err);
            return Ok(true);
        }
    }
}