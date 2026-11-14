//! Enforced skills: Neuron's own rules that always apply.
//!
//! Unlike the bundled default skills ([`crate::default_skills`]), an enforced
//! skill stays on when `"defaultSkills": false` and wins over a project or user
//! skill of the same name (`/skills` warns about the ignored one). Today the set
//! is compiled in; [`active_enforced_skills`] is the one place a later, verified
//! source (such as a signed gateway manifest) would add to it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use sha2::{Digest, Sha256};

use crate::config::RuntimeConfig;

/// One enforced skill: its name, the one-line summary the system prompt
/// carries, and its files (relative path and contents, `SKILL.md` first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnforcedSkill {
    pub name: String,
    pub summary: String,
    pub files: Vec<(String, String)>,
}

/// The enforced skills built into this binary.
#[must_use]
pub fn compiled_enforced_skills() -> Vec<EnforcedSkill> {
    vec![EnforcedSkill {
        name: "reading-documents".to_string(),
        summary: "read Office and PDF files with read_document, never with scripts".to_string(),
        files: vec![(
            "SKILL.md".to_string(),
            include_str!("../enforced-skills/reading-documents/SKILL.md").to_string(),
        )],
    }]
}

/// The enforced skills in effect.
#[must_use]
pub fn active_enforced_skills() -> Vec<EnforcedSkill> {
    compiled_enforced_skills()
}

/// Whether `name` (any case) is an enforced skill.
#[must_use]
pub fn is_enforced_skill(name: &str) -> bool {
    active_enforced_skills()
        .iter()
        .any(|skill| skill.name.eq_ignore_ascii_case(name.trim()))
}

/// Writes `skills` under `config_home/enforced-skills/<content hash>/<name>/`
/// so skill discovery and the Skill tool read them like any skill folder.
pub fn enforced_skill_root_for(
    config_home: &Path,
    skills: &[EnforcedSkill],
) -> io::Result<PathBuf> {
    let mut digest = Sha256::new();
    for skill in skills {
        for (path, contents) in &skill.files {
            digest.update(skill.name.as_bytes());
            digest.update(path.as_bytes());
            digest.update(contents.as_bytes());
        }
    }
    let root = config_home
        .join("enforced-skills")
        .join(format!("{:x}", digest.finalize()));
    write_skill_files(&root, skills)?;
    Ok(root)
}

/// Writes each skill's files under `root/<name>/`, leaving files that
/// already hold the right contents alone.
fn write_skill_files(root: &Path, skills: &[EnforcedSkill]) -> io::Result<()> {
    for skill in skills {
        for (relative, contents) in &skill.files {
            let target = root.join(&skill.name).join(relative);
            if fs::read_to_string(&target).ok().as_deref() != Some(contents.as_str()) {
                fs::create_dir_all(target.parent().expect("skill files have parents"))?;
                fs::write(target, contents)?;
            }
        }
    }
    Ok(())
}

/// [`enforced_skill_root_for`] with the active enforced skills. When
/// `config_home` cannot be written, the files go into a private folder this
/// process creates under the temp folder ([`private_fallback_root`]), so an
/// enforced skill still wins over a project skill of its name.
pub fn enforced_skill_root(config_home: &Path) -> io::Result<PathBuf> {
    let skills = active_enforced_skills();
    enforced_skill_root_for(config_home, &skills)
        .or_else(|error| private_fallback_root(&skills).map_err(|_| error))
}

/// The fallback folder for enforced skill files: created by this process
/// under an unguessable name (owner-only on Unix), never a shared or
/// predictable path another user could fill first, and reused for the rest
/// of the process while it is still a folder only its owner can reach.
fn private_fallback_root(skills: &[EnforcedSkill]) -> io::Result<PathBuf> {
    static ROOT: Mutex<Option<PathBuf>> = Mutex::new(None);
    let mut root = ROOT.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(existing) = root.as_ref() {
        if is_private_dir(existing) && write_skill_files(existing, skills).is_ok() {
            return Ok(existing.clone());
        }
    }
    let created = create_private_dir()?;
    write_skill_files(&created, skills)?;
    *root = Some(created.clone());
    Ok(created)
}

/// A new, empty folder in the temp folder, made by this call: creation
/// fails rather than reuse a path that already exists.
fn create_private_dir() -> io::Result<PathBuf> {
    let base = std::env::temp_dir();
    for _ in 0..16 {
        let path = base.join(format!(
            "neuron-enforced-{}-{:016x}",
            std::process::id(),
            rand::random::<u64>()
        ));
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not create a private folder for the enforced skills",
    ))
}

/// Whether `path` is still a real folder (not a link) that, on Unix, only
/// its owner can reach.
fn is_private_dir(path: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return false;
        }
    }
    metadata.is_dir()
}

/// The system prompt section naming the enforced skills, one short line each.
#[must_use]
pub fn enforced_skill_section(skills: &[EnforcedSkill]) -> Option<String> {
    if skills.is_empty() {
        return None;
    }
    let index = skills
        .iter()
        .map(|skill| format!("- {}: {}", skill.name, skill.summary))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "# Neuron enforced skills\nAlways follow these; the Skill tool returns the full text.\n{index}"
    ))
}

