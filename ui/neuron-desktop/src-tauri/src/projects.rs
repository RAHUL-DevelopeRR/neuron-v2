//! Projects opened before, newest first, kept in the app's config folder.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::workspace::Workspace;

const RECENT_LIMIT: usize = 12;
const FILE: &str = "recent-projects.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    pub path: String,
}

impl Project {
    pub fn of(workspace: &Workspace) -> Self {
        Self {
            name: workspace.name(),
            path: workspace.display_root(),
        }
    }
}

fn store(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|dir| dir.join(FILE))
}

pub fn recent(app: &AppHandle) -> Vec<Project> {
    store(app).map(|path| read(&path)).unwrap_or_default()
}

/// Puts `project` first; a failed write only loses the list's order.
pub fn remember(app: &AppHandle, project: &Project) {
    if let Some(path) = store(app) {
        let _ = write(&path, &with_first(read(&path), project));
    }
}

fn read(path: &Path) -> Vec<Project> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write(path: &Path, projects: &[Project]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(
        path,
        serde_json::to_string_pretty(projects).unwrap_or_else(|_| "[]".into()),
    )
}

/// `project` first, without its older entry, at most [`RECENT_LIMIT`] long.
fn with_first(mut projects: Vec<Project>, project: &Project) -> Vec<Project> {
    projects.retain(|existing| !existing.path.eq_ignore_ascii_case(&project.path));
    projects.insert(0, project.clone());
    projects.truncate(RECENT_LIMIT);
    projects
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(path: &str) -> Project {
        Project {
            name: path.rsplit(['/', '\\']).next().unwrap().to_string(),
            path: path.to_string(),
        }
    }

    #[test]
    fn the_newest_first_once_and_a_short_list() {
        let list = with_first(vec![project("C:/a"), project("C:/b")], &project("c:/B"));
        assert_eq!(
            list.iter().map(|p| p.path.as_str()).collect::<Vec<_>>(),
            ["c:/B", "C:/a"]
        );
        let long = (0..20).fold(Vec::new(), |list, i| {
            with_first(list, &project(&format!("/p{i}")))
        });
        assert_eq!(long.len(), RECENT_LIMIT);
        assert_eq!(long[0].path, "/p19");

        let dir =
            std::env::temp_dir().join(format!("neuron-desktop-recent-{}", std::process::id()));
        let file = dir.join(FILE);
        write(&file, &long).unwrap();
        assert_eq!(read(&file), long);
        let _ = fs::remove_dir_all(dir);
        assert!(read(Path::new("no/such/file.json")).is_empty());
    }
}
