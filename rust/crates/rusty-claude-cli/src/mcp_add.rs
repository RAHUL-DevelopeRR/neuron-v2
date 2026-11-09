//! `neuron mcp add <id>` in a terminal: asks for the connector's variables (secrets with
//! the input hidden), adds it from the catalog and, for a connector that signs in with
//! OAuth, signs in. Outside a terminal, or with `--output-format json`, the variables
//! come from the environment as they do for `/mcp add` (see the `commands` crate).

use std::collections::BTreeMap;
use std::env;
use std::io::{self, BufRead, IsTerminal, Write};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal;
use runtime::{ConfigLoader, McpCatalog, McpCatalogEntry, McpCatalogVariable, McpInstallRequest};

/// `add ...` run interactively: `None` when this is not an `add` the terminal should
/// ask for (other arguments, an unknown id, no terminal), so the caller reports it.
pub(crate) fn handle_interactive_mcp_add(
    args: Option<&str>,
) -> Option<Result<String, Box<dyn std::error::Error>>> {
    let words = args?.split_whitespace().collect::<Vec<_>>();
    let (&"add", rest) = words.split_first()? else {
        return None;
    };
    let parsed = commands::parse_mcp_add_args(rest).ok()?;
    let entry = McpCatalog::builtin().get(&parsed.id)?;
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return None;
    }
    Some(add_interactively(entry, parsed))
}

fn add_interactively(
    entry: &McpCatalogEntry,
    parsed: commands::McpAddArgs,
) -> Result<String, Box<dyn std::error::Error>> {
    eprintln!(
        "Adding {} ({}, {}, {}).",
        entry.name,
        entry.pinned_label(),
        entry.publisher,
        entry.licence
    );
    let values = collect_values(entry, |name| env::var(name).ok(), ask)?;
    let cwd = env::current_dir()?;
    let loader = ConfigLoader::default_for(&cwd);
    let outcome = runtime::install_mcp_server(
        &loader,
        &McpInstallRequest {
            id: parsed.id,
            name: parsed.name,
            scope: parsed.scope,
            values,
        },
    );
    let mut report = commands::render_mcp_add_outcome(&outcome);
    if let Ok(outcome) = &outcome {
        if outcome.needs_login {
            match crate::mcp_auth::login(&outcome.name) {
                Ok(message) => report.push_str(&format!("\n{message}")),
                Err(error) => report.push_str(&format!(
                    "\nSign-in did not finish ({error}); run `neuron mcp login {}` to try again.",
                    outcome.name
                )),
            }
        }
    }
    Ok(report)
}

/// The values to add `entry` with: each variable from the environment when set there,
/// else from `ask` (`None` or empty: left out, so an optional one is skipped and a
/// required one is reported missing by the install).
fn collect_values(
    entry: &McpCatalogEntry,
    from_env: impl Fn(&str) -> Option<String>,
    mut ask: impl FnMut(&McpCatalogVariable) -> io::Result<Option<String>>,
) -> io::Result<BTreeMap<String, String>> {
    let mut values = BTreeMap::new();
    for variable in &entry.env {
        let value = match from_env(&variable.name).filter(|value| !value.is_empty()) {
            Some(value) => {
                eprintln!("  {} taken from the environment.", variable.name);
                Some(value)
            }
            None => ask(variable)?,
        };
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            values.insert(variable.name.clone(), value);
        }
    }
    Ok(values)
}

/// Asks for one variable on the terminal: a secret with the input hidden.
fn ask(variable: &McpCatalogVariable) -> io::Result<Option<String>> {
    let mut prompt = format!("  {}", variable.label);
    if let Some(url) = &variable.help_url {
        prompt.push_str(&format!(" ({url})"));
    }
    if let Some(default) = &variable.default {
        prompt.push_str(&format!(" [{default}]"));
    } else if !variable.required {
        prompt.push_str(" [Enter to skip]");
    }
    prompt.push_str(": ");
    let mut stderr = io::stderr();
    write!(stderr, "{prompt}")?;
    stderr.flush()?;
    let answer = if variable.secret {
        read_hidden_line()?
    } else {
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;
        Some(line.trim().to_string())
    };
    Ok(answer.filter(|answer| !answer.is_empty()))
}

