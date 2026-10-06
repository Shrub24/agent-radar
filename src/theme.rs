//! Visual vocabulary: agent marks, lifecycle marks and colours.
//!
//! The glyph tables and the palette follow `herdr-radar`'s conventions so a
//! Radar pane and a Herdr sidebar agree on what a mark means. Marks are kept
//! to ordinary Unicode ([`TEXT`]); the Private Use Area face herdr-radar can
//! install is a font-level concern, not a rendering one, and shape — not only
//! colour — distinguishes every lifecycle state.
//!
//! A vendor absent from [`BRAND`] gets no colour of its own and takes the
//! row's ink, which is how a monochrome brand signs itself. Green and red are
//! withheld from vendors: they mean done and blocked.

use std::path::PathBuf;

use ratatui::style::Color;

use tui_spinner::FluxFrames;

use crate::config::{Config, Palette};
use crate::model::AgentState;

/// Ordinary-Unicode mark per agent, keyed by the name Herdr reports.
pub const TEXT: &[(&str, &str)] = &[
    ("claude", "§"),
    ("codex", "Λ"),
    ("opencode", "◇"),
    ("omp", "Π"),
    ("cline", "∇"),
    ("mastracode", "∑"),
    ("kimi", "✨"),
    ("kilo", "♟"),
    ("maki", "✳"),
    ("pi", "π"),
    ("hermes", "☪"),
    ("cursor", "◆"),
    ("copilot", "⊙"),
    ("deepseek", "≋"),
    ("gemini", "✦"),
    ("gpt", "✺"),
    ("qwen", "Ϙ"),
    ("grok", "✖"),
    ("agy", "△"),
    ("kiro", "Ω"),
    ("amp", "Ʌ"),
    ("devin", "ꓓ"),
    ("qodercli", "Ǫ"),
    ("glm", "Ƶ"),
    ("kimchi", "Ķ"),
    ("muse", "∞"),
    ("crush", "♥"),
];

/// The same marks in the Private Use Area of the icon font `herdr-radar`
/// installs (`Herdr Agent Icons Max`), whose glyphs are drawn logos rather
/// than borrowed punctuation. Selected only when that font is actually
/// present, so a terminal without it never shows tofu.
pub const PUA: &[(&str, &str)] = &[
    ("claude", "\u{e1a0}"),
    ("codex", "\u{e1a1}"),
    ("opencode", "\u{e1a2}"),
    ("omp", "\u{e1a3}"),
    ("cline", "\u{e1a4}"),
    ("mastracode", "\u{e1a5}"),
    ("kimi", "\u{e1a6}"),
    ("kilo", "\u{e1a7}"),
    ("maki", "\u{e1a8}"),
    ("pi", "\u{e1a9}"),
    ("hermes", "\u{e1aa}"),
    ("cursor", "\u{e1ab}"),
    ("copilot", "\u{e1ac}"),
    ("deepseek", "\u{e1ad}"),
    ("gemini", "\u{e1ae}"),
    ("gpt", "\u{e1af}"),
    ("qwen", "\u{e1b0}"),
    ("grok", "\u{e1b1}"),
    ("agy", "\u{e1b2}"),
    ("kiro", "\u{e1b3}"),
    ("amp", "\u{e1b4}"),
    ("devin", "\u{e1b5}"),
    ("qodercli", "\u{e1b6}"),
    ("glm", "\u{e1b7}"),
    ("kimchi", "\u{e1b8}"),
    ("muse", "\u{e1b9}"),
    ("crush", "\u{e1ba}"),
];

/// Which table the agent marks come from.
///
/// `auto` (the default) uses the icon font when it is installed, so the marks
/// are the drawn logos where they exist and never tofu where they do not.
/// `RADAR_ICONS=font|text|none` overrides it; `font` is what to set when the
/// font was installed system-wide rather than in the user font directory.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icons {
    Font,
    Text,
    None,
}

static ICONS: std::sync::OnceLock<Icons> = std::sync::OnceLock::new();

/// The mark table for this process, resolved once.
pub fn icons() -> Icons {
    *ICONS.get_or_init(detect_icons)
}

fn detect_icons() -> Icons {
    match std::env::var("RADAR_ICONS")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "font" => Icons::Font,
        "text" => Icons::Text,
        "none" => Icons::None,
        _ if icon_font_installed() => Icons::Font,
        _ => Icons::Text,
    }
}

