//! The colours Radar draws with, and where they come from.
//!
//! Palette choice is the user's, not the program's: a terminal theme is a
//! deliberate decision, and a dashboard that hard-codes hues argues with it.
//! Every colour Radar draws therefore has a name here, every name has a default
//! that reproduces Radar's own appearance, and a config file is a diff against
//! that baseline rather than a document that has to exist.
//!
//! Values are read and written as the terminal's own vocabulary — ANSI names
//! and indices — because those are the slots a theme actually sets. A literal
//! `#rrggbb` is accepted, and is then Radar's colour rather than the theme's.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ratatui::style::Color;
use serde::Deserialize;

use crate::process_icons::PROCESS_ICONS;

/// Every colour Radar draws, by the role it plays.
///
/// Roles rather than places: the same slot paints a row's text and its mark, and
/// a role that means one thing in one panel means it in the others too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// Headings: the ink a group label is drawn in, so the hierarchy reads
    /// before the words do. It is the one role meant to stand out from body
    /// text rather than recede from it.
    pub heading: Color,
    /// Panel borders and their titles: the frame around the content rather
    /// than the content, which some themes want dimmer than everything else.
    pub border: Color,
    /// Second-rank ink: field names, pane ids, and the mark of an agent whose
    /// vendor publishes no colour.
    pub subtle: Color,
    /// Ordinary body ink: a pane's own text, and the parked mark.
    pub muted: Color,
    /// A lifecycle the source did not report.
    pub unknown: Color,
    /// Finished.
    pub done: Color,
    /// Working, for an agent whose vendor publishes no colour of its own.
    pub working: Color,
    /// The model turn yielded while work it depends on is unresolved.
    pub waiting: Color,
    /// The active assignment waits on its owner.
    pub blocked: Color,
    /// A handoff, delivery or cleanup converging; also drawn animated.
    pub settling: Color,
    /// A retained association's last-observed facts.
    pub retained: Color,
    /// A source failure the user has to notice.
    pub failed: Color,
    /// A live agent whose running executable is not the one installed now.
    pub stale: Color,
    /// The fill behind the selected row.
    pub selection: Color,
}

impl Palette {
    /// Every role with the ink it resolved to, in the order the document
    /// prints them.
    ///
    /// The printed document and its round trip are derived from this, so a
    /// role cannot be added to the palette and silently left out of what the
    /// user is shown or able to set.
    pub fn named(&self) -> [(&'static str, Color); 14] {
        [
            ("heading", self.heading),
            ("border", self.border),
            ("subtle", self.subtle),
            ("muted", self.muted),
            ("unknown", self.unknown),
            ("done", self.done),
            ("working", self.working),
            ("waiting", self.waiting),
            ("blocked", self.blocked),
            ("settling", self.settling),
            ("retained", self.retained),
            ("failed", self.failed),
            ("stale", self.stale),
            ("selection", self.selection),
        ]
    }
}

impl Default for Palette {
    /// A working appearance, and nothing more than a starting point: it is what
    /// `radar --print-config` prints for editing.
    fn default() -> Self {
        Self {
            // The terminal's own foreground, drawn bold: a heading rather than
            // a hue, and legible on a theme Radar has never seen.
            heading: Color::Reset,
            border: Color::Reset,
            subtle: Color::DarkGray,
            muted: Color::Reset,
            unknown: Color::LightMagenta,
            done: Color::LightGreen,
            working: Color::LightBlue,
            // Calm but distinct from working: the agent is not idle, it is
            // waiting on something else.
            waiting: Color::LightCyan,
            // Attention, because a blocked assignment is the row to act on.
            blocked: Color::LightYellow,
            // Dimmer than working: a settling agent is winding down.
            settling: Color::Blue,
            retained: Color::LightYellow,
            failed: Color::LightRed,
            // A warning, not a failure: the process still runs, its file does
            // not match. Distinct from `blocked`'s light yellow so the two
            // read apart on the same row.
            stale: Color::Yellow,
            selection: Color::Black,
        }
    }
}

/// A vendor's own colour, keyed by the agent name a source reports.
///
/// A published brand colour is the brand's, so Radar ships the ones it knows
/// rather than inventing a hue per agent — but they are still the user's to
/// change, and a vendor Radar has never heard of can be given one here.
pub type Brands = BTreeMap<String, Color>;

/// The vendor colours Radar ships, as hex.
///
/// Chosen to stay legible on a light and a dark panel, since a brand colour
/// cannot follow the terminal's theme the way a palette slot can.
pub const BRAND_DEFAULTS: &[(&str, &str)] = &[
    ("pi", "#d67079"),
    ("claude", "#d97757"),
    ("gemini", "#4285f4"),
    ("kimi", "#1783ff"),
    ("deepseek", "#4d6bfe"),
    ("qwen", "#615ced"),
    ("kiro", "#9046ff"),
    ("cline", "#586876"),
    ("kilo", "#9a9808"),
    ("kimchi", "#ff521d"),
    ("muse", "#0082fb"),
    ("crush", "#ff388b"),
];

/// How Radar moves: an animation per state that animates, and how fast.
///
/// Marks are states; which animation a state runs is the user's, and a state
/// configured as `none` keeps its fixed mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Appearance {
    pub working: &'static str,
    pub waiting: &'static str,
    pub blocked: &'static str,
    pub settling: &'static str,
    pub lost: &'static str,
    pub unknown: &'static str,
    /// The marks that mean a process is running in a pane: an ordinary pane's
    /// foreground command and the badge beside an agent's unresolved tasks.
    pub command: &'static str,
    /// Frames per second; redraws happen only while a visible mark animates.
    pub fps: u32,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            working: "pulse",
            waiting: "clock",
            blocked: "none",
            settling: "orbit",
            lost: "none",
            unknown: "none",
            command: "pulse",
            fps: 10,
        }
    }
}

