use anyhow::{Context, Result};
use std::path::Path;
use std::{env, fs};

/// Represents the output data for GitHub Actions
#[derive(Debug, Clone)]
pub struct ActionOutput {
    pub update_available: bool,
    pub version: String,
    pub checksum: String,
    pub artifact_path: String,
}

impl ActionOutput {
    /// Creates a new ActionOutput for when an update is available
    pub fn update_available(version: String, checksum: String, artifact_path: &Path) -> Self {
        Self {
            update_available: true,
            version,
            checksum,
            artifact_path: artifact_path.display().to_string(),
        }
    }

    /// Creates a new ActionOutput for when no update is available
    pub fn no_update() -> Self {
        Self {
            update_available: false,
            version: String::new(),
            checksum: String::new(),
            artifact_path: String::new(),
        }
    }
}

/// Sets GitHub Actions output using both the deprecated ::set-output format
/// and the new GITHUB_OUTPUT environment file format
///
/// # Arguments
///
/// * `output` - The output data to set
///
/// # Returns
///
/// Returns `Ok(())` on success, or an error if writing to GITHUB_OUTPUT fails.
///
/// # Notes
///
/// This function uses both output methods for compatibility:
/// - Prints to stdout using the deprecated ::set-output format
/// - Writes to $GITHUB_OUTPUT file if the environment variable is set
pub fn set_github_actions_output(output: &ActionOutput) -> Result<()> {
    if output.update_available {
        // Set output using deprecated format (for older runners)
        println!("::set-output name=update_available::true");
        println!("::set-output name=version::{}", output.version);
        println!("::set-output name=checksum::{}", output.checksum);
        println!("::set-output name=artifact_path::{}", output.artifact_path);

        // Set output using new format (for newer runners)
        if let Ok(output_file) = env::var("GITHUB_OUTPUT") {
            let content = format!(
                "update_available=true\nversion={}\nchecksum={}\nartifact_path={}\n",
                output.version, output.checksum, output.artifact_path
            );
            fs::write(&output_file, content).with_context(|| {
                format!("Failed to write to GITHUB_OUTPUT file: {}", output_file)
            })?;
            log::debug!("Wrote output to GITHUB_OUTPUT file: {}", output_file);
        }

        log::info!(
            "Set GitHub Actions output: update available, version={}, checksum={}",
            output.version,
            output.checksum
        );
    } else {
        // Set output using deprecated format
        println!("::set-output name=update_available::false");

        // Set output using new format
        if let Ok(output_file) = env::var("GITHUB_OUTPUT") {
            fs::write(&output_file, "update_available=false\n").with_context(|| {
                format!("Failed to write to GITHUB_OUTPUT file: {}", output_file)
            })?;
            log::debug!("Wrote output to GITHUB_OUTPUT file: {}", output_file);
        }

        log::info!("Set GitHub Actions output: no update available");
    }

    Ok(())
}

/// Logs the reason for the release decision
///
/// # Arguments
///
/// * `should_create` - Whether a release should be created
/// * `reason` - The reason for the decision
/// * `version` - The current version
pub fn log_release_decision(should_create: bool, reason: &str, version: &str) {
    if should_create {
        log::info!("Update detected - {}", reason);
        log::info!(
            "Release will be created by GitHub Actions for version {}",
            version
        );
    } else {
        log::info!("No update detected - {}", reason);
        log::info!("Latest release is already at version {}", version);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn test_action_output_update_available() {
        let output = ActionOutput::update_available(
            "1.0.0".to_string(),
            "abc123".to_string(),
            &PathBuf::from("/path/to/artifact"),
        );

        assert!(output.update_available);
        assert_eq!(output.version, "1.0.0");
        assert_eq!(output.checksum, "abc123");
        assert_eq!(output.artifact_path, "/path/to/artifact");
    }

    #[test]
    fn test_action_output_no_update() {
        let output = ActionOutput::no_update();

        assert!(!output.update_available);
        assert_eq!(output.version, "");
        assert_eq!(output.checksum, "");
        assert_eq!(output.artifact_path, "");
    }

    #[test]
    fn test_set_github_actions_output_no_env() {
        // Should not fail even without GITHUB_OUTPUT env var
        let output = ActionOutput::no_update();
        let result = set_github_actions_output(&output);
        assert!(result.is_ok());
    }

    #[test]
    fn test_set_github_actions_output_with_env() {
        let temp_dir = tempdir().unwrap();
        let output_file = temp_dir.path().join("github_output");

        // Set the environment variable
        env::set_var("GITHUB_OUTPUT", output_file.to_str().unwrap());

        let output = ActionOutput::update_available(
            "1.0.0".to_string(),
            "abc123".to_string(),
            &PathBuf::from("/test"),
        );

        let result = set_github_actions_output(&output);
        assert!(result.is_ok());

        // Check that file was created and has content
        assert!(output_file.exists());
        let content = fs::read_to_string(&output_file).unwrap();
        assert!(content.contains("update_available=true"));
        assert!(content.contains("version=1.0.0"));

        // Clean up
        env::remove_var("GITHUB_OUTPUT");
    }

    #[test]
    fn test_log_release_decision() {
        // This function only logs, so we just test it doesn't panic
        log_release_decision(true, "Version changed", "1.0.0");
        log_release_decision(false, "No changes", "1.0.0");
    }
}