/// Whether the icon font is in the user font directory, where `herdr-radar`
/// installs it (`$XDG_DATA_HOME/fonts`, else `~/.local/share/fonts`).
fn icon_font_installed() -> bool {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    let Some(dir) = base.map(|base| base.join("fonts")) else {
        return false;
    };
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("HerdrAgentIconsMax") && name.ends_with(".ttf")
        })
    })
}

/// Eight-dot braille frames: the gap walks round one cell, so a working row
/// reads as motion without a second animation fighting it.
/// The animations a working agent's mark can run, named for configuration.
///
/// The frames are [`FluxFrames`](tui_spinner::FluxFrames) — one-character
/// sequences built for exactly this, a single cell cycling through glyphs.
/// Radar draws the marks itself (its rows carry spans, not widgets), so what it
/// takes from the crate is the vocabulary rather than the widget.
pub const ANIMATIONS: &[(&str, &[char])] = &[
    ("arc", FluxFrames::ARC),
    ("bar", FluxFrames::BAR),
    ("block", FluxFrames::BLOCK),
    ("bounce", FluxFrames::BOUNCE),
    ("braille", FluxFrames::BRAILLE),
    ("circle-fill", FluxFrames::CIRCLE_FILL),
    ("classic", FluxFrames::CLASSIC),
    ("clock", FluxFrames::CLOCK),
    ("corners", FluxFrames::CORNERS),
    ("diamond", FluxFrames::DIAMOND),
    ("dice", FluxFrames::DICE),
    ("half", FluxFrames::HALF),
    ("line", FluxFrames::LINE),
    ("moon", FluxFrames::MOON),
    ("none", &[' ']),
    ("orbit", FluxFrames::ORBIT),
    ("pair", FluxFrames::PAIR),
    ("piston", FluxFrames::PISTON),
    ("pulse", FluxFrames::PULSE),
    ("square", FluxFrames::SQUARE),
    ("star", FluxFrames::STAR),
    ("triangles", FluxFrames::TRIANGLES),
];

/// The frames an animation is configured by, if it is one.
pub fn frames(name: &str) -> Option<&'static [char]> {
    ANIMATIONS
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, frames)| *frames)
}

/// The frame an animation is on at a step of the clock.
pub fn frame(frames: &[char], step: usize) -> char {
    frames[step % frames.len()]
}

/// The mark for the program a pane's foreground command runs, when a Nerd Font
/// can draw it and the user has not taken that program's mark away.
pub fn process_mark(program: &str) -> Option<&str> {
    let program = program.trim().to_ascii_lowercase();
    let mark = config().processes.get(&program)?;
    // Only a Nerd Font glyph needs the font: a plain character a user set
    // themselves is drawable whatever their terminal is running.
    if needs_nerd_font(mark) && !nerd_font() {
        return None;
    }
    Some(mark)
}

/// Whether a mark is drawn from Nerd Font's private use areas, where a terminal
/// without the font shows nothing rather than a character.
fn needs_nerd_font(mark: &str) -> bool {
    mark.chars()
        .any(|ch| matches!(ch, '\u{e000}'..='\u{f8ff}' | '\u{f0000}'..='\u{ffffd}'))
}

/// The mark in front of an ordinary pane with nothing running in it, so a pane
/// row is never a blank column beside a marked agent row above it.
pub fn pane_mark() -> &'static str {
    if nerd_font() { "\u{e795}" } else { "\u{25ad}" }
}

/// The warning mark beside a live agent whose running executable is not the
/// program installed now. A triangle from the Nerd Font where one can be drawn,
/// a plain bang where it cannot, so the row is never blank-then-nothing.
pub fn stale_mark() -> &'static str {
    stale_mark_for(nerd_font())
}

/// The stale mark for a terminal with or without the Nerd Font, so the choice
/// is testable without controlling the process-wide font detection.
fn stale_mark_for(nerd_font: bool) -> &'static str {
    if nerd_font { "\u{f071}" } else { "!" }
}

/// The frames the running-command marks move by: an ordinary pane's foreground
/// command, and the mark beside an agent row's running background work. One
/// setting, because both mean the same thing — a process is running here.
pub fn command_frames() -> Option<&'static [char]> {
    let name = config().appearance.command;
    if name == "none" { None } else { frames(name) }
}

/// Whether a Nerd Font is installed, which is what decides if the program marks
/// can be drawn rather than left as tofu. This is a separate question from the
/// agent icon font: that face carries the vendor marks and nothing else.
/// `RADAR_ICONS` overrides the guess — `font` asserts the terminal's font has
/// the glyphs, `text` and `none` say it does not.
fn nerd_font() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        match std::env::var("RADAR_ICONS")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "font" => true,
            "text" | "none" => false,
            _ => font_dirs().iter().any(|dir| has_nerd_font(dir, 0)),
        }
    })
}

