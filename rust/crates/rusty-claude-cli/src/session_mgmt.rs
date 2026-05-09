//! Session management: CRUD operations for managed sessions.
//!
//! Extracted from the main module for maintainability.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use runtime::Session;

use crate::brand::*;
use crate::tool_ui::display_clean_path;

/// Lightweight session identifier + path pair.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub id: String,
    pub path: PathBuf,
}

/// Summary of a saved session for listing.
#[derive(Debug, Clone)]
pub struct ManagedSessionSummary {
    pub id: String,
    pub path: PathBuf,
    pub updated_at_ms: u64,
    pub modified_epoch_millis: u128,
    pub message_count: usize,
    pub parent_session_id: Option<String>,
    pub branch_name: Option<String>,
}

pub fn sessions_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(current_session_store()?.sessions_dir().to_path_buf())
}

pub fn current_session_store() -> Result<runtime::SessionStore, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    runtime::SessionStore::from_cwd(&cwd).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}

pub fn new_cli_session() -> Result<Session, Box<dyn std::error::Error>> {
    Ok(Session::new().with_workspace_root(env::current_dir()?))
}

pub fn create_managed_session_handle(
    session_id: &str,
) -> Result<SessionHandle, Box<dyn std::error::Error>> {
    let handle = current_session_store()?.create_handle(session_id);
    Ok(SessionHandle {
        id: handle.id,
        path: handle.path,
    })
}

pub fn resolve_session_reference(
    reference: &str,
) -> Result<SessionHandle, Box<dyn std::error::Error>> {
    let handle = current_session_store()?
        .resolve_reference(reference)
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    Ok(SessionHandle {
        id: handle.id,
        path: handle.path,
    })
}

pub fn resolve_managed_session_path(
    session_id: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    current_session_store()?
        .resolve_managed_path(session_id)
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}

pub fn list_managed_sessions() -> Result<Vec<ManagedSessionSummary>, Box<dyn std::error::Error>> {
    Ok(current_session_store()?
        .list_sessions()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?
        .into_iter()
        .map(|session| ManagedSessionSummary {
            id: session.id,
            path: session.path,
            updated_at_ms: session.updated_at_ms,
            modified_epoch_millis: session.modified_epoch_millis,
            message_count: session.message_count,
            parent_session_id: session.parent_session_id,
            branch_name: session.branch_name,
        })
        .collect())
}

pub fn latest_managed_session() -> Result<ManagedSessionSummary, Box<dyn std::error::Error>> {
    let session = current_session_store()?
        .latest_session()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    Ok(ManagedSessionSummary {
        id: session.id,
        path: session.path,
        updated_at_ms: session.updated_at_ms,
        modified_epoch_millis: session.modified_epoch_millis,
        message_count: session.message_count,
        parent_session_id: session.parent_session_id,
        branch_name: session.branch_name,
    })
}

pub fn load_session_reference(
    reference: &str,
) -> Result<(SessionHandle, Session), Box<dyn std::error::Error>> {
    let loaded = current_session_store()?
        .load_session(reference)
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    Ok((
        SessionHandle {
            id: loaded.handle.id,
            path: loaded.handle.path,
        },
        loaded.session,
    ))
}

pub fn delete_managed_session(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if !path.exists() {
        return Err(format!("session file does not exist: {}", path.display()).into());
    }
    fs::remove_file(path)?;
    Ok(())
}

pub fn confirm_session_deletion(session_id: &str) -> bool {
    print!("Delete session '{session_id}'? This cannot be undone. [y/N]: ");
    io::stdout().flush().unwrap_or(());
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim(), "y" | "Y" | "yes" | "Yes" | "YES")
}

pub fn render_session_list(active_session_id: &str) -> Result<String, Box<dyn std::error::Error>> {
    let sessions = list_managed_sessions()?;
    let w = 72;

    let mut lines = Vec::new();
    lines.push(box_top("Sessions", w));

    // Directory row
    let dir_display = sessions_dir()?.display().to_string();
    let dir_short = display_clean_path(&dir_display);
    lines.push(box_row(&format!("{DIM}Directory: {dir_short}{R}"), w));
    lines.push(box_separator(w));

    if sessions.is_empty() {
        lines.push(box_row(
            &format!("{DIM}No managed sessions saved yet.{R}"),
            w,
        ));
        lines.push(box_bottom(w));
        return Ok(lines.join("\n"));
    }

    // Header
    lines.push(box_row(
        &format!(
            "{BLUE}{BOLD}{id:<24} {status:<10} {msgs:<6} {modified}{R}",
            id = "SESSION",
            status = "STATUS",
            msgs = "MSGS",
            modified = "MODIFIED",
        ),
        w,
    ));
    lines.push(box_separator(w));

    // Session rows
    for session in &sessions {
        let (marker_icon, status_text) = if session.id == active_session_id {
            (ICON_ACTIVE, format!("{GREEN}current{R}"))
        } else {
            (ICON_INACTIVE, format!("{DIM}saved{R}"))
        };
        let id_display = if session.id.len() > 22 {
            format!("{}\u{2026}", &session.id[..21])
        } else {
            session.id.clone()
        };
        let modified = format_session_modified_age(session.modified_epoch_millis);
        let lineage = match (
            session.branch_name.as_deref(),
            session.parent_session_id.as_deref(),
        ) {
            (Some(branch), _) => format!(" {ORANGE}\u{23c7} {branch}{R}"),
            _ => String::new(),
        };

        let row_content = format!(
            "{marker_icon} {ORANGE}{id:<22}{R} {status:<10} {DIM}{msgs:<6}{R} {DIM}{modified}{R}{lineage}",
            id = id_display,
            status = status_text,
            msgs = session.message_count,
        );
        lines.push(box_row(&row_content, w));
    }

    lines.push(box_bottom(w));
    Ok(lines.join("\n"))
}

pub fn format_session_modified_age(modified_epoch_millis: u128) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map_or(modified_epoch_millis, |duration| duration.as_millis());
    let delta_seconds = now
        .saturating_sub(modified_epoch_millis)
        .checked_div(1_000)
        .unwrap_or_default();
    match delta_seconds {
        0..=4 => "just-now".to_string(),
        5..=59 => format!("{delta_seconds}s-ago"),
        60..=3_599 => format!("{}m-ago", delta_seconds / 60),
        3_600..=86_399 => format!("{}h-ago", delta_seconds / 3_600),
        _ => format!("{}d-ago", delta_seconds / 86_400),
    }
}

pub fn write_session_clear_backup(
    session: &Session,
    session_path: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let backup_path = session_clear_backup_path(session_path);
    session.save_to_path(&backup_path)?;
    Ok(backup_path)
}

pub fn session_clear_backup_path(session_path: &Path) -> PathBuf {
    let timestamp = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map_or(0, |duration| duration.as_millis());
    let file_name = session_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("session.jsonl");
    session_path.with_file_name(format!("{file_name}.before-clear-{timestamp}.bak"))
}
