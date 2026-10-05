//! Title normalisation: what a row shows, once the runtime's own prefixes are
//! taken off.
//!
//! Agents write their mark and their name into the terminal title, and Herdr
//! keeps the last one a pane had even after the session it came from is gone.
//! A row that already draws the vendor's mark does not need the mark repeated
//! in text, and a pane whose title is a finished session's is not saying
//! anything about what the pane is now.
//!
//! The rules follow `herdr-radar`'s `stripPiMark`/`stripVendorPulse`
//! (`lib/state.js`): strip only a mark or a name at the very start, only when a
//! separator says the rest is a title, and only when something is left over.
//! `gemini rocks` keeps its words — the vendor was not a prefix there.

use crate::theme;

/// A bracket Herdr and some agents pulse while something waits: `[!] foo`.
fn strip_pulse(title: &str) -> &str {
    let Some(rest) = title.strip_prefix('[') else {
        return title;
    };
    let Some(end) = rest.find(']') else {
        return title;
    };
    let inner = rest[..end].trim();
    if inner.is_empty() || !inner.chars().all(|ch| matches!(ch, '!' | '.' | '·' | ',')) {
        return title;
    }
    let after = rest[end + 1..].trim_start();
    if after.is_empty() { title } else { after }
}

/// Whether `ch` opens a title after a mark or a name.
fn is_separator(ch: char) -> bool {
    matches!(ch, '-' | '–' | '—' | '·' | '|' | ':' | '/')
}

/// The remainder after a leading `head` plus its separator, or `None` when
/// `head` does not start the title, no separator follows it, or nothing
/// readable is left. Whitespace alone is not a separator: `codex notes` is a
/// sentence, `codex - notes` is a prefix.
fn after_prefix<'a>(title: &'a str, head: &str) -> Option<&'a str> {
    if head.is_empty() || !title.get(..head.len())?.eq_ignore_ascii_case(head) {
        return None;
    }
    let rest = title[head.len()..].trim_start();
    let separator = rest.chars().next()?;
    if !is_separator(separator) {
        return None;
    }
    let rest = rest[separator.len_utf8()..].trim_start();
    (!rest.is_empty()).then_some(rest)
}

/// The first token of a title: what a provider name would have to be.
fn leading_token(title: &str) -> &str {
    let end = title
        .find(|ch: char| ch.is_whitespace() || is_separator(ch))
        .unwrap_or(title.len());
    &title[..end]
}

fn after_mark(title: &str) -> Option<&str> {
    let mark = title.chars().next()?;
    if !theme::is_mark(mark) {
        return None;
    }
    let rest = &title[mark.len_utf8()..];
    let rest = rest.trim_start();
    let rest = match rest.chars().next() {
        Some(ch) if is_separator(ch) => rest[ch.len_utf8()..].trim_start(),
        _ => rest,
    };
    if rest.is_empty() { None } else { Some(rest) }
}

/// Whether `text` starts with a mark or a name a provider writes, which is
/// what makes a latched pane label a session title rather than a pane name.
pub fn has_provider_prefix(text: &str) -> bool {
    let stripped = strip_pulse(text);
    if after_mark(stripped).is_some() {
        return true;
    }
    let token = leading_token(stripped);
    theme::is_agent(token) && after_prefix(stripped, token).is_some()
}

/// The vendor mark a latched session title opens with, when Radar knows the
/// provider that wrote it.
///
/// A finished session leaves its title — mark and all — on the pane's label,
/// and that mark is the only thing on the pane saying which agent it was.
pub fn session_vendor(text: &str) -> Option<&'static str> {
    let title = strip_pulse(text);
    let head = title.chars().next()?;
    if theme::is_mark(head) {
        return theme::vendor_mark(&head.to_string());
    }
    let token = leading_token(title);
    if after_prefix(title, token).is_some() {
        theme::vendor_mark(token)
    } else {
        None
    }
}