/// Font directories worth looking in, the user's own first.
fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".local/share/fonts"));
        dirs.push(home.join(".fonts"));
    }
    dirs.push(PathBuf::from("/run/current-system/sw/share/fonts"));
    dirs.push(PathBuf::from("/usr/local/share/fonts"));
    dirs.push(PathBuf::from("/usr/share/fonts"));
    dirs
}

/// Whether a directory holds a font file whose name says Nerd Font, three
/// levels down, which is as deep as a font package nests them.
fn has_nerd_font(dir: &std::path::Path, depth: usize) -> bool {
    if depth > 3 {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if path.is_dir() {
            if has_nerd_font(&path, depth + 1) {
                return true;
            }
        } else if name.contains("nerd") && name.contains("font") {
            return true;
        }
    }
    false
}

/// The animation a state's mark runs, when the state has one.
///
/// Every state except idle and done has its own animation setting. `none`
/// preserves that state's fixed mark.
pub fn animation_for(state: &AgentState) -> Option<&'static [char]> {
    let appearance = &config().appearance;
    let name = match state {
        AgentState::Working => appearance.working,
        AgentState::Waiting => appearance.waiting,
        AgentState::Blocked => appearance.blocked,
        AgentState::Settling => appearance.settling,
        AgentState::Lost => appearance.lost,
        AgentState::Unknown | AgentState::Other(_) => appearance.unknown,
        AgentState::Idle | AgentState::Done => return None,
    };
    if name == "none" { None } else { frames(name) }
}

/// Whether a row's mark is animating.
///
/// Only a state that animates and is being observed now counts: a retained row
/// carries the last state anyone saw, so its mark stays on frame zero rather
/// than claiming activity nobody is observing.
pub fn animates(state: &AgentState, retained: bool) -> bool {
    !retained && animation_for(state).is_some()
}

/// The mark in front of a row: the animation's current frame for a state that
/// moves, otherwise the fixed shape Herdsman's own widget draws for that state.
fn state_mark(state: &AgentState, frame: usize) -> char {
    if let Some(frames) = animation_for(state) {
        return frames[frame % frames.len()];
    }
    match state {
        AgentState::Idle => '·',
        AgentState::Done => '✓',
        AgentState::Waiting => '◷',
        AgentState::Blocked => '◐',
        AgentState::Lost => '×',
        AgentState::Unknown | AgentState::Other(_) => '?',
        AgentState::Working => '⣀',
        AgentState::Settling => '◌',
    }
}

/// The colour of a row's state mark. `working` wears the vendor's brand colour
/// so the busy row is findable at a glance; every other state is semantic, and
/// `lost` is a failure rather than a hue of its own.
fn state_colour(state: &AgentState, agent: Option<&str>) -> Color {
    let palette = palette();
    match state {
        AgentState::Idle => palette.muted,
        AgentState::Working => agent.and_then(brand_colour).unwrap_or(palette.working),
        AgentState::Waiting => palette.waiting,
        AgentState::Blocked => palette.blocked,
        AgentState::Settling => palette.settling,
        AgentState::Done => palette.done,
        AgentState::Unknown | AgentState::Other(_) => palette.unknown,
        AgentState::Lost => palette.failed,
    }
}

/// The mark for an agent, or `None` for an agent with no published mark or a
/// disabled mark table — unknown agents are left unmarked rather than given a
/// stand-in that would read as the wrong vendor.
pub fn logo(agent: Option<&str>) -> Option<&'static str> {
    let agent = agent?;
    let table = match icons() {
        Icons::Font => PUA,
        Icons::Text => TEXT,
        Icons::None => return None,
    };
    table
        .iter()
        .find(|(name, _)| *name == agent)
        .map(|(_, mark)| *mark)
}

/// The mark for the vendor a title prefix names or marks, when Radar knows it.
///
/// A latched session title opens with the provider's own mark or name; this
/// turns either form into the mark a row would draw for that vendor.
pub fn vendor_mark(token: &str) -> Option<&'static str> {
    let name = TEXT
        .iter()
        .chain(PUA.iter())
        .find(|(name, mark)| *mark == token || name.eq_ignore_ascii_case(token))
        .map(|(name, _)| *name)?;
    logo(Some(name))
}

/// Whether `token` names a known agent, by id.
pub fn is_agent(token: &str) -> bool {
    config().brand(token).is_some()
        || TEXT
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case(token))
}

