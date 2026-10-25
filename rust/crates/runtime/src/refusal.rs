//! What Neuron says when the model declines to continue: the provider's note in
//! Neuron's words, and the one line a refused turn ends with.

/// How the line that ends a refused turn starts.
pub const REFUSAL_LINE: &str = "The model declined to continue this request.";

/// Names a provider's refusal note may carry, and what Neuron says instead. Only
/// the capitalized names of prose are replaced: lowercase ids (a provider id such
/// as `anthropic`, a model id such as `claude-opus-5-5`) are left as they are.
const PROVIDER_NAMES: &[(&str, &str)] = &[("Claude", "the model"), ("Anthropic", "the provider")];

/// How the Zero-X gateway's note starts ("Claude declined this request ..."), once
/// the name is replaced.
const DECLINED_LEAD: &str = "The model declined this request";

/// The provider's refusal note in Neuron's words. The model's and the provider's
/// names give way to "the model" and "the provider", and a lead saying that the
/// request was declined becomes "Declined", which [`refusal_line`] folds into its
/// own words instead of saying it twice:
///
/// "Claude declined this request (safety category: cyber). Rephrase it or choose
/// another model." becomes "Declined (safety category: cyber). Rephrase it or
/// choose another model."
#[must_use]
pub fn neutral_refusal_note(note: &str) -> String {
    let mut neutral = String::with_capacity(note.len());
    let mut rest = note;
    'scan: while let Some(next) = rest.chars().next() {
        let at_word_start = neutral.chars().next_back().is_none_or(|c| !is_word_char(c));
        if at_word_start {
            for (name, replacement) in PROVIDER_NAMES {
                let Some(after) = rest.strip_prefix(name) else {
                    continue;
                };
                if after.chars().next().is_some_and(is_word_char) {
                    continue;
                }
                if starts_sentence(&neutral) {
                    neutral.push_str(&capitalized(replacement));
                } else {
                    neutral.push_str(replacement);
                }
                rest = after;
                continue 'scan;
            }
        }
        neutral.push(next);
        rest = &rest[next.len_utf8()..];
    }
    let declined = neutral
        .get(..DECLINED_LEAD.len())
        .is_some_and(|lead| lead.eq_ignore_ascii_case(DECLINED_LEAD));
    if declined {
        format!("Declined{}", &neutral[DECLINED_LEAD.len()..])
    } else {
        neutral
    }
}

/// The one line a refused turn ends with: [`REFUSAL_LINE`], and the provider's
/// note (as [`neutral_refusal_note`] words it) when it sent one. A note that starts
/// with "Declined" is folded into the line: "Declined (safety category: cyber).
/// Rephrase it." gives "The model declined to continue this request (safety
/// category: cyber). Rephrase it."
#[must_use]
pub fn refusal_line(note: Option<&str>) -> String {
    let Some(note) = note.map(str::trim).filter(|note| !note.is_empty()) else {
        return REFUSAL_LINE.to_string();
    };
    match note.strip_prefix("Declined") {
        Some(rest) if rest.starts_with(" (") => {
            format!("{}{rest}", REFUSAL_LINE.trim_end_matches('.'))
        }
        Some(rest) if rest.is_empty() || rest.starts_with('.') => {
            let rest = rest.trim_start_matches('.').trim_start();
            if rest.is_empty() {
                REFUSAL_LINE.to_string()
            } else {
                format!("{REFUSAL_LINE} {rest}")
            }
        }
        _ => format!("{REFUSAL_LINE} {note}"),
    }
}

/// Characters that continue a word or an id (`claude-opus-5-5`, `anthropic/x`).
/// An apostrophe does not: "Anthropic's" is the name and a possessive.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | '/')
}

/// Whether text appended to `before` starts a sentence.
fn starts_sentence(before: &str) -> bool {
    let before = before.trim_end();
    before.is_empty() || before.ends_with(['.', '!', '?'])
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::{neutral_refusal_note, refusal_line, REFUSAL_LINE};

    /// The notes the Zero-X gateway's Claude adapter sends (`refusalNote`).
    const GATEWAY_NOTE: &str =
        "Claude declined this request (safety category: cyber). Rephrase it or choose another model.";
    const GATEWAY_NOTE_WITHOUT_CATEGORY: &str =
        "Claude declined this request. Rephrase it or choose another model.";
    const GATEWAY_REASONING_NOTE: &str =
        "Claude does not reproduce its internal reasoning. Ask for a short summary of the answer instead.";

    #[test]
    fn gateway_notes_lose_the_provider_names() {
        assert_eq!(
            neutral_refusal_note(GATEWAY_NOTE),
            "Declined (safety category: cyber). Rephrase it or choose another model."
        );
        assert_eq!(
            neutral_refusal_note(GATEWAY_NOTE_WITHOUT_CATEGORY),
            "Declined. Rephrase it or choose another model."
        );
        assert_eq!(
            neutral_refusal_note(GATEWAY_REASONING_NOTE),
            "The model does not reproduce its internal reasoning. Ask for a short summary of the answer instead."
        );
        assert_eq!(
            neutral_refusal_note("This breaks Anthropic's usage policy, so Claude stopped."),
            "This breaks the provider's usage policy, so the model stopped."
        );
        // Ids, other words and notes without a name stay as they are.
        for note in [
            "Declined (safety category: cyber). Rephrase it or choose another model.",
            "I can't help with that.",
            "Routed to anthropic/claude-opus-5-5 by Claudette's policy.",
        ] {
            assert_eq!(neutral_refusal_note(note), note);
        }
    }

    #[test]
    fn the_refusal_line_says_declined_once() {
        assert_eq!(refusal_line(None), REFUSAL_LINE);
        assert_eq!(refusal_line(Some("  ")), REFUSAL_LINE);
        let lines = [
            GATEWAY_NOTE,
            GATEWAY_NOTE_WITHOUT_CATEGORY,
            GATEWAY_REASONING_NOTE,
        ]
        .map(|note| refusal_line(Some(&neutral_refusal_note(note))));
        assert_eq!(
            lines,
            [
                "The model declined to continue this request (safety category: cyber). Rephrase it or choose another model.",
                "The model declined to continue this request. Rephrase it or choose another model.",
                "The model declined to continue this request. The model does not reproduce its internal reasoning. Ask for a short summary of the answer instead.",
            ]
        );
        for line in &lines {
            assert!(
                !line.contains("Claude") && !line.contains("Anthropic"),
                "{line}"
            );
            assert_eq!(line.matches("declined").count(), 1, "{line}");
        }
        assert_eq!(refusal_line(Some("Declined.")), REFUSAL_LINE);
        assert_eq!(
            refusal_line(Some("Declined by policy.")),
            "The model declined to continue this request. Declined by policy."
        );
        assert_eq!(
            refusal_line(Some("I can't help with that.")),
            "The model declined to continue this request. I can't help with that."
        );
    }
}