/// What a pane row should show.
///
/// A pane's own label wins while it names something — `nvim sidebar` is a name
/// someone gave the pane. A label that is a finished session's title does not:
/// the terminal title says what the pane is now, and the old title belongs in
/// the details, superseded.
///
/// A title that only repeats the workspace is dropped for the pane id: Herdr's
/// abbreviated cwd is most often the project directory the group header has
/// already named, and printing `~/P/d/nix-fleet` under `nix-fleet` spends the
/// width the rest of the row needs. A path somewhere else is kept — a pane in a
/// subdirectory, or running in another checkout, is saying something.
pub fn pane_title(
    label: Option<&str>,
    live: Option<&str>,
    workspace_label: Option<&str>,
    pane_id: &str,
) -> String {
    let label = label.filter(|text| !text.is_empty());
    let live = live.filter(|text| !text.is_empty());
    if let Some(label) = label.filter(|label| !has_provider_prefix(label)) {
        return display(label, None);
    }
    match live {
        Some(live) if !names_workspace(live, workspace_label) => display(live, None),
        // Nothing but the workspace's own directory: the header says it.
        Some(_) => pane_id.to_string(),
        None => label.map_or_else(|| pane_id.to_string(), |label| display(label, None)),
    }
}

/// Whether a title is only the directory the workspace is named after.
fn names_workspace(title: &str, workspace_label: Option<&str>) -> bool {
    let Some(label) = workspace_label.filter(|label| !label.is_empty()) else {
        return false;
    };
    let base = title
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(title);
    base.eq_ignore_ascii_case(label)
}

/// A title without a trailing `- <workspace>`: pi writes the workspace after the
/// session title, and the group header above the row has already named it.
///
/// Only a separated suffix that equals the workspace label goes, and only when
/// something is left.
pub fn strip_workspace_suffix<'a>(title: &'a str, workspace_label: Option<&str>) -> &'a str {
    let Some(label) = workspace_label.filter(|label| !label.is_empty()) else {
        return title;
    };
    let Some(head) = title
        .len()
        .checked_sub(label.len())
        .and_then(|at| title.get(..at).zip(title.get(at..)))
        .filter(|(_, tail)| tail.eq_ignore_ascii_case(label))
        .map(|(head, _)| head)
    else {
        return title;
    };
    // `my-nix-homelab` ends in the workspace without being a suffix on it.
    if !head.ends_with(char::is_whitespace) {
        return title;
    }
    let head = head.trim_end();
    match head.chars().next_back() {
        Some(ch) if is_separator(ch) => {
            let kept = head[..head.len() - ch.len_utf8()].trim_end();
            if kept.is_empty() { title } else { kept }
        }
        _ => title,
    }
}