/// Whether Radar ships a mark for this agent kind: the vendors it knows.
///
/// Containment reads it to tell a known non-Pi agent — a kind Pi Herdsman never
/// manages — from one Radar cannot place, which is never guessed unmanaged. The
/// table is static, so a user's `[brands]` entry cannot change what is safe to
/// close.
pub fn shipped_agent(name: &str) -> bool {
    TEXT.iter()
        .chain(PUA.iter())
        .any(|(id, _)| id.eq_ignore_ascii_case(name))
}

/// Whether `ch` is one of the vendor marks, in either table. Used to take a
/// mark the runtime left in a title off a row that draws it already.
pub fn is_mark(ch: char) -> bool {
    TEXT.iter()
        .chain(PUA.iter())
        .any(|(_, mark)| mark.starts_with(ch))
}

/// A vendor's published colour, if the configuration has one for it.
fn brand_colour(agent: &str) -> Option<Color> {
    config().brand(agent)
}

/// The palette in use: the configured one, or the built-in default.
///
/// State hues are ANSI slots rather than fixed values, so they follow whatever
/// theme the terminal is using — hand-written or generated — without Radar
/// having to be told which one it is. Ratatui has no theme abstraction to hook
/// into, and the terminal palette is the only thing that follows the user.
pub fn palette() -> Palette {
    config().palette
}

/// The configuration in use: the installed one, or the built-in default.
///
/// Borrowed rather than copied: this is read per row, and the vendor table is
/// not something to clone on every render.
pub fn config() -> &'static Config {
    CONFIG.get_or_init(Config::default)
}

/// Sets the configuration this process draws with, once, at startup.
pub fn install(chosen: Config) {
    let _ = CONFIG.set(chosen);
}

static CONFIG: std::sync::OnceLock<Config> = std::sync::OnceLock::new();

/// The mark and colour for a tree row's lifecycle column.
///
/// Retained associations keep their last-observed mark but are painted in the
/// retained ink: a row that is not currently observed must not read as a live
/// state.
pub fn agent_state(
    state: &AgentState,
    agent: Option<&str>,
    retained: bool,
    frame: usize,
) -> (char, Color) {
    // A retained row is not being observed, so its mark does not move: it is
    // the last state anyone saw, held on frame zero.
    let mark = state_mark(state, if retained { 0 } else { frame });
    if retained {
        return (mark, palette().retained);
    }
    (mark, state_colour(state, agent))
}

/// The style for an agent row's own text.
///
/// The title carries the state colour its mark has, so a glance reads the state
/// without parsing a word, and the one lifecycle that is *moving* is set in
/// bold: weight is the second axis a coloured list needs, and spending it on the
/// busy rows keeps the finished and parked ones quiet. A retained row is drawn
/// in second-rank ink instead — it is not current, and must not read as if it
/// were.
pub fn agent_text(
    state: &AgentState,
    agent: Option<&str>,
    retained: bool,
) -> ratatui::style::Style {
    let style = ratatui::style::Style::new();
    if retained {
        return style.fg(palette().subtle);
    }
    let style = style.fg(state_colour(state, agent));
    // Weight is the second axis a coloured list needs: it marks work in flight.
    if matches!(state, AgentState::Working) {
        style.add_modifier(ratatui::style::Modifier::BOLD)
    } else {
        style
    }
}

/// The colour of an agent's own mark, or the row's ink when it has none.
pub fn agent_ink(agent: Option<&str>, retained: bool) -> Color {
    match (retained, agent) {
        (true, _) => palette().subtle,
        (false, Some(agent)) => brand_colour(agent).unwrap_or(palette().subtle),
        (false, None) => palette().subtle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_mark_needs_no_nerd_font_and_a_private_use_one_does() {
        // A character a user picks is drawable whatever font the terminal runs;
        // a Nerd Font codepoint is not, so the installed font decides.
        assert!(!needs_nerd_font("X"));
        assert!(!needs_nerd_font("\u{25ad}"));
        assert!(needs_nerd_font("\u{e795}"));
        assert!(needs_nerd_font("\u{f0524}"));
        // The stale warning is one of those codepoints, so a terminal without
        // the font gets the plain fallback rather than tofu.
        assert!(needs_nerd_font("\u{f071}"));
        assert_eq!(stale_mark_for(false), "!");
        assert_eq!(stale_mark_for(true), "\u{f071}");
        assert!(
            crate::process_icons::PROCESS_ICONS
                .iter()
                .all(|(_, mark)| needs_nerd_font(mark)),
            "every shipped mark is a Nerd Font codepoint, so the font decides"
        );
    }
}
