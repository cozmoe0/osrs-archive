use anyhow::{Context, Result};
use octocrab::Octocrab;

/// Represents the result of checking if a new release should be created
#[derive(Debug, Clone)]
pub struct ReleaseCheck {
    pub should_create: bool,
    pub reason: String,
}

/// Checks if a new GitHub release should be created based on version and checksum
///
/// # Arguments
///
/// * `github` - Authenticated GitHub client
/// * `owner` - Repository owner (username or organization)
/// * `repo` - Repository name
/// * `version` - Current version string
/// * `checksum` - Current artifact checksum
///
/// # Returns
///
/// Returns `ReleaseCheck` indicating whether a new release should be created and why.
pub async fn should_create_release(
    github: &Octocrab,
    owner: &str,
    repo: &str,
    version: &str,
    checksum: &str,
) -> Result<ReleaseCheck> {
    log::info!(
        "Checking if release should be created for version: {}",
        version
    );

    let latest_release = github.repos(owner, repo).releases().get_latest().await;

    match latest_release {
        Ok(release) => {
            log::info!("Latest release found: {}", release.tag_name);

            // Check if version is different
            if release.tag_name != version {
                let reason = format!("Version changed from {} to {}", release.tag_name, version);
                log::info!("{}", reason);
                return Ok(ReleaseCheck {
                    should_create: true,
                    reason,
                });
            }

            // Check if checksum is in release body (indicates same content)
            if let Some(body) = &release.body {
                if body.contains(checksum) {
                    let reason = "Checksum found in release body - no content changes".to_string();
                    log::info!("{}", reason);
                    return Ok(ReleaseCheck {
                        should_create: false,
                        reason,
                    });
                }
            }

            // Check if release has assets
            if release.assets.is_empty() {
                let reason = "No assets found in latest release".to_string();
                log::info!("{}", reason);
                return Ok(ReleaseCheck {
                    should_create: true,
                    reason,
                });
            }

            // Same version but different content (checksum not found in body)
            let reason = "Same version but content has changed (different checksum)".to_string();
            log::info!("{}", reason);
            Ok(ReleaseCheck {
                should_create: false,
                reason,
            })
        }
        Err(e) => {
            let reason = format!("No previous releases found or API error: {}", e);
            log::info!("{}", reason);
            Ok(ReleaseCheck {
                should_create: true,
                reason,
            })
        }
    }
}

/// Creates a GitHub client with the provided personal access token
///
/// # Arguments
///
/// * `token` - GitHub personal access token
///
/// # Returns
///
/// Returns an authenticated `Octocrab` client.
pub fn create_github_client(token: &str) -> Result<Octocrab> {
    Octocrab::builder()
        .personal_token(token)
        .build()
        .context("Failed to create GitHub client")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_release_check_creation() {
        let check = ReleaseCheck {
            should_create: true,
            reason: "Test reason".to_string(),
        };

        assert!(check.should_create);
        assert_eq!(check.reason, "Test reason");
    }

    #[tokio::test]
    async fn test_create_github_client() {
        let result = create_github_client("fake_token");
        assert!(result.is_ok());
    }
}
