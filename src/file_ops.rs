use std::{fs, io};
use std::fs::File;
use std::path::Path;
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

/// Compresses a directory into a ZIP archive
/// 
/// # Arguments
/// 
/// * `src_dir` - Source directory to compress
/// * `zip_file_path` - Output path for the ZIP file
/// 
/// # Returns
/// 
/// Returns `Ok(())` on success, or an IO error if compression fails.
/// 
/// # Notes
/// 
/// - Skips existing ZIP files to avoid recursive compression
/// - Preserves directory structure
/// - Uses Deflate compression with Unix permissions
pub fn zip_directory(src_dir: &Path, zip_file_path: &Path) -> Result<()> {
    let file = File::create(zip_file_path)
        .with_context(|| format!("Failed to create ZIP file: {}", zip_file_path.display()))?;
    let mut zip = ZipWriter::new(file);

    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o755);

    let entries = fs::read_dir(src_dir)
        .with_context(|| format!("Failed to read source directory: {}", src_dir.display()))?;

    for entry in entries {
        let entry = entry.with_context(|| format!("Failed to read directory entry in: {}", src_dir.display()))?;
        let path = entry.path();
        
        // Get the relative path name for the ZIP entry
        let name = path.strip_prefix(src_dir)
            .with_context(|| format!("Failed to get relative path for: {}", path.display()))?
            .to_str()
            .with_context(|| format!("Path contains invalid UTF-8: {}", path.display()))?;

        // Skip ZIP files to avoid recursive compression
        if let Some(extension) = path.extension() {
            if extension.eq_ignore_ascii_case("zip") {
                log::debug!("Skipping ZIP file: {}", name);
                continue;
            }
        }

        if path.is_file() {
            zip.start_file(name, options)
                .with_context(|| format!("Failed to start ZIP entry for: {}", name))?;
            let mut f = File::open(&path)
                .with_context(|| format!("Failed to open file for compression: {}", path.display()))?;
            io::copy(&mut f, &mut zip)
                .with_context(|| format!("Failed to compress file: {}", path.display()))?;
            log::debug!("Compressed file: {}", name);
        } else if path.is_dir() {
            zip.add_directory(name, options)
                .with_context(|| format!("Failed to add directory to ZIP: {}", name))?;
            log::debug!("Added directory: {}", name);
        }
    }

    zip.finish()
        .context("Failed to finalize ZIP archive")?;
    
    log::info!("Successfully created ZIP archive: {}", zip_file_path.display());
    Ok(())
}

/// Calculates the SHA256 checksum of a file
/// 
/// # Arguments
/// 
/// * `file_path` - Path to the file to checksum
/// 
/// # Returns
/// 
/// Returns the hexadecimal representation of the SHA256 hash.
pub async fn calculate_checksum(file_path: &Path) -> Result<String> {
    let file_data = tokio::fs::read(file_path)
        .await
        .with_context(|| format!("Failed to read file for checksum: {}", file_path.display()))?;

    let mut hasher = Sha256::new();
    hasher.update(&file_data);
    let checksum = hex::encode(hasher.finalize());

    log::debug!("Calculated checksum for {}: {}", file_path.display(), checksum);
    Ok(checksum)
}

/// Safely removes a file, logging any errors but not failing
/// 
/// # Arguments
/// 
/// * `file_path` - Path to the file to remove
pub async fn safe_remove_file(file_path: &Path) {
    if let Err(e) = tokio::fs::remove_file(file_path).await {
        log::warn!("Failed to remove file {}: {}", file_path.display(), e);
    } else {
        log::debug!("Successfully removed file: {}", file_path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn test_zip_nonexistent_directory() {
        let result = zip_directory(
            &PathBuf::from("nonexistent"),
            &PathBuf::from("test.zip")
        );
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_calculate_checksum_nonexistent_file() {
        let result = calculate_checksum(&PathBuf::from("nonexistent.txt")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_safe_remove_nonexistent_file() {
        // This should not panic or error
        safe_remove_file(&PathBuf::from("nonexistent.txt")).await;
    }

    #[tokio::test]
    async fn test_calculate_checksum_empty_file() {
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("empty.txt");
        
        // Create empty file
        tokio::fs::write(&file_path, "").await.unwrap();
        
        let checksum = calculate_checksum(&file_path).await.unwrap();
        // SHA256 of empty string
        assert_eq!(checksum, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }
}