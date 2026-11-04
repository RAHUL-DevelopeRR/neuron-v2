//! The bundled browser: the Playwright MCP server Neuron ships as `playwright`.
//!
//! - How it is launched ([`apply_launch_settings`]): an in-memory profile
//!   (`--isolated`), automatically named files under `<project>/.neuron/browser`
//!   (`--output-dir`), headless when there is no display, in CI or when
//!   `browser.headless` is true, screenshots left out of tool results until
//!   tool results can carry images (`browser.images`), and local and private
//!   addresses blocked unless `browser.allowLocal` is true.
//! - The permission tier of each of its tools ([`BUNDLED_BROWSER_TOOLS`]),
//!   which replaces the server's own annotations (every tool says it reaches
//!   the open world, which would make each call need full access).
//! - Whether a one-shot run (`neuron -p`) loads it: only with `--with-browser`
//!   or `browser.oneShot` ([`one_shot_loads_bundled`]).
//!
//! The flags were checked against @playwright/mcp 0.0.83 (its README and
//! `cli.js`). Its origin lists are a convenience, not a security boundary:
//! the server applies them to the requests a page makes, but not to
//! redirects, and a public name that resolves to a private address is not
//! caught.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::{ConfigError, ConfigSource, McpServerConfig, McpStdioServerConfig};
use crate::json::JsonValue;
use crate::{PermissionMode, RuntimeConfig};

/// The bundled browser's server name.
pub const BROWSER_SERVER: &str = "playwright";

/// The prefix of the bundled browser's qualified tool names.
pub const BROWSER_TOOL_PREFIX: &str = "mcp__playwright__";

/// Where the browser's automatically named files go, under the project.
pub const BROWSER_OUTPUT_DIR: &str = ".neuron/browser";

/// The server's environment variable for blocked origins. The list goes in
/// the environment rather than the arguments: on Windows the server starts
/// through `cmd.exe /c npx`, which must not see its `;` and `*` characters.
pub const BLOCKED_ORIGINS_ENV: &str = "PLAYWRIGHT_MCP_BLOCKED_ORIGINS";

/// Local, private, link-local (cloud metadata) and unique-local addresses, as
/// @playwright/mcp origin patterns: a value that is not a URL becomes the
/// glob `*://<value>/**`, where `*` matches anything but `/` (so a port is
/// covered too).
pub const LOCAL_BLOCKED_ORIGINS: &[&str] = &[
    "localhost*",
    "*.localhost*",
    "127.*",
    "0.*",
    "10.*",
    "172.16.*",
    "172.17.*",
    "172.18.*",
    "172.19.*",
    "172.20.*",
    "172.21.*",
    "172.22.*",
    "172.23.*",
    "172.24.*",
    "172.25.*",
    "172.26.*",
    "172.27.*",
    "172.28.*",
    "172.29.*",
    "172.30.*",
    "172.31.*",
    "192.168.*",
    "169.254.*",
    "[::]*",
    "[::1]*",
    "[::ffff:*",
    "[fc*",
    "[fd*",
    "[fe80:*",
];

/// Whether screenshots and other images reach the model (`browser.images`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BrowserImages {
    /// Left out of tool results (`--image-responses omit`). The default until
    /// tool results carry images: today a screenshot would arrive as tens of
    /// thousands of characters of base64 text.
    #[default]
    Omit,
    /// Sent as the server returns them.
    Allow,
}

impl BrowserImages {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Omit => "omit",
            Self::Allow => "allow",
        }
    }
}

/// The `browser` settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BrowserSettings {
    /// `browser.headless`: `true` always runs headless. Without it (or with
    /// `false`) the browser is headed only when there is a display and the
    /// run is not in CI.
    pub headless: bool,
    /// `browser.images`.
    pub images: BrowserImages,
    /// `browser.allowLocal`: let the browser open localhost and private
    /// addresses (testing one's own app).
    pub allow_local: bool,
    /// `browser.oneShot`: load the browser in one-shot runs too, as
    /// `--with-browser` does.
    pub one_shot: bool,
}

