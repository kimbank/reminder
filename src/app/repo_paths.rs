use std::{collections::BTreeMap, fs};

pub(super) fn canonical_repo_key(repo: &str) -> Option<String> {
    let parts: Vec<_> = repo.trim().split('/').collect();
    if parts.len() != 2 || parts.iter().any(|part| part.is_empty()) {
        return None;
    }

    Some(format!(
        "{}/{}",
        parts[0].to_lowercase(),
        parts[1].to_lowercase()
    ))
}

pub(super) fn normalize_hydrated_repo_paths(
    repo_paths: BTreeMap<String, String>,
) -> (BTreeMap<String, String>, usize) {
    let mut normalized = BTreeMap::new();
    let mut dropped = 0;

    for (repo, path) in repo_paths {
        let Some(repo_key) = canonical_repo_key(&repo) else {
            dropped += 1;
            continue;
        };

        let Ok(canonical_path) = fs::canonicalize(&path) else {
            dropped += 1;
            continue;
        };

        if !canonical_path.is_dir() || !canonical_path.join(".git").exists() {
            dropped += 1;
            continue;
        }

        normalized.insert(repo_key, canonical_path.display().to_string());
    }

    (normalized, dropped)
}