/// The loaded configuration: defaults with any file's choices applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub palette: Palette,
    pub brands: Brands,
    pub appearance: Appearance,
    /// Program marks for pane rows, by program name. Unknown programs fall back
    /// to the pane's own mark, and a value of `"none"` removes a program's.
    pub processes: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            palette: Palette::default(),
            brands: default_brands(),
            appearance: Appearance::default(),
            processes: default_processes(),
        }
    }
}

/// The program marks Radar ships with.
fn default_processes() -> BTreeMap<String, String> {
    PROCESS_ICONS
        .iter()
        .map(|(program, mark)| ((*program).to_string(), (*mark).to_string()))
        .collect()
}

/// The vendor colours Radar ships with, as a map.
fn default_brands() -> Brands {
    BRAND_DEFAULTS
        .iter()
        .filter_map(|(name, hex)| {
            parse_color(hex)
                .ok()
                .map(|color| ((*name).to_string(), color))
        })
        .collect()
}

impl Config {
    /// The colour for a vendor, if Radar has one for it.
    pub fn brand(&self, agent: &str) -> Option<Color> {
        self.brands.get(&agent.trim().to_ascii_lowercase()).copied()
    }
}

impl Config {
    /// Loads the configuration for this run.
    ///
    /// `RADAR_CONFIG` names the file; otherwise it is
    /// `$XDG_CONFIG_HOME/radar/config.toml`, else `~/.config/radar/config.toml`.
    /// A file that is not there is not an error — the defaults are the
    /// configuration. A file that is there and cannot be used is an error the
    /// caller should report: silently ignoring a mistyped colour would leave the
    /// user editing a file that does nothing.
    pub fn load() -> Result<Self, String> {
        let Some(path) = config_path() else {
            return Ok(Self::default());
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    /// Parses a configuration document over the defaults.
    pub fn parse(text: &str) -> Result<Self, String> {
        let file: FileConfig = toml::from_str(text).map_err(|error| error.to_string())?;
        let mut config = Self::default();
        let colors = file.colors.unwrap_or_default();
        for (name, spec) in colors.entries() {
            let Some(spec) = spec else { continue };
            let color =
                parse_color(&spec.to_string()).map_err(|error| format!("{name}: {error}"))?;
            config.palette.set(name, color);
        }
        for (agent, spec) in file.brands.unwrap_or_default() {
            let color = parse_color(&spec.to_string())
                .map_err(|error| format!("brands.{agent}: {error}"))?;
            config.brands.insert(agent.to_ascii_lowercase(), color);
        }
        for (program, mark) in file.processes.unwrap_or_default() {
            let mark = mark.trim();
            let program = program.trim().to_ascii_lowercase();
            // An empty value or `none` gives a program no mark, which is how a
            // user takes one of the built-in ones away.
            if mark.is_empty() || mark.eq_ignore_ascii_case("none") {
                config.processes.remove(&program);
                continue;
            }
            if mark.chars().count() > 2 {
                return Err(format!(
                    "processes.{program}: `{mark}` is more than two characters; a mark is one \
                     column, and two at most where a glyph is drawn with its variation selector"
                ));
            }
            config.processes.insert(program, mark.to_string());
        }
        let appearance = file.appearance.unwrap_or_default();
        if let Some(name) = appearance.working {
            config.appearance.working = animation_name("working", &name)?;
        }
        if let Some(name) = appearance.settling {
            config.appearance.settling = animation_name("settling", &name)?;
        }
        if let Some(name) = appearance.waiting {
            config.appearance.waiting = animation_name("waiting", &name)?;
        }
        if let Some(name) = appearance.blocked {
            config.appearance.blocked = animation_name("blocked", &name)?;
        }
        if let Some(name) = appearance.lost {
            config.appearance.lost = animation_name("lost", &name)?;
        }
        if let Some(name) = appearance.unknown {
            config.appearance.unknown = animation_name("unknown", &name)?;
        }
        if let Some(name) = appearance.command {
            config.appearance.command = animation_name("command", &name)?;
        }
        if let Some(fps) = appearance.fps {
            if !(1..=60).contains(&fps) {
                return Err(format!(
                    "appearance.fps: {fps} is not between 1 and 60 frames per second"
                ));
            }
            config.appearance.fps = fps;
        }
        Ok(config)
    }

    /// The effective configuration as a document, for a user to edit.
    ///
    /// Printed rather than shipped: it is a diff against the built-in defaults,
    /// so what a user needs is what their build actually resolved to.
    pub fn to_document(&self) -> String {
        let palette = self.palette;
        let mut out = String::from(
            "# Radar's colours.\n\
             #\n\
             # Values name the terminal's own palette slots, so they follow the theme\n\
             # the terminal is already using:\n\
             #   \"default\"            the terminal's foreground\n\
             #   \"red\", \"light-blue\", \"dark-gray\", ...   the ANSI slots\n\
             #   0-255                a palette index\n\
             #   \"#rrggbb\"            a fixed colour the theme cannot change\n\
             #\n\
             # Every key below is optional; a missing one keeps its default.\n\
             [colors]\n\
             # group headings, drawn bold",
        );
        for (role, color) in palette.named() {
            out.push_str(&format!("\n{role} = \"{}\"", color_spec(color)));
        }
        out.push_str(
            "\n\n# Vendor colours. These cannot follow the terminal theme the way a palette\n\
             # slot does — a published brand colour is the brand's — so they are literal.\n\
             # A key is the agent name the source reports; add one for a vendor Radar\n\
             # does not know, or change one of these.\n\
             [brands]",
        );
        for (name, hex) in BRAND_DEFAULTS {
            let color = self
                .brands
                .get(*name)
                .copied()
                .unwrap_or_else(|| parse_color(hex).expect("built-in brand colours are valid"));
            out.push_str(&format!("\n{name} = \"{}\"", color_spec(color)));
        }
        // A vendor the user added is part of the configuration too, so the
        // printed document stays the whole of it.
        for (name, color) in &self.brands {
            if BRAND_DEFAULTS.iter().any(|(known, _)| *known == name) {
                continue;
            }
            out.push_str(&format!("\n{name} = \"{}\"", color_spec(*color)));
        }
        let names = comment_wrapped(ANAMES.iter().map(|(name, _)| *name));
        // One key per state that animates, so a state's motion is set beside
        // its colour rather than by one global spinner.
        out.push_str(&format!(
            "\n\n# How the marks move. A state that animates names its animation here, and\n\
             # Radar redraws only while one is on screen — a quiet fleet draws nothing,\n\
             # so the rate is what a busy one costs.\n\
             [appearance]\n\
             # frames per second\n\
             fps = {}\n\
             # a working agent's mark\n\
             {}\n\
             working = \"{}\"\n\
             waiting = \"{}\"\n\
             blocked = \"{}\"\n\
             settling = \"{}\"\n\
             lost = \"{}\"\n\
             unknown = \"{}\"\n\
             # a command running in a pane, and an agent's running background work\n\
             command = \"{}\"",
            self.appearance.fps,
            names,
            self.appearance.working,
            self.appearance.waiting,
            self.appearance.blocked,
            self.appearance.settling,
            self.appearance.lost,
            self.appearance.unknown,
            self.appearance.command,
        ));
        out.push_str(
            "\n\n# Program marks for pane rows, keyed by the program the pane's foreground\n\
             # command runs. A program that is not listed keeps the pane's own mark;\n\
             # a value of \"none\" takes a built-in one away. These are Nerd Font\n\
             # glyphs, drawn only where a Nerd Font is installed.\n\
             [processes]",
        );
        for (program, mark) in &self.processes {
            out.push_str(&format!("\n\"{program}\" = \"{mark}\""));
        }
        // A program the user took away is written out as `none` rather than
        // left out: the document is a diff against the built-in table, so
        // leaving it out would put it back on the next read.
        for (program, _) in PROCESS_ICONS {
            if !self.processes.contains_key(*program) {
                out.push_str(&format!("\n\"{program}\" = \"none\""));
            }
        }
        out.push('\n');
        out
    }
}

/// The animation names, as the document lists them.
use crate::theme::ANIMATIONS as ANAMES;

/// Fits a list of names into comment lines, so the printed document stays
/// readable however many animations there are.
fn comment_wrapped<'a>(names: impl Iterator<Item = &'a str>) -> String {
    let mut lines: Vec<String> = Vec::new();
    for name in names {
        match lines.last_mut() {
            Some(line) if line.len() + name.len() + 2 <= 72 => {
                line.push_str(", ");
                line.push_str(name);
            }
            _ => lines.push(format!("# {name}")),
        }
    }
    lines.join("\n")
}

/// Resolves an animation name, reporting one that is not an animation.
fn animation_name(state: &str, name: &str) -> Result<&'static str, String> {
    let folded = name.trim().to_ascii_lowercase();
    ANAMES
        .iter()
        .find(|(candidate, _)| *candidate == folded)
        .map(|(candidate, _)| *candidate)
        .ok_or_else(|| {
            let names: Vec<&str> = ANAMES.iter().map(|(name, _)| *name).collect();
            format!(
                "appearance.{state}: unknown animation `{name}`; use one of {}",
                names.join(", ")
            )
        })
}