impl BrowserSettings {
    /// Reads the `browser` object of merged settings; absent keys keep their
    /// defaults.
    pub fn from_settings(merged: &BTreeMap<String, JsonValue>) -> Result<Self, ConfigError> {
        let mut settings = Self::default();
        let Some(value) = merged.get("browser") else {
            return Ok(settings);
        };
        let object = value.as_object().ok_or_else(|| {
            ConfigError::Parse("merged settings: field browser must be an object".to_string())
        })?;
        let flag = |key: &str| -> Result<Option<bool>, ConfigError> {
            object.get(key).map_or(Ok(None), |value| {
                value.as_bool().map(Some).ok_or_else(|| {
                    ConfigError::Parse(format!(
                        "merged settings: field browser.{key} must be a boolean"
                    ))
                })
            })
        };
        settings.headless = flag("headless")?.unwrap_or(false);
        settings.allow_local = flag("allowLocal")?.unwrap_or(false);
        settings.one_shot = flag("oneShot")?.unwrap_or(false);
        if let Some(images) = object.get("images") {
            settings.images = match images.as_str() {
                Some("omit") => BrowserImages::Omit,
                Some("allow") => BrowserImages::Allow,
                _ => {
                    return Err(ConfigError::Parse(
                        "merged settings: field browser.images must be \"omit\" or \"allow\""
                            .to_string(),
                    ))
                }
            };
        }
        Ok(settings)
    }

    /// The settings of a loaded configuration; the defaults when they do not
    /// parse (loading reported that already).
    #[must_use]
    pub fn of(config: &RuntimeConfig) -> Self {
        Self::from_settings(config.merged()).unwrap_or_default()
    }
}

/// What the machine offers the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowserHost {
    /// A display a headed browser can open on.
    pub display: bool,
    /// A CI run (`CI` is set and not `false` or `0`).
    pub ci: bool,
}

impl BrowserHost {
    /// This process's machine: Windows and macOS have a display; elsewhere
    /// `DISPLAY` or `WAYLAND_DISPLAY` must be set.
    #[must_use]
    pub fn detect() -> Self {
        let set = |name: &str| {
            std::env::var(name)
                .ok()
                .is_some_and(|value| !value.trim().is_empty())
        };
        let display =
            cfg!(any(windows, target_os = "macos")) || set("DISPLAY") || set("WAYLAND_DISPLAY");
        let ci = std::env::var("CI").ok().is_some_and(|value| {
            let value = value.trim().to_ascii_lowercase();
            !value.is_empty() && value != "false" && value != "0"
        });
        Self { display, ci }
    }
}

/// Whether the browser runs headless: with no display, in CI, or when the
/// settings ask for it.
#[must_use]
pub fn runs_headless(settings: &BrowserSettings, host: BrowserHost) -> bool {
    settings.headless || !host.display || host.ci
}

/// The browser's output folder for a project.
#[must_use]
pub fn output_dir(project: &Path) -> PathBuf {
    project.join(BROWSER_OUTPUT_DIR)
}

/// Adds the safe launch flags to the bundled browser's start command.
pub fn apply_launch_settings(
    server: &mut McpStdioServerConfig,
    settings: &BrowserSettings,
    project: &Path,
    host: BrowserHost,
) {
    server.args.push("--isolated".to_string());
    server.args.push("--output-dir".to_string());
    server
        .args
        .push(output_dir(project).to_string_lossy().into_owned());
    if runs_headless(settings, host) {
        server.args.push("--headless".to_string());
    }
    if settings.images == BrowserImages::Omit {
        server.args.push("--image-responses".to_string());
        server.args.push(BrowserImages::Omit.as_str().to_string());
    }
    if !settings.allow_local {
        server.env.insert(
            BLOCKED_ORIGINS_ENV.to_string(),
            LOCAL_BLOCKED_ORIGINS.join(";"),
        );
    }
}