/// Reads a line without echoing it. Ctrl-C or Esc cancels.
fn read_hidden_line() -> io::Result<Option<String>> {
    terminal::enable_raw_mode()?;
    let keys = std::iter::from_fn(|| match event::read() {
        Ok(Event::Key(key)) => Some(Ok(key)),
        Ok(_) => Some(Ok(KeyEvent::new(KeyCode::Null, KeyModifiers::NONE))),
        Err(error) => Some(Err(error)),
    });
    let result = collect_hidden(keys);
    let _ = terminal::disable_raw_mode();
    eprintln!();
    match result? {
        HiddenLine::Entered(text) => Ok(Some(text)),
        HiddenLine::Cancelled => Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "cancelled; nothing was added",
        )),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum HiddenLine {
    Entered(String),
    Cancelled,
}

/// The text typed before Enter: characters added, Backspace removes the last one, key
/// releases ignored. Ctrl-C and Esc cancel.
fn collect_hidden(keys: impl Iterator<Item = io::Result<KeyEvent>>) -> io::Result<HiddenLine> {
    let mut text = String::new();
    for key in keys {
        let key = key?;
        if key.kind == KeyEventKind::Release {
            continue;
        }
        match key.code {
            KeyCode::Enter => return Ok(HiddenLine::Entered(text)),
            KeyCode::Esc => return Ok(HiddenLine::Cancelled),
            KeyCode::Char('c' | 'C') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(HiddenLine::Cancelled)
            }
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Char(character) => text.push(character),
            _ => {}
        }
    }
    Ok(HiddenLine::Entered(text))
}

#[cfg(test)]
mod tests {
    use std::io;

    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    use super::{collect_hidden, collect_values, HiddenLine};

    fn key(code: KeyCode) -> io::Result<KeyEvent> {
        Ok(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn hidden_input_takes_characters_and_backspace_until_enter() {
        let mut release = KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        let keys = vec![
            key(KeyCode::Char('a')),
            key(KeyCode::Char('b')),
            key(KeyCode::Backspace),
            Ok(release),
            key(KeyCode::Char('c')),
            key(KeyCode::Enter),
            key(KeyCode::Char('x')),
        ];
        assert_eq!(
            collect_hidden(keys.into_iter()).unwrap(),
            HiddenLine::Entered("ac".to_string())
        );
        let cancel = vec![
            key(KeyCode::Char('a')),
            Ok(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        ];
        assert_eq!(
            collect_hidden(cancel.into_iter()).unwrap(),
            HiddenLine::Cancelled
        );
    }

    #[test]
    fn values_come_from_the_environment_first_and_skipped_answers_are_left_out() {
        let catalog = runtime::McpCatalog::builtin();
        let github = catalog.get("github").unwrap();
        let mut asked = Vec::new();
        let values = collect_values(
            github,
            |_| None,
            |variable| {
                asked.push((variable.name.clone(), variable.secret));
                Ok(Some("ghp_typed".to_string()))
            },
        )
        .unwrap();
        assert_eq!(
            asked,
            vec![("GITHUB_PERSONAL_ACCESS_TOKEN".to_string(), true)]
        );
        assert_eq!(values["GITHUB_PERSONAL_ACCESS_TOKEN"], "ghp_typed");

        let from_env = collect_values(
            github,
            |name| (name == "GITHUB_PERSONAL_ACCESS_TOKEN").then(|| "ghp_env".to_string()),
            |_| panic!("not asked when the environment has it"),
        )
        .unwrap();
        assert_eq!(from_env["GITHUB_PERSONAL_ACCESS_TOKEN"], "ghp_env");

        let context7 = catalog.get("context7").unwrap();
        let skipped = collect_values(context7, |_| None, |_| Ok(None)).unwrap();
        assert!(skipped.is_empty());
    }
}