/// Every skill section of the system prompt: the enforced skills, which no
/// setting turns off, then the bundled defaults unless `"defaultSkills": false`.
/// Writing the enforced skill files is best effort: the section itself needs
/// no file, so a config home that cannot be written does not stop the prompt.
pub fn skill_prompt_sections(
    config: &RuntimeConfig,
    config_home: &Path,
) -> io::Result<Vec<String>> {
    let skills = active_enforced_skills();
    let _ = enforced_skill_root(config_home);
    let mut sections = enforced_skill_section(&skills)
        .into_iter()
        .collect::<Vec<_>>();
    sections.extend(crate::default_skills::default_skill_sections(
        config,
        config_home,
    )?);
    Ok(sections)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigLoader;

    fn temp_root(label: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "neuron-enforced-{label}-{}-{unique}",
            std::process::id()
        ))
    }

    #[test]
    fn enforced_skills_stay_when_default_skills_are_off() {
        let root = temp_root("defaults-off");
        let config_home = root.join("config");
        fs::create_dir_all(&config_home).unwrap();
        fs::write(
            config_home.join("settings.json"),
            r#"{"defaultSkills":false}"#,
        )
        .unwrap();
        let loader = ConfigLoader::new(&root, &config_home);
        let sections = skill_prompt_sections(&loader.load().unwrap(), &config_home).unwrap();
        assert_eq!(sections.len(), 1, "only the enforced section: {sections:?}");
        assert!(sections[0].starts_with("# Neuron enforced skills"));
        assert!(sections[0].contains("- reading-documents: "));
        assert!(sections[0].contains("read_document"));
        assert!(!sections[0].contains("# Neuron default skills"));
        let skill_file = enforced_skill_root(&config_home)
            .unwrap()
            .join("reading-documents")
            .join("SKILL.md");
        assert!(fs::read_to_string(skill_file)
            .unwrap()
            .contains("name: reading-documents"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn enforced_skills_come_before_the_defaults() {
        let root = temp_root("defaults-on");
        let config_home = root.join("config");
        fs::create_dir_all(&config_home).unwrap();
        let loader = ConfigLoader::new(&root, &config_home);
        let sections = skill_prompt_sections(&loader.load().unwrap(), &config_home).unwrap();
        assert_eq!(sections.len(), 2);
        assert!(sections[0].starts_with("# Neuron enforced skills"));
        assert!(sections[1].starts_with("# Neuron default skills"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn enforced_index_and_body_stay_short() {
        for skill in compiled_enforced_skills() {
            let line = format!("- {}: {}", skill.name, skill.summary);
            // About four characters a token: under ~25 tokens.
            assert!(line.chars().count() <= 100, "{line}");
            let (path, body) = &skill.files[0];
            assert_eq!(path, "SKILL.md");
            assert!(body.contains(&format!("name: {}", skill.name)));
            assert!(body.contains("license: MIT"));
            assert!(body.chars().count() < 1_500, "{} is long", skill.name);
            assert!(!body.contains('\u{2014}'), "no em dashes");
        }
        assert!(is_enforced_skill("Reading-Documents"));
        assert!(!is_enforced_skill("ponytail"));
        let section = enforced_skill_section(&compiled_enforced_skills()).unwrap();
        assert!(section.chars().count() < 250, "{section}");
    }

    #[test]
    fn an_unwritable_config_home_does_not_stop_the_prompt() {
        let root = temp_root("unwritable");
        fs::create_dir_all(&root).unwrap();
        // A file where the config folder should be: nothing can be written
        // under it.
        let config_home = root.join("config");
        fs::write(&config_home, "not a folder").unwrap();
        // Bundled skills off: the case that used to write nothing at all.
        let settings_home = root.join("settings");
        fs::create_dir_all(&settings_home).unwrap();
        fs::write(
            settings_home.join("settings.json"),
            r#"{"defaultSkills":false}"#,
        )
        .unwrap();
        let loader = ConfigLoader::new(&root, &settings_home);
        let sections = skill_prompt_sections(&loader.load().unwrap(), &config_home)
            .expect("the prompt still builds");
        assert!(sections[0].starts_with("# Neuron enforced skills"));
        // The skill files land in the temp folder instead.
        let fallback = enforced_skill_root(&config_home).expect("temp fallback");
        assert!(fallback
            .join("reading-documents")
            .join("SKILL.md")
            .is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_temp_fallback_is_a_private_folder_no_one_else_can_prepare() {
        let root = temp_root("private-fallback");
        fs::create_dir_all(&root).unwrap();
        let config_home = root.join("config");
        fs::write(&config_home, "not a folder").unwrap();
        // The old fallback was temp/neuron-<user>/enforced-skills/<hash>:
        // another user could create it first and add skills of their own.
        let user = std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_default();
        let predictable = std::env::temp_dir().join(format!("neuron-{user}"));
        let fallback = enforced_skill_root(&config_home).expect("temp fallback");
        assert!(
            !fallback.starts_with(&predictable),
            "{}",
            fallback.display()
        );
        let name = fallback.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("neuron-enforced-"), "{name}");
        assert!(name.len() > "neuron-enforced-".len() + 16, "{name}");
        assert!(is_private_dir(&fallback));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&fallback).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        // Reused for the rest of the process (other tests share it, so its
        // files are left in place).
        assert_eq!(enforced_skill_root(&config_home).unwrap(), fallback);
        // A folder that already exists is never taken over.
        let another = create_private_dir().unwrap();
        assert_ne!(another, fallback);
        fs::remove_dir_all(another).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_empty_enforced_set_adds_no_section() {
        assert_eq!(enforced_skill_section(&[]), None);
    }
}
