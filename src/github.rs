// src/github.rs
//
// Fetches the latest stable release tags for Bitcoin Core and Electrs from
// the GitHub Releases API.  Release candidates (tags containing "rc") are
// filtered out, matching the Python implementation exactly.

use anyhow::{Context, Result};
use serde::Deserialize;

const BITCOIN_API: &str = "https://api.github.com/repos/bitcoin/bitcoin/releases";
const ELECTRS_API: &str = "https://api.github.com/repos/romanz/electrs/releases";
const MAX_VERSIONS: usize = 10;

// ─── GitHub API response shape ────────────────────────────────────────────────

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
}

// ─── Public fetch functions ───────────────────────────────────────────────────

/// Fetch up to 10 stable Bitcoin Core release tags from GitHub.
pub async fn fetch_bitcoin_versions() -> Result<Vec<String>> {
    fetch_versions(BITCOIN_API, "Bitcoin Core").await
}

/// Fetch up to 10 stable Electrs release tags from GitHub.
pub async fn fetch_electrs_versions() -> Result<Vec<String>> {
    fetch_versions(ELECTRS_API, "Electrs").await
}

// ─── Shared implementation ────────────────────────────────────────────────────

async fn fetch_versions(url: &str, project: &str) -> Result<Vec<String>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        // GitHub API requires a User-Agent header.
        .user_agent("bitcoin-compiler/0.1")
        .build()
        .context("Failed to build HTTP client")?;

    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("HTTP GET failed for {project} releases"))?;

    let response = response
        .error_for_status()
        .with_context(|| format!("GitHub API returned error status for {project}"))?;

    let releases: Vec<GitHubRelease> = response
        .json()
        .await
        .with_context(|| format!("Failed to parse {project} release JSON"))?;

    let versions: Vec<String> = releases
        .into_iter()
        .filter(|r| !r.tag_name.to_lowercase().contains("rc"))
        .map(|r| r.tag_name)
        .take(MAX_VERSIONS)
        .collect();

    Ok(versions)
}