/// The roles, in the order they are documented and printed.
impl Palette {
    fn set(&mut self, role: &str, color: Color) {
        match role {
            "heading" => self.heading = color,
            "border" => self.border = color,
            "subtle" => self.subtle = color,
            "muted" => self.muted = color,
            "unknown" => self.unknown = color,
            "done" => self.done = color,
            "working" => self.working = color,
            "waiting" => self.waiting = color,
            "blocked" => self.blocked = color,
            "settling" => self.settling = color,
            "retained" => self.retained = color,
            "failed" => self.failed = color,
            "stale" => self.stale = color,
            "selection" => self.selection = color,
            _ => {}
        }
    }
}

/// Where the configuration file is, if a home directory can be found at all.
fn config_path() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("RADAR_CONFIG") {
        return Some(PathBuf::from(explicit));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("radar").join("config.toml"))
}

/// The ANSI names a value may use, paired with what they mean.
const NAMES: &[(&str, Color)] = &[
    ("default", Color::Reset),
    ("reset", Color::Reset),
    ("black", Color::Black),
    ("red", Color::Red),
    ("green", Color::Green),
    ("yellow", Color::Yellow),
    ("blue", Color::Blue),
    ("magenta", Color::Magenta),
    ("cyan", Color::Cyan),
    ("gray", Color::Gray),
    ("grey", Color::Gray),
    ("dark-gray", Color::DarkGray),
    ("dark-grey", Color::DarkGray),
    ("light-red", Color::LightRed),
    ("light-green", Color::LightGreen),
    ("light-yellow", Color::LightYellow),
    ("light-blue", Color::LightBlue),
    ("light-magenta", Color::LightMagenta),
    ("light-cyan", Color::LightCyan),
    ("white", Color::White),
];

