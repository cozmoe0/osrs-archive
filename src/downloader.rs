use std::fs;
use std::fs::create_dir_all;
use std::io::Read;
use std::path::{Path, PathBuf};
use anyhow::{bail, Context, Result};
use base64::Engine;
use base64::engine::general_purpose;
use flate2::read::GzDecoder;
use futures_util::future::try_join_all;
use reqwest::{Client, Url};
use sha2::{Digest, Sha256};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use crate::config::{Config, MetafileEntry};

const BASE_DOWNLOAD_URL: &str = "https://jagex.akamaized.net/direct6";
const MAX_CONCURRENT_DOWNLOADS: usize = 8;
const HTTP_TIMEOUT_SECS: u64 = 300;

/// A downloader that handles OSRS client archive downloads
/// 
/// The `Downloader` struct encapsulates all the functionality needed to download,
/// decompress, verify, and extract OSRS client files from Jagex's CDN.
/// 
/// # Example
/// 
/// ```rust,no_run
/// use std::path::PathBuf;
/// use osrs_archive::downloader::Downloader;
/// 
/// # async fn example() -> anyhow::Result<()> {
/// let output_dir = PathBuf::from("./downloads");
/// let downloader = Downloader::new("osrs".to_string(), output_dir)?;
/// downloader.download_build("live").await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Downloader {
    http_client: Client,
    repo: String,
    output_dir: PathBuf,
}

impl Downloader {
    /// Creates a new downloader instance
    /// 
    /// # Arguments
    /// 
    /// * `repo` - The repository name (e.g., "osrs", "osrs3")  
    /// * `output_dir` - The directory where downloaded files will be extracted
    /// 
    /// # Errors
    /// 
    /// Returns an error if the HTTP client cannot be configured.
    pub fn new(repo: String, output_dir: PathBuf) -> Result<Self> {
        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
            .pool_max_idle_per_host(MAX_CONCURRENT_DOWNLOADS)
            .user_agent("osrs-archive/1.0")
            .build()
            .context("Failed to create HTTP client")?;
            
        Ok(Self {
            http_client,
            repo,
            output_dir,
        })
    }

    /// Downloads and extracts a client build
    /// 
    /// This is the main entry point for downloading a complete client build.
    /// It handles the entire process from configuration loading to final cleanup.
    /// 
    /// # Arguments
    /// 
    /// * `build` - The build identifier (e.g., "live", "beta")
    /// 
    /// # Errors
    /// 
    /// Returns an error if any step of the download process fails, including:
    /// - Configuration loading
    /// - Directory creation
    /// - Piece downloads
    /// - File extraction
    /// - Cleanup operations
    pub async fn download_build(&self, build: &str) -> Result<()> {
        log::info!("Downloading client {}.{}...", self.repo, build);

        let mut config = Config::new(&self.repo, build);
        config.load_all().await
            .context("Failed to load configuration")?;
        log::info!("Loaded remote config data.");

        self.ensure_output_directory()
            .context("Failed to create output directory")?;

        let piece_urls = self.generate_piece_urls(&config.metafile.pieces)
            .context("Failed to generate piece URLs")?;
        log::info!("Found {} pieces to download.", piece_urls.len());

        self.download_and_process_pieces(&piece_urls).await
            .context("Failed to download pieces")?;

        let combined_path = self.combine_piece_files(&piece_urls).await
            .context("Failed to combine piece files")?;

        self.extract_files_from_archive(&combined_path, &config.metafile.files).await
            .context("Failed to extract files")?;

        self.cleanup_temporary_files(&combined_path).await
            .context("Failed to cleanup temporary files")?;

        log::info!("Download complete!");
        Ok(())
    }

    /// Ensures the output directory exists, creating it if necessary
    fn ensure_output_directory(&self) -> Result<()> {
        if !self.output_dir.exists() {
            fs::create_dir_all(&self.output_dir)
                .with_context(|| format!("Failed to create directory: {}", self.output_dir.display()))?;
            log::warn!("Created missing output directory: {}", self.output_dir.display());
        }
        Ok(())
    }

