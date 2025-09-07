use std::fs;
use std::path::Path;
use anyhow::{Context, Result};
use pelite::FileMap;
use pelite::pe32::Pe as Pe32;
use pelite::pe64::Pe as Pe64;
use pelite::resources::version_info::Language;

const DEFAULT_VERSION: &str = "NONE";

/// Represents version information extracted from a PE executable
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
    /// Creates a new empty `ExecutableVersionInfo`
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

impl Default for ExecutableVersionInfo {
    fn default() -> Self {
        Self::new()
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

/// Extracts version information from a PE executable file
/// 
/// # Arguments
/// 
/// * `file_path` - Path to the PE file (.exe or .dll)
/// 
/// # Returns
/// 
/// Returns `Ok(Some(ExecutableVersionInfo))` if version info is found,
/// `Ok(None)` if no version info is available, or an error if the file
/// cannot be read or is not a valid PE file.
pub fn extract_version_info(file_path: &Path) -> Result<Option<ExecutableVersionInfo>> {
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
        .unwrap_or_default();
    
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

/// Scans a directory for executable files and extracts version information
/// 
/// # Arguments
/// 
/// * `dir` - Directory path to scan for .exe and .dll files
/// 
/// # Returns
/// 
/// Returns the first file version found, or "NONE" if no version information is available.
pub fn extract_versions_from_directory(dir: &Path) -> Result<String> {
    log::info!("Scanning directory for executable files: {}", dir.display());
    
    let entries = fs::read_dir(dir)
        .with_context(|| format!("Failed to read directory: {}", dir.display()))?;
    
    for entry in entries {
        let entry = entry.with_context(|| format!("Failed to read directory entry in: {}", dir.display()))?;
        let path = entry.path();
        
        if path.is_file() {
            if let Some(extension) = path.extension() {
                if extension.eq_ignore_ascii_case("exe") || extension.eq_ignore_ascii_case("dll") {
                    if let Ok(Some(version_info)) = extract_version_info(&path) {
                        if let Some(file_version) = version_info.file_version {
                            log::info!("Found version {} in {}", file_version, 
                                     path.file_name().unwrap_or_default().to_string_lossy());
                            return Ok(file_version);
                        }
                    }
                    // Continue searching if this file had no version info
                    log::debug!("No version info found in {}", 
                               path.file_name().unwrap_or_default().to_string_lossy());
                }
            }
        }
    }
    
    log::warn!("No version information found in any executable files");
    Ok(DEFAULT_VERSION.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_executable_version_info_new() {
        let info = ExecutableVersionInfo::new();
        assert!(info.file_version.is_none());
        assert!(info.product_version.is_none());
    }

    #[test]
    fn test_executable_version_info_default() {
        let info = ExecutableVersionInfo::default();
        assert!(info.file_version.is_none());
        assert!(info.product_version.is_none());
    }

    #[test]
    fn test_extract_versions_from_nonexistent_directory() {
        let result = extract_versions_from_directory(&PathBuf::from("nonexistent"));
        assert!(result.is_err());
    }
}