/// Reads one colour: a name, a palette index, or a literal `#rrggbb`.
pub fn parse_color(spec: &str) -> Result<Color, String> {
    let value = spec.trim();
    if value.is_empty() {
        return Err("no colour given".into());
    }
    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex(hex).ok_or_else(|| format!("`{spec}` is not a #rrggbb colour"));
    }
    if let Ok(index) = value.parse::<u8>() {
        return Ok(Color::Indexed(index));
    }
    let folded = value.to_ascii_lowercase().replace('_', "-");
    NAMES
        .iter()
        .find(|(name, _)| *name == folded)
        .map(|(_, color)| *color)
        .ok_or_else(|| {
            let names: Vec<&str> = NAMES.iter().map(|(name, _)| *name).collect();
            format!(
                "unknown colour `{spec}`; use one of {}, 0-255, or #rrggbb",
                names.join(", ")
            )
        })
}

/// How a colour is written back out: the name it would be configured by, so the
/// printed document is the one a user can edit.
fn color_spec(color: Color) -> String {
    if let Some((name, _)) = NAMES.iter().find(|(_, known)| *known == color) {
        return (*name).to_string();
    }
    match color {
        Color::Indexed(index) => index.to_string(),
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        other => format!("{other:?}").to_ascii_lowercase(),
    }
}