    /// Generates piece URLs from digest strings
    fn generate_piece_urls(&self, pieces: &[String]) -> Result<Vec<Url>> {
        pieces
            .iter()
            .map(|digest| {
                let digest_bytes = general_purpose::STANDARD.decode(digest.as_str())
                    .with_context(|| format!("Failed to decode base64 digest: {}", digest))?;
                let digest_hex_str = hex::encode(&digest_bytes);

                let piece_url = format!(
                    "{BASE_DOWNLOAD_URL}/{}/pieces/{}/{}.solidpiece",
                    self.repo,
                    digest_hex_str.get(0..2)
                        .ok_or_else(|| anyhow::anyhow!("Invalid digest hex string"))?,
                    digest_hex_str
                );
                
                piece_url.parse()
                    .with_context(|| format!("Failed to parse piece URL: {}", piece_url))
            })
            .collect()
    }

    /// Downloads and processes all piece files concurrently
    async fn download_and_process_pieces(&self, piece_urls: &[Url]) -> Result<()> {
        // Process downloads in batches to avoid overwhelming the server
        for chunk in piece_urls.chunks(MAX_CONCURRENT_DOWNLOADS) {
            let futures: Vec<_> = chunk
                .iter()
                .map(|url| self.download_and_process_single_piece(url))
                .collect();
            
            try_join_all(futures).await
                .context("Failed to download piece batch")?;
        }
        
        Ok(())
    }

    /// Downloads and processes a single piece file
    async fn download_and_process_single_piece(&self, piece_url: &Url) -> Result<()> {
        let file_name = Self::extract_filename_from_url(piece_url);
        log::info!("Downloading piece: {}", file_name);

        let response = self.http_client
            .get(piece_url.clone())
            .send()
            .await
            .with_context(|| format!("Failed to send request to {}", piece_url))?;

        if !response.status().is_success() {
            bail!("HTTP error {}: {}", response.status(), piece_url);
        }

        let bytes = response.bytes().await
            .context("Failed to read response bytes")?;

        log::info!("Decompressing piece: {}", file_name);
        let decompressed_bytes = Self::decompress_piece_data(&bytes, piece_url)?;

        self.write_piece_to_file(&decompressed_bytes, &file_name).await?;

        Self::verify_piece_checksum(&decompressed_bytes, &file_name, piece_url)?;
        log::info!("Checksum for file {} verified!", file_name);

        Ok(())
    }

    /// Extracts filename from piece URL
    fn extract_filename_from_url(piece_url: &Url) -> String {
        piece_url.path_segments()
            .and_then(|segments| segments.last())
            .unwrap_or("unknown")
            .to_string()
    }

    /// Decompresses piece data using gzip
    fn decompress_piece_data(bytes: &[u8], piece_url: &Url) -> Result<Vec<u8>> {
        if bytes.len() < 6 {
            bail!("Invalid piece data: too short");
        }

        let bytes = &bytes[6..]; // Skip first 6 bytes
        let mut decoder = GzDecoder::new(bytes);
        let mut decompressed_bytes = Vec::new();
        
        decoder.read_to_end(&mut decompressed_bytes)
            .with_context(|| format!("Failed to decompress piece data for {}", piece_url))?;
        
        Ok(decompressed_bytes)
    }

    /// Writes piece data to file
    async fn write_piece_to_file(&self, data: &[u8], file_name: &str) -> Result<()> {
        let file_path = self.output_dir.join(file_name);
        if file_path.exists() {
            tokio::fs::remove_file(&file_path).await
                .with_context(|| format!("Failed to remove existing file: {}", file_path.display()))?;
        }

        let mut file = File::create(&file_path).await
            .with_context(|| format!("Failed to create file: {}", file_path.display()))?;
        file.write_all(data).await
            .with_context(|| format!("Failed to write to file: {}", file_path.display()))?;
        
        Ok(())
    }

    /// Verifies piece checksum
    fn verify_piece_checksum(data: &[u8], file_name: &str, piece_url: &Url) -> Result<()> {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let checksum = format!("{:x}", hasher.finalize());

        let expected_digest = piece_url.path_segments()
            .and_then(|segments| segments.last())
            .ok_or_else(|| anyhow::anyhow!("Invalid piece URL: no filename"))?
            .strip_suffix(".solidpiece")
            .ok_or_else(|| anyhow::anyhow!("Invalid piece filename: missing .solidpiece extension"))?;
            
        if checksum != expected_digest {
            bail!("Checksum mismatch for {}! Expected {}, got {}", 
                  file_name, expected_digest, checksum);
        }
        
        Ok(())
    }