/// Creates the bundled browser's output folder with a `.gitignore` that
/// keeps it out of the project's history. Does nothing when the browser is
/// not the bundled one or has no `--output-dir`.
pub fn prepare_output_dir(config: &RuntimeConfig) -> std::io::Result<()> {
    let Some(server) = config.mcp().get(BROWSER_SERVER) else {
        return Ok(());
    };
    if server.scope != ConfigSource::Bundled {
        return Ok(());
    }
    let McpServerConfig::Stdio(stdio) = &server.config else {
        return Ok(());
    };
    let Some(dir) = stdio
        .args
        .iter()
        .position(|arg| arg == "--output-dir")
        .and_then(|index| stdio.args.get(index + 1))
    else {
        return Ok(());
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    let gitignore = dir.join(".gitignore");
    if !gitignore.exists() {
        std::fs::write(gitignore, "*\n")?;
    }
    Ok(())
}

/// How much a bundled-browser tool can do, which decides its permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserToolTier {
    /// Reads or arranges the pages already open: workspace-write, no
    /// question.
    Page,
    /// Opens web pages or acts on them: workspace-write, plus one approval
    /// of web browsing per session (or the run's `--allow-origin` hosts).
    Web,
    /// Sends local files to a page, returns raw request and response data
    /// (headers and cookies included) or runs code in the server's process:
    /// full access.
    FullAccess,
}

/// Every tool of @playwright/mcp 0.0.83 (25, as its server lists them) and
/// its tier. A tool missing here (a later release) keeps what its own
/// annotations ask for, which is full access for all of them today.
pub const BUNDLED_BROWSER_TOOLS: &[(&str, BrowserToolTier)] = &[
    ("browser_snapshot", BrowserToolTier::Page),
    ("browser_take_screenshot", BrowserToolTier::Page),
    ("browser_console_messages", BrowserToolTier::Page),
    ("browser_find", BrowserToolTier::Page),
    ("browser_wait_for", BrowserToolTier::Page),
    // URLs only; the full request and response is browser_network_request.
    ("browser_network_requests", BrowserToolTier::Page),
    // `new` with a URL opens a page: see [`browser_call_tier`].
    ("browser_tabs", BrowserToolTier::Page),
    ("browser_resize", BrowserToolTier::Page),
    ("browser_emulate_media", BrowserToolTier::Page),
    ("browser_close", BrowserToolTier::Page),
    ("browser_navigate", BrowserToolTier::Web),
    ("browser_navigate_back", BrowserToolTier::Web),
    ("browser_click", BrowserToolTier::Web),
    ("browser_type", BrowserToolTier::Web),
    ("browser_fill_form", BrowserToolTier::Web),
    ("browser_select_option", BrowserToolTier::Web),
    ("browser_press_key", BrowserToolTier::Web),
    ("browser_hover", BrowserToolTier::Web),
    ("browser_drag", BrowserToolTier::Web),
    ("browser_handle_dialog", BrowserToolTier::Web),
    // Runs in the page, which the browser isolates; it can still navigate.
    ("browser_evaluate", BrowserToolTier::Web),
    ("browser_network_request", BrowserToolTier::FullAccess),
    ("browser_file_upload", BrowserToolTier::FullAccess),
    ("browser_drop", BrowserToolTier::FullAccess),
    // Runs JavaScript in the server's own Node process: the server calls it
    // "RCE-equivalent".
    ("browser_run_code_unsafe", BrowserToolTier::FullAccess),
];

/// The tier of a bundled-browser tool by its own name (`browser_click`).
#[must_use]
pub fn browser_tool_tier(tool: &str) -> Option<BrowserToolTier> {
    BUNDLED_BROWSER_TOOLS
        .iter()
        .find(|(name, _)| *name == tool)
        .map(|(_, tier)| *tier)
}

/// The permission a bundled-browser tool needs before its input is looked
/// at; `None` for a tool not in [`BUNDLED_BROWSER_TOOLS`].
#[must_use]
pub fn browser_tool_permission(tool: &str) -> Option<PermissionMode> {
    browser_tool_tier(tool).map(|tier| match tier {
        BrowserToolTier::Page | BrowserToolTier::Web => PermissionMode::WorkspaceWrite,
        BrowserToolTier::FullAccess => PermissionMode::DangerFullAccess,
    })
}