fn parse_hex(hex: &str) -> Option<Color> {
    let (r, g, b) = match hex.len() {
        6 => (
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
        ),
        3 => {
            let digit = |index: usize| {
                let value = u8::from_str_radix(&hex[index..=index], 16).ok()?;
                Some(value * 17)
            };
            (digit(0)?, digit(1)?, digit(2)?)
        }
        _ => return None,
    };
    Some(Color::Rgb(r, g, b))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    colors: Option<FileColors>,
    brands: Option<BTreeMap<String, ColorSpec>>,
    appearance: Option<FileAppearance>,
    processes: Option<BTreeMap<String, String>>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FileAppearance {
    working: Option<String>,
    waiting: Option<String>,
    blocked: Option<String>,
    settling: Option<String>,
    lost: Option<String>,
    unknown: Option<String>,
    command: Option<String>,
    fps: Option<u32>,
}

impl FileColors {
    /// The roles the document set, in a fixed order.
    fn entries(&self) -> [(&'static str, Option<&ColorSpec>); 14] {
        [
            ("heading", self.heading.as_ref()),
            ("border", self.border.as_ref()),
            ("subtle", self.subtle.as_ref()),
            ("muted", self.muted.as_ref()),
            ("unknown", self.unknown.as_ref()),
            ("done", self.done.as_ref()),
            ("working", self.working.as_ref()),
            ("waiting", self.waiting.as_ref()),
            ("blocked", self.blocked.as_ref()),
            ("settling", self.settling.as_ref()),
            ("retained", self.retained.as_ref()),
            ("failed", self.failed.as_ref()),
            ("stale", self.stale.as_ref()),
            ("selection", self.selection.as_ref()),
        ]
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FileColors {
    heading: Option<ColorSpec>,
    border: Option<ColorSpec>,
    subtle: Option<ColorSpec>,
    muted: Option<ColorSpec>,
    unknown: Option<ColorSpec>,
    done: Option<ColorSpec>,
    working: Option<ColorSpec>,
    waiting: Option<ColorSpec>,
    blocked: Option<ColorSpec>,
    settling: Option<ColorSpec>,
    retained: Option<ColorSpec>,
    failed: Option<ColorSpec>,
    stale: Option<ColorSpec>,
    selection: Option<ColorSpec>,
}

/// A colour as a document writes it: `done = "green"` or `selection = 8`.
#[derive(Clone, Deserialize)]
#[serde(untagged)]
enum ColorSpec {
    Name(String),
    Index(u8),
}

impl std::fmt::Display for ColorSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Name(name) => formatter.write_str(name),
            Self::Index(index) => write!(formatter, "{index}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_marks_come_from_the_table_and_a_user_edits_them() {
        let config = Config::parse(
            r#"[processes]
nvim = "N"
htop = "none"
"#,
        )
        .expect("process marks parse");
        assert_eq!(config.processes.get("nvim").map(String::as_str), Some("N"));
        assert!(
            !config.processes.contains_key("htop"),
            "`none` takes a built-in away"
        );
        assert!(
            config.processes.contains_key("git"),
            "every other program keeps its mark"
        );

        let document = config.to_document();
        assert!(document.contains("\"nvim\" = \"N\""), "{document}");
        assert!(
            document.contains("\"htop\" = \"none\""),
            "a removed program is written out as none, so the document still says so:\n{document}"
        );
        let round_trip = Config::parse(&document).expect("the document parses");
        assert_eq!(round_trip.processes, config.processes);

        let error = Config::parse("[processes]\nnvim = \"three\"").expect_err("too long");
        assert!(error.contains("processes.nvim"), "{error}");
        // The running marks are one setting, and `none` is the reduced-motion
        // answer that keeps them still.
        let still = Config::parse("[appearance]\ncommand = \"none\"\n").expect("parses");
        assert_eq!(still.appearance.command, "none");
        assert!(still.to_document().contains("command = \"none\""));
    }

    #[test]
    fn every_palette_role_is_documentable_and_settable() {
        let document = Config::default().to_document();
        for (role, _) in Palette::default().named() {
            assert!(
                document.contains(&format!("\n{role} = ")),
                "{role} is missing from the document the user edits"
            );
            // And naming it back must set it rather than being ignored as an
            // unknown key.
            let config = Config::parse(&format!("[colors]\n{role} = \"red\"\n"))
                .unwrap_or_else(|error| panic!("{role} is not settable: {error}"));
            assert_eq!(
                config
                    .palette
                    .named()
                    .iter()
                    .find(|(r, _)| *r == role)
                    .map(|(_, c)| *c),
                Some(Color::Red)
            );
        }
    }

    #[test]
    fn names_indices_and_hex_all_read_as_colours() {
        assert_eq!(parse_color("green"), Ok(Color::Green));
        assert_eq!(parse_color("Light-Blue"), Ok(Color::LightBlue));
        assert_eq!(parse_color("dark_grey"), Ok(Color::DarkGray));
        assert_eq!(parse_color("default"), Ok(Color::Reset));
        assert_eq!(parse_color("4"), Ok(Color::Indexed(4)));
        assert_eq!(parse_color("255"), Ok(Color::Indexed(255)));
        assert_eq!(parse_color("#ff8800"), Ok(Color::Rgb(255, 136, 0)));
        assert_eq!(parse_color("#f80"), Ok(Color::Rgb(255, 136, 0)));
    }

    #[test]
    fn a_colour_it_does_not_know_says_so() {
        let error = parse_color("chartreuse").expect_err("unknown");
        assert!(error.contains("chartreuse"), "{error}");
        assert!(parse_color("#12345").is_err());
        assert!(parse_color("").is_err());
    }

    #[test]
    fn a_file_sets_only_what_it_names() {
        let config = Config::parse("[colors]\ndone = \"red\"\nselection = 4\n").expect("parses");
        assert_eq!(config.palette.done, Color::Red);
        assert_eq!(config.palette.selection, Color::Indexed(4));
        // Everything else keeps the built-in default.
        assert_eq!(config.palette.working, Palette::default().working);
    }

    #[test]
    fn vendor_colours_ship_and_can_be_overridden_or_added() {
        let config = Config::default();
        assert_eq!(config.brand("claude"), Some(Color::Rgb(0xd9, 0x77, 0x57)));
        // A vendor Radar has never heard of can be given a colour.
        let config =
            Config::parse("[brands]\nclaude = 208\nmy-agent = \"#00ff00\"\npi = \"light-blue\"\n")
                .expect("parses");
        assert_eq!(config.brand("claude"), Some(Color::Indexed(208)));
        assert_eq!(config.brand("my-agent"), Some(Color::Rgb(0, 255, 0)));
        assert_eq!(config.brand("pi"), Some(Color::LightBlue));
        // An untouched vendor keeps its shipped colour.
        assert_eq!(config.brand("gemini"), Config::default().brand("gemini"));
    }

    #[test]
    fn a_vendor_the_user_added_is_printed_too() {
        let config = Config::parse("[brands]\nmy-agent = \"#00ff00\"\n").expect("parses");
        let printed = config.to_document();
        assert!(printed.contains("my-agent = \"#00ff00\""), "{printed}");
        assert_eq!(Config::parse(&printed).expect("round trips"), config);
        // And no vendor is printed twice.
        assert_eq!(printed.matches("\npi = ").count(), 1, "{printed}");
    }

    #[test]
    fn an_empty_document_changes_nothing() {
        assert_eq!(Config::parse("").expect("parses"), Config::default());
        assert_eq!(
            Config::parse("# a comment\n").expect("parses"),
            Config::default()
        );
    }

    #[test]
    fn each_animating_state_names_its_own_animation() {
        let config = Config::parse("[appearance]\nworking = \"moon\"\nfps = 20\n").expect("parses");
        assert_eq!(config.appearance.working, "moon");
        assert_eq!(config.appearance.fps, 20);
        let document = config.to_document();
        assert!(document.contains("working = \"moon\""), "{document}");
        assert!(document.contains("fps = 20"), "{document}");
        // Every animation the document offers is one the loader accepts, so a
        // name read off `--print-config` always works.
        for (name, _) in crate::theme::ANIMATIONS {
            let config =
                Config::parse(&format!("[appearance]\nworking = \"{name}\"\n")).expect("parses");
            assert_eq!(config.appearance.working, *name);
        }
    }

    #[test]
    fn an_unknown_animation_or_rate_is_reported() {
        let error = Config::parse("[appearance]\nworking = \"chartreuse\"\n").expect_err("unknown");
        assert!(error.contains("appearance.working"), "{error}");
        assert!(error.contains("unknown animation"), "{error}");
        let error = Config::parse("[appearance]\nfps = 0\n").expect_err("out of range");
        assert!(error.contains("appearance.fps"), "{error}");
    }

    #[test]
    fn a_mistyped_key_or_colour_is_an_error_not_a_silent_default() {
        let error = Config::parse("[colors]\ndne = \"red\"\n").expect_err("unknown key");
        assert!(error.contains("dne"), "{error}");
        let error = Config::parse("[colors]\ndone = \"crimson\"\n").expect_err("unknown colour");
        assert!(error.contains("done"), "{error}");
        assert!(Config::parse("[colours]\ndone = \"red\"\n").is_err());
    }

    #[test]
    fn the_printed_document_reads_back_as_the_same_configuration() {
        let config = Config::default();
        let printed = config.to_document();
        assert_eq!(Config::parse(&printed).expect("round trips"), config);
        // And it is a document a user can edit: every role is present and
        // every role can be set back through it.
        for (role, _) in Palette::default().named() {
            assert!(printed.contains(role), "{role} is missing:\n{printed}");
            let set = Config::parse(&format!("[colors]\n{role} = \"red\"\n")).expect("settable");
            assert_ne!(
                set.palette.named(),
                Config::default().palette.named(),
                "{role}"
            );
        }
    }
}