    /// Combines all piece files into a single file
    async fn combine_piece_files(&self, piece_urls: &[Url]) -> Result<PathBuf> {
        log::info!("Combining piece files...");

        let combined_path = self.output_dir.join("combined_file");
        if combined_path.exists() {
            tokio::fs::remove_file(&combined_path).await
                .context("Failed to remove existing combined file")?;
        }
        
        let mut combined_file = File::create(&combined_path).await
            .context("Failed to create combined file")?;

        let pieces_paths: Vec<PathBuf> = piece_urls
            .iter()
            .map(|url| self.output_dir.join(Self::extract_filename_from_url(url)))
            .collect();

        let mut total_size = 0;
        for piece_path in pieces_paths {
            let mut piece_file = File::open(&piece_path).await
                .with_context(|| format!("Failed to open piece file: {}", piece_path.display()))?;
            let mut bytes = Vec::new();
            piece_file.read_to_end(&mut bytes).await
                .with_context(|| format!("Failed to read piece file: {}", piece_path.display()))?;
            total_size += bytes.len();
            combined_file.write_all(&bytes).await
                .context("Failed to write to combined file")?;
        }
        
        combined_file.sync_all().await
            .context("Failed to sync combined file")?;
        drop(combined_file);
        log::info!("Combined file created ({}MB)", total_size / 1024 / 1024);

        Ok(combined_path)
    }

    /// Extracts individual files from the combined archive
    async fn extract_files_from_archive(
        &self,
        combined_path: &Path, 
        file_list: &[MetafileEntry]
    ) -> Result<()> {
        let mut source_file = File::open(combined_path).await
            .with_context(|| format!("Failed to open combined file: {}", combined_path.display()))?;

        for file in file_list.iter() {
            let file_name = &file.name;
            let file_size = file.size as usize;

            let mut file_output = vec![0u8; file_size];
            source_file.read_exact(&mut file_output).await
                .with_context(|| format!("Failed to read {} bytes for file: {}", file_size, file_name))?;

            let output_file_path = self.output_dir.join(file_name);
            if let Some(parent) = output_file_path.parent() {
                create_dir_all(parent)
                    .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
            }

            let mut output = File::create(&output_file_path).await
                .with_context(|| format!("Failed to create output file: {}", output_file_path.display()))?;
            output.write_all(&file_output).await
                .with_context(|| format!("Failed to write output file: {}", output_file_path.display()))?;

            log::info!("File {} extracted from combined file.", file_name);
        }

        Ok(())
    }

    /// Cleans up temporary files created during the download process
    async fn cleanup_temporary_files(&self, combined_path: &Path) -> Result<()> {
        log::info!("Cleaning up files...");

        let mut cleaned_count = 0;
        
        // Remove all .solidpiece files
        let mut entries = fs::read_dir(&self.output_dir)
            .with_context(|| format!("Failed to read output directory: {}", self.output_dir.display()))?;

        while let Some(entry) = entries.next().transpose()? {
            let path = entry.path();
            if let Some(extension) = path.extension() {
                if extension == "solidpiece" {
                    fs::remove_file(&path)
                        .with_context(|| format!("Failed to remove file: {}", path.display()))?;
                    cleaned_count += 1;
                }
            }
        }

        // Remove the combined file
        if combined_path.exists() {
            fs::remove_file(combined_path)
                .with_context(|| format!("Failed to remove combined file: {}", combined_path.display()))?;
            cleaned_count += 1;
        }

        log::debug!("Cleaned up {} temporary files.", cleaned_count);
        Ok(())
    }
}

/// Convenience function that maintains backwards compatibility
pub async fn download(repo: &str, build: &str, output_dir: &PathBuf) -> Result<()> {
    let downloader = Downloader::new(repo.to_string(), output_dir.clone())?;
    downloader.download_build(build).await
}