/// The tier of one call by its qualified name (`mcp__playwright__browser_tabs`)
/// and JSON input: `browser_tabs` opening a tab on a URL is web browsing, a
/// blank new tab or any other tab action is not. `None` for a name that is
/// not a known bundled-browser tool.
#[must_use]
pub fn browser_call_tier(qualified_name: &str, input: &str) -> Option<BrowserToolTier> {
    let tool = qualified_name.strip_prefix(BROWSER_TOOL_PREFIX)?;
    let tier = browser_tool_tier(tool)?;
    if tool != "browser_tabs" {
        return Some(tier);
    }
    let Ok(serde_json::Value::Object(object)) = serde_json::from_str::<serde_json::Value>(input)
    else {
        return Some(tier);
    };
    let opens_url = object.get("action").and_then(serde_json::Value::as_str) == Some("new")
        && object
            .get("url")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|url| !url.trim().is_empty() && url.trim() != "about:blank");
    Some(if opens_url {
        BrowserToolTier::Web
    } else {
        tier
    })
}

static ONE_SHOT_RUN: AtomicBool = AtomicBool::new(false);

/// Marks this process as a one-shot run (`neuron -p`), which leaves out the
/// bundled MCP servers unless asked for ([`one_shot_loads_bundled`]).
pub fn set_one_shot_run(one_shot: bool) {
    ONE_SHOT_RUN.store(one_shot, Ordering::Relaxed);
}

/// Whether this process is a one-shot run.
#[must_use]
pub fn is_one_shot_run() -> bool {
    ONE_SHOT_RUN.load(Ordering::Relaxed)
}

/// Whether a one-shot run loads the bundled server `name`: only the browser,
/// and only with `--with-browser` (this run's
/// [`crate::RunWebGrants::with_browser`]) or `browser.oneShot`.
#[must_use]
pub fn one_shot_loads_bundled(name: &str, config: &RuntimeConfig) -> bool {
    one_shot_loads_bundled_with(name, config, crate::run_web_grants().with_browser)
}

/// [`one_shot_loads_bundled`] with `--with-browser` given or not.
#[must_use]
pub fn one_shot_loads_bundled_with(name: &str, config: &RuntimeConfig, with_browser: bool) -> bool {
    name == BROWSER_SERVER && (with_browser || BrowserSettings::of(config).one_shot)
}