/// The title a row should show.
///
/// `agent` is the row's own reported agent, when it has one. An agent row only
/// loses a name that matches itself — `codex` reported as `pi` keeps its words —
/// while a pane row, which knows nothing, loses any known provider name.
pub fn display(title: &str, agent: Option<&str>) -> String {
    let title = strip_pulse(title);
    let title = after_mark(title).unwrap_or(title);
    let named = match agent {
        Some(agent) => after_prefix(title, agent),
        None => {
            let token = leading_token(title);
            theme::is_agent(token)
                .then(|| after_prefix(title, token))
                .flatten()
        }
    };
    named.unwrap_or(title).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_a_trailing_workspace_only_when_separated_and_something_remains() {
        let strip = |title, label| strip_workspace_suffix(title, Some(label));
        assert_eq!(
            strip("Inspect Bifrost - nix-homelab", "nix-homelab"),
            "Inspect Bifrost"
        );
        assert_eq!(
            strip("Inspect Bifrost · NIX-HOMELAB", "nix-homelab"),
            "Inspect Bifrost"
        );
        // A word that merely ends the same way is not a suffix.
        assert_eq!(
            strip("Fix my-nix-homelab", "nix-homelab"),
            "Fix my-nix-homelab"
        );
        assert_eq!(strip("Fix nix-homelab", "nix-homelab"), "Fix nix-homelab");
        // Nothing left means nothing is stripped.
        assert_eq!(strip("- nix-homelab", "nix-homelab"), "- nix-homelab");
        assert_eq!(strip("nix-homelab", "nix-homelab"), "nix-homelab");
        assert_eq!(strip_workspace_suffix("a - b", None), "a - b");
    }

    #[test]
    fn strips_a_leading_vendor_mark_and_its_separator() {
        assert_eq!(display("π Lint sweep", Some("pi")), "Lint sweep");
        assert_eq!(
            display("π - Inspect Bifrost - nix-homelab", Some("pi")),
            "Inspect Bifrost - nix-homelab"
        );
        assert_eq!(display("π 01a0edc4", None), "01a0edc4");
    }

    #[test]
    fn strips_a_provider_name_only_where_it_is_a_prefix() {
        assert_eq!(display("codex - notes", Some("codex")), "notes");
        assert_eq!(display("agy: notes", None), "notes");
        // A word that merely starts the title is not a prefix.
        assert_eq!(display("gemini rocks", None), "gemini rocks");
        assert_eq!(display("codex notes", Some("codex")), "codex notes");
        // An agent that is not this row's own name stays in the words.
        assert_eq!(display("codex - notes", Some("pi")), "codex - notes");
    }

    #[test]
    fn never_empties_a_title_and_never_strips_a_bare_word() {
        assert_eq!(display("π", Some("pi")), "π");
        assert_eq!(display("pi", Some("pi")), "pi");
        assert_eq!(display("π · ", None), "π · ");
        assert_eq!(display("π: scratch", None), "scratch");
    }

    #[test]
    fn strips_a_pulsing_bracket() {
        assert_eq!(
            display("[!] Action Required | radar", Some("pi")),
            "Action Required | radar"
        );
        assert_eq!(display("[ ] keep", None), "[ ] keep");
        assert_eq!(display("[words] keep", None), "[words] keep");
    }

    #[test]
    fn a_latched_session_title_gives_way_to_what_the_pane_is_now() {
        assert_eq!(
            pane_title(
                Some("π 01a0edc4"),
                Some("~/P/d/agents"),
                Some("nix-fleet"),
                "wH:pG"
            ),
            "~/P/d/agents"
        );
        // A title that is only the directory the workspace is named after says
        // nothing the header has not already said.
        assert_eq!(
            pane_title(
                Some("π 01a0edc4"),
                Some("~/P/d/nix-fleet"),
                Some("nix-fleet"),
                "wH:pG"
            ),
            "wH:pG"
        );
        // A name someone gave the pane stays a name.
        assert_eq!(
            pane_title(
                Some("nvim sidebar"),
                Some("~/P/d/agents"),
                Some("sessions"),
                "wG:p22"
            ),
            "nvim sidebar"
        );
        // With nothing live to fall back on, the session title still says more
        // than the pane id.
        assert_eq!(
            pane_title(Some("π 01a0edc4"), None, Some("nix-fleet"), "wH:pG"),
            "01a0edc4"
        );
        assert_eq!(pane_title(None, None, None, "wA:p4"), "wA:p4");
    }

    #[test]
    fn recognises_which_labels_are_session_titles() {
        assert!(has_provider_prefix("π 01a0edc4"));
        assert!(has_provider_prefix("π - Inspect Bifrost - nix-homelab"));
        assert!(has_provider_prefix("codex - notes"));
        assert!(!has_provider_prefix("nvim sidebar"));
        assert!(!has_provider_prefix("agent"));
        assert!(!has_provider_prefix("~/P/d/nix-fleet"));
    }

    #[test]
    fn leaves_ordinary_titles_alone() {
        assert_eq!(
            display("~/Projects/dev/agent-radar", None),
            "~/Projects/dev/agent-radar"
        );
        assert_eq!(display("nvim sidebar", None), "nvim sidebar");
        assert_eq!(display("fleet owner task", Some("pi")), "fleet owner task");
    }
}