/// Whether the bundled server `name` is configured but not loaded: a
/// one-shot run (`one_shot`) that did not ask for it.
#[must_use]
pub fn bundled_server_left_out(
    name: &str,
    config: &RuntimeConfig,
    one_shot: bool,
    with_browser: bool,
) -> bool {
    one_shot
        && config
            .mcp()
            .get(name)
            .is_some_and(|server| server.scope == ConfigSource::Bundled)
        && !one_shot_loads_bundled_with(name, config, with_browser)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(json: &str) -> Result<BrowserSettings, ConfigError> {
        let parsed = crate::json::JsonValue::parse(json).expect("json");
        BrowserSettings::from_settings(parsed.as_object().expect("object"))
    }

    fn launch(settings: BrowserSettings, host: BrowserHost) -> McpStdioServerConfig {
        let mut server = McpStdioServerConfig {
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "@playwright/mcp@0.0.83".to_string()],
            env: BTreeMap::new(),
            tool_call_timeout_ms: None,
        };
        apply_launch_settings(&mut server, &settings, Path::new("/work/app"), host);
        server
    }

    const DESKTOP: BrowserHost = BrowserHost {
        display: true,
        ci: false,
    };

    #[test]
    fn settings_default_to_omitted_images_and_blocked_local_addresses() {
        assert_eq!(settings("{}").unwrap(), BrowserSettings::default());
        let parsed = settings(
            r#"{"browser":{"headless":true,"images":"allow","allowLocal":true,"oneShot":true}}"#,
        )
        .unwrap();
        assert_eq!(
            parsed,
            BrowserSettings {
                headless: true,
                images: BrowserImages::Allow,
                allow_local: true,
                one_shot: true,
            }
        );
        let error = settings(r#"{"browser":{"images":"auto"}}"#).unwrap_err();
        assert!(error.to_string().contains("browser.images"), "{error}");
        let error = settings(r#"{"browser":{"allowLocal":"yes"}}"#).unwrap_err();
        assert!(error.to_string().contains("browser.allowLocal"), "{error}");
        assert!(settings(r#"{"browser":true}"#).is_err());
    }

    #[test]
    fn launch_is_isolated_with_an_output_folder_and_no_images_by_default() {
        let server = launch(BrowserSettings::default(), DESKTOP);
        let args = server.args.join(" ");
        assert!(server
            .args
            .starts_with(&["-y".to_string(), "@playwright/mcp@0.0.83".to_string()]));
        assert!(server.args.contains(&"--isolated".to_string()), "{args}");
        let dir = server
            .args
            .iter()
            .position(|arg| arg == "--output-dir")
            .map(|index| PathBuf::from(&server.args[index + 1]))
            .expect("--output-dir");
        assert_eq!(dir, Path::new("/work/app").join(".neuron").join("browser"));
        assert!(args.contains("--image-responses omit"), "{args}");
        // A display and no CI: headed.
        assert!(!server.args.contains(&"--headless".to_string()), "{args}");
        // Never a persistent profile, a secrets file or unrestricted files.
        for flag in [
            "--user-data-dir",
            "--secrets",
            "--allow-unrestricted-file-access",
            "--extension",
        ] {
            assert!(!args.contains(flag), "{args}");
        }
        let blocked = server
            .env
            .get(BLOCKED_ORIGINS_ENV)
            .expect("blocked origins");
        for origin in [
            "localhost*",
            "127.*",
            "10.*",
            "192.168.*",
            "169.254.*",
            "[::1]*",
        ] {
            assert!(blocked.split(';').any(|item| item == origin), "{blocked}");
        }
    }

    #[test]
    fn headless_without_a_display_in_ci_or_when_asked() {
        let defaults = BrowserSettings::default();
        let has = |server: &McpStdioServerConfig| server.args.contains(&"--headless".to_string());
        assert!(!has(&launch(defaults, DESKTOP)));
        assert!(has(&launch(
            defaults,
            BrowserHost {
                display: false,
                ci: false
            }
        )));
        assert!(has(&launch(
            defaults,
            BrowserHost {
                display: true,
                ci: true
            }
        )));
        let asked = BrowserSettings {
            headless: true,
            ..defaults
        };
        assert!(has(&launch(asked, DESKTOP)));
    }

    #[test]
    fn allowed_images_and_local_addresses_drop_their_flags() {
        let server = launch(
            BrowserSettings {
                images: BrowserImages::Allow,
                allow_local: true,
                ..BrowserSettings::default()
            },
            DESKTOP,
        );
        assert!(!server.args.contains(&"--image-responses".to_string()));
        assert!(!server.env.contains_key(BLOCKED_ORIGINS_ENV));
        assert!(server.args.contains(&"--isolated".to_string()));
    }

    /// The tools @playwright/mcp 0.0.83 lists (a stdio probe of the server).
    const PROBED_TOOLS: [&str; 25] = [
        "browser_click",
        "browser_close",
        "browser_console_messages",
        "browser_drag",
        "browser_drop",
        "browser_emulate_media",
        "browser_evaluate",
        "browser_file_upload",
        "browser_fill_form",
        "browser_find",
        "browser_handle_dialog",
        "browser_hover",
        "browser_navigate",
        "browser_navigate_back",
        "browser_network_request",
        "browser_network_requests",
        "browser_press_key",
        "browser_resize",
        "browser_run_code_unsafe",
        "browser_select_option",
        "browser_snapshot",
        "browser_tabs",
        "browser_take_screenshot",
        "browser_type",
        "browser_wait_for",
    ];

    #[test]
    fn the_tier_table_covers_exactly_the_servers_tools() {
        let mut table: Vec<&str> = BUNDLED_BROWSER_TOOLS
            .iter()
            .map(|(name, _)| *name)
            .collect();
        table.sort_unstable();
        assert_eq!(table, PROBED_TOOLS);
        for tool in PROBED_TOOLS {
            assert!(browser_tool_permission(tool).is_some(), "{tool}");
        }
        assert_eq!(browser_tool_permission("browser_future_tool"), None);
    }

    #[test]
    fn only_file_network_detail_and_server_code_tools_need_full_access() {
        let full: Vec<&str> = PROBED_TOOLS
            .into_iter()
            .filter(|tool| browser_tool_permission(tool) == Some(PermissionMode::DangerFullAccess))
            .collect();
        assert_eq!(
            full,
            [
                "browser_drop",
                "browser_file_upload",
                "browser_network_request",
                "browser_run_code_unsafe"
            ]
        );
        for tool in [
            "browser_snapshot",
            "browser_navigate",
            "browser_click",
            "browser_evaluate",
        ] {
            assert_eq!(
                browser_tool_permission(tool),
                Some(PermissionMode::WorkspaceWrite),
                "{tool}"
            );
        }
    }

    #[test]
    fn a_new_tab_on_a_url_is_web_browsing_and_other_tab_actions_are_not() {
        let tier = |input: &str| browser_call_tier("mcp__playwright__browser_tabs", input);
        assert_eq!(
            tier(r#"{"action":"new","url":"https://a.test"}"#),
            Some(BrowserToolTier::Web)
        );
        assert_eq!(tier(r#"{"action":"new"}"#), Some(BrowserToolTier::Page));
        assert_eq!(
            tier(r#"{"action":"new","url":"about:blank"}"#),
            Some(BrowserToolTier::Page)
        );
        assert_eq!(tier(r#"{"action":"list"}"#), Some(BrowserToolTier::Page));
        assert_eq!(
            tier(r#"{"action":"close","index":1}"#),
            Some(BrowserToolTier::Page)
        );
        assert_eq!(
            browser_call_tier("mcp__playwright__browser_navigate", "{}"),
            Some(BrowserToolTier::Web)
        );
        assert_eq!(
            browser_call_tier("mcp__other__browser_navigate", "{}"),
            None
        );
        assert_eq!(
            browser_call_tier("mcp__playwright__browser_future", "{}"),
            None
        );
    }

    #[test]
    fn the_output_folder_is_made_with_a_gitignore_only_for_the_bundled_browser() {
        let root = std::env::temp_dir().join(format!(
            "neuron-browser-output-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos())
        ));
        let config_home = root.join("config");
        std::fs::create_dir_all(&config_home).unwrap();
        let loader = crate::ConfigLoader::new(&root, &config_home);
        prepare_output_dir(&loader.load().unwrap()).unwrap();
        let dir = root.join(".neuron").join("browser");
        assert_eq!(
            std::fs::read_to_string(dir.join(".gitignore")).unwrap(),
            "*
"
        );
        // A gitignore the user edited is kept.
        std::fs::write(
            dir.join(".gitignore"),
            "*
!keep.png
",
        )
        .unwrap();
        prepare_output_dir(&loader.load().unwrap()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join(".gitignore")).unwrap(),
            "*
!keep.png
"
        );
        std::fs::remove_dir_all(&dir).unwrap();

        // A browser server of the user's own: nothing is made.
        std::fs::write(
            config_home.join("settings.json"),
            r#"{"mcpServers":{"playwright":{"command":"my-browser","args":["--output-dir","elsewhere"]}}}"#,
        )
        .unwrap();
        prepare_output_dir(&loader.load().unwrap()).unwrap();
        assert!(!dir.exists());
        assert!(!root.join("elsewhere").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
