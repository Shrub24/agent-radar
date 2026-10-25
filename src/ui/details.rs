//! The detail pages: overview, registry, process and task pages, and the process table.

use super::*;

/// What the foreground program has done to the terminal, in words.
pub(super) fn terminal_line(mode: TerminalMode) -> String {
    match mode {
        TerminalMode::FullScreen => "full screen — the program is drawing the pane".into(),
        TerminalMode::Line => "line mode — the shell still owns the terminal".into(),
        TerminalMode::Unknown => unavailable(),
    }
}

/// The verdict a freshness comparison has to give, where it has one to give.
///
/// A current comparison has none: the package line above names what is running,
/// and a stale mark's absence is what says it is the installed one. A
/// deliberate other build says what it is and stops there — naming both roots
/// on a line a reader scans is what the block is for. A comparison that could
/// not be made gives its reason in a word; the sentence is the block's.
pub(super) fn verdict_line(identity: &BinaryIdentity) -> Option<Line<'static>> {
    match identity.freshness {
        BinaryFreshness::Current if identity.running == identity.installed => None,
        // One side alone cannot be compared with anything, so nothing is
        // claimed either way.
        BinaryFreshness::Current if identity.running.is_none() || identity.installed.is_none() => {
            None
        }
        BinaryFreshness::Current => Some(field("binary", "other build".to_string())),
        // A comparison nobody attempted carries no reason, and a verdict with
        // no reason to give is not a verdict.
        BinaryFreshness::Unknown => identity
            .unknown
            .map(|reason| field("binary", unknown_word(reason).to_string())),
        BinaryFreshness::Stale => {
            let palette = theme::palette();
            let mut spans = vec![Span::styled(
                "binary: ".to_string(),
                Style::new().fg(palette.subtle),
            )];
            spans.push(Span::styled(
                format!("{} (stale)", theme::stale_mark()),
                Style::new().fg(palette.stale),
            ));
            // The installed target, named as compactly as the package line
            // names the running one. A running file the kernel marked deleted
            // has no counterpart to name, and the mark stands alone.
            if let Some(installed) = identity.installed.as_deref() {
                spans.push(Span::raw(format!(
                    " installed {}",
                    package_text(installed, identity.running.as_deref())
                )));
            }
            Some(Line::from(spans))
        }
    }
}

/// Where the store keeps its packages. A root under it is a hash and a name;
/// anywhere else a path is its own identity.
pub(super) const STORE: &str = "/nix/store/";

/// How much of a store hash the page shows: enough to tell two builds of one
/// version apart in a panel, far short of the 32 characters that name the same
/// root twice.
pub(super) const SHORT_HASH: usize = 7;

/// A store root's hash and package name, for `/nix/store` paths only.
pub(super) fn store_root(path: &str) -> Option<(&str, &str)> {
    let (hash, name) = path.strip_prefix(STORE)?.split_once('-')?;
    (!hash.is_empty() && !name.is_empty()).then_some((hash, name))
}

/// The first [`SHORT_HASH`] characters of a store hash, cut on a character
/// boundary so a path this machine did not write cannot split a character.
pub(super) fn short_hash(hash: &str) -> String {
    hash.chars().take(SHORT_HASH).collect()
}

/// A package as one short phrase: its name and version, and a short hash of its
/// store root where two builds of that same version have to be told apart.
///
/// Two versions name themselves, and one installation compared with itself has
/// only one build to name; two builds of one version are the same words over
/// different bits, and the hash is the only thing that tells a reader which of
/// them is running. A path outside the store has no version to prefer and is
/// named whole — it is short, and it is what the runtime was given.
pub(super) fn package_text(root: &str, other: Option<&str>) -> String {
    let Some((hash, name)) = store_root(root) else {
        return sanitize(root);
    };
    match other.and_then(store_root) {
        Some((other_hash, other_name)) if other_name == name && other_hash != hash => {
            format!("{} {}", sanitize(name), short_hash(hash))
        }
        _ => sanitize(name),
    }
}

/// A path inside a root, as it is drawn: the root is named once, by the package
/// line, and repeating it under every file says nothing.
///
/// A path only sharing a prefix with the root — another package beside it — is
/// not inside it, and a path that *is* the root has nothing relative to show.
pub(super) fn relative_to<'a>(path: &'a str, root: Option<&str>) -> Option<&'a str> {
    let rest = path.strip_prefix(root?)?;
    if rest.is_empty() {
        return None;
    }
    rest.strip_prefix('/')
}

/// The same reason in a word or two, for the line a reader scans: the sentence
/// it is cut from is the block's.
pub(super) fn unknown_word(reason: BinaryUnknown) -> &'static str {
    match reason {
        BinaryUnknown::NoCounterpart => "no counterpart",
        BinaryUnknown::UnsupportedLauncher => "payload unknown",
        BinaryUnknown::Unreadable => "unreadable",
        BinaryUnknown::NotCompared => "not the named program",
    }
}

/// Why nothing was compared, in the page's words. Each reason is a different
/// thing to go and look at.
pub(super) fn unknown_reason(reason: BinaryUnknown) -> &'static str {
    match reason {
        BinaryUnknown::NoCounterpart => "no installed counterpart resolves",
        BinaryUnknown::UnsupportedLauncher => {
            "the installed counterpart's payload could not be identified"
        }
        BinaryUnknown::Unreadable => "the running executable could not be read",
        BinaryUnknown::NotCompared => "the running file is not the program the runtime named",
    }
}

/// What a descendant sum is, said on the page that draws one: the processes the
/// kernel reports beneath a root, which is neither who owns them nor what work
/// they serve — and a resident set that counts a page shared by two processes
/// in each of them.
pub(super) const DESCENDANT_SUM: &str = "observed processes beneath this one, not a workload or assignment total \
     (a page shared with another process counts in each)";

/// The measurements of one process: what the kernel says it is doing and what it
/// is using now.
///
/// The values are whoever the page is describing — the row's own foreground, or
/// the descendant the reader selected — and nothing here is inherited from an
/// owner, a session or a background task: a task's published PID arrives without
/// a birth identity and is not sampled at all. A value this machine could not
/// measure says so with its reason rather than as a zero, and a measured idle
/// interval is drawn as the zero it is.
pub(super) fn process_section(
    state: ProcessState,
    cpu: Option<CpuPercent>,
    rss_bytes: Option<u64>,
) -> Vec<Line<'static>> {
    let (cpu, cpu_reason) = measured_value(
        cpu_value(cpu),
        "no interval of this process has been measured",
    );
    let (rss, rss_reason) = measured_value(
        rss_value(rss_bytes),
        "the kernel's page count could not be converted",
    );
    metric_section(
        "process",
        &[
            vec![Metric::stated("state", state_word(state))],
            vec![
                Metric::measured("cpu", cpu, cpu_reason),
                Metric::measured("rss", rss, rss_reason),
            ],
        ],
    )
}

/// The qualified sums of what was observed beneath a root: how many processes,
/// and what they are using between them.
///
/// The root's own figures are drawn apart from these and never added to them: a
/// build under a pane is the build's CPU, not the pane's. A sum that could not
/// cover every member is drawn as the lower bound it is, with the reason it is
/// one, and a root nothing could be enumerated beneath draws that as unavailable
/// rather than as a measured zero.
pub(super) fn descendant_section(resources: &ProcessResources) -> Vec<Line<'static>> {
    let descendants = &resources.descendants;
    let (count, count_reason) = measured_value(
        descendant_count(descendants.observed),
        "nothing beneath this process could be enumerated",
    );
    let (total_cpu, total_cpu_reason) = total_value(&descendants.cpu, |cpu| cpu_percent(*cpu));
    let (total_rss, total_rss_reason) = total_value(&descendants.rss_bytes, |bytes| size(*bytes));
    metric_section(
        "descendants",
        &[
            vec![Metric::measured("count", count, count_reason)],
            vec![
                Metric::measured("cpu", total_cpu, total_cpu_reason),
                Metric::measured("rss", total_rss, total_rss_reason),
            ],
        ],
    )
}

/// Cells between one fact of a metric section and the fact beside it, and
/// between a fact's label and its value.
pub(super) const METRIC_FACT_GAP: usize = 2;
pub(super) const METRIC_LABEL_GAP: usize = 2;

/// The narrowest panel a metric section is drawn in on the side-by-side layout.
///
/// Two facts share a line only when both fit inside that width. A row that had
/// to wrap leaves the second fact's label at the end of the line above its own
/// value, and a label read apart from its number is worse than a fact on a line
/// of its own. The panel's real width is not known here — the page is built
/// before it is laid out — so the width the layout can never go below is the
/// one a pair has to fit.
pub(super) const METRIC_PAIR_WIDTH: usize = 28;

/// One fact of a compact metric section: its label, its value as the section
/// draws it, and the reason that value is not a measurement — or is only a bound
/// on one — when there is a reason to give.
pub(super) struct Metric {
    label: &'static str,
    value: String,
    reason: Option<String>,
}

impl Metric {
    /// A fact drawn as it was read, with nothing qualifying it.
    fn stated(label: &'static str, value: String) -> Self {
        Self {
            label,
            value,
            reason: None,
        }
    }

    /// A measured value that carries a reason: `unavailable` with why nothing
    /// was measured, or a lower bound with why it is one.
    fn measured(label: &'static str, value: String, reason: Option<String>) -> Self {
        Self {
            label,
            value,
            reason,
        }
    }
}

/// A value this machine may not have measured, with the reason it did not.
///
/// The value stays `unavailable` and the reason travels beside it, so nothing
/// measured can be silently dropped and nothing unmeasured can read as a zero.
pub(super) fn measured_value(value: Option<String>, reason: &str) -> (String, Option<String>) {
    match value {
        Some(value) => (value, None),
        None => (unavailable(), Some(reason.to_string())),
    }
}

/// One metric section: a heading, then the facts given, at most two to a row.
///
/// Labels are as wide as the section's longest, so the values line up down the
/// section instead of drifting with the label above them. Two facts share a row
/// while both fit the narrowest panel a section is drawn in; a row that does not
/// gives each fact a line of its own, so no value is ever cut short and no label
/// is ever left on the line above its own number.
///
/// A value that could not be measured, or that covers only part of what it
/// names, keeps its place on the line and puts its reason on the next one,
/// aligned under the value it belongs to. A reader comparing two readings
/// compares the values, and a reason reads as a reason rather than as another
/// measurement. A fact that has a reason therefore never shares its line: the
/// reason belongs to one value, and a line drawing two of them has no way to say
/// which.
pub(super) fn metric_section(heading_text: &str, rows: &[Vec<Metric>]) -> Vec<Line<'static>> {
    let palette = theme::palette();
    let mut lines = vec![heading(heading_text)];
    for row in rows {
        // A row's labels are as wide as its own longest, so a row of two facts
        // spends no more of the panel on its labels than it has to: the width a
        // pair saves is the width the panel is being spent on.
        let label_width = row
            .iter()
            .map(|metric| metric.label.chars().count())
            .max()
            .unwrap_or(0);
        let cell_width =
            |metric: &Metric| label_width + METRIC_LABEL_GAP + metric.value.chars().count();
        let paired = row.len() > 1
            && row.iter().all(|metric| metric.reason.is_none())
            && row.iter().map(cell_width).sum::<usize>() + METRIC_FACT_GAP * (row.len() - 1)
                <= METRIC_PAIR_WIDTH;
        let drawn: Vec<&[Metric]> = if paired {
            vec![row.as_slice()]
        } else {
            row.iter().map(std::slice::from_ref).collect()
        };
        let reason_column = " ".repeat(label_width + METRIC_LABEL_GAP);
        for facts in drawn {
            let mut spans = Vec::new();
            for (index, metric) in facts.iter().enumerate() {
                if index > 0 {
                    spans.push(Span::raw(" ".repeat(METRIC_FACT_GAP)));
                }
                spans.push(Span::styled(
                    format!(
                        "{:<label_width$}{}",
                        metric.label,
                        " ".repeat(METRIC_LABEL_GAP)
                    ),
                    Style::new().fg(palette.subtle),
                ));
                spans.push(Span::raw(metric.value.clone()));
            }
            lines.push(Line::from(spans));
            for metric in facts {
                if let Some(reason) = &metric.reason {
                    lines.push(Line::from(vec![
                        Span::raw(reason_column.clone()),
                        Span::styled(reason.clone(), Style::new().fg(palette.muted)),
                    ]));
                }
            }
        }
    }
    lines
}

/// The scheduler state the kernel reported, as one short enumerable name.
///
/// It says only what the scheduler is doing with the process — a sleeping
/// process may be waiting on a socket or on nothing, and a zombie has already
/// exited — so it is the kernel's own state, never progress or a verdict on the
/// work underneath, and it carries no activity colour. The name is kept short
/// on purpose: this is the value that changes while a reader watches, and a
/// changing sentence reads as a changing claim.
pub(super) fn state_word(state: ProcessState) -> String {
    match state {
        ProcessState::Running => "running".into(),
        ProcessState::Sleeping => "sleeping".into(),
        ProcessState::DiskSleep => "disk wait".into(),
        ProcessState::Stopped => "stopped".into(),
        ProcessState::TracingStop => "traced".into(),
        ProcessState::Zombie => "zombie".into(),
        ProcessState::Dead => "dead".into(),
        ProcessState::Idle => "idle".into(),
        ProcessState::Other(letter) => format!("unknown ({})", sanitize(&letter.to_string())),
    }
}

/// A measured interval as a section draws it: a percentage of one CPU, which
/// work on more than one can carry past 100.
///
/// The base is not repeated on every value: it is a property of the measure
/// rather than of one reading, and stating it beside each number is what the
/// section's width is spent on instead of the numbers.
pub(super) fn cpu_percent(cpu: CpuPercent) -> String {
    format!("{:.1}%", cpu.hundredths() as f64 / 100.0)
}

/// Interval CPU, or nothing when this machine has no interval to measure.
///
/// Unavailable until two readings of the same incarnation make an interval. A
/// first reading, a counter that went backwards, no elapsed time and a failed
/// read all leave nothing to measure — which is not the same as zero, and is
/// not always the warm-up either. A measured idle interval is zero, and is
/// drawn as one.
pub(super) fn cpu_value(cpu: Option<CpuPercent>) -> Option<String> {
    cpu.map(cpu_percent)
}

/// A process's resident set, or nothing when this machine cannot convert the
/// kernel's page count to a size.
pub(super) fn rss_value(bytes: Option<u64>) -> Option<String> {
    bytes.map(size)
}

/// How many processes were observed beneath a root, as a section draws it: a
/// count, or nothing when none could be enumerated at all.
pub(super) fn descendant_count(observed: Option<u32>) -> Option<String> {
    observed.map(|count| match count {
        1 => "1 process".to_string(),
        count => format!("{count} processes"),
    })
}

/// A total as a section draws it: its value, or why it is not one. A partial
/// total keeps what it does cover, marked as the lower bound it is, and carries
/// the reason it is incomplete; an unknown one is unavailable, with the reason
/// no total could be made.
pub(super) fn total_value<T>(
    total: &Total<T>,
    show: impl Fn(&T) -> String,
) -> (String, Option<String>) {
    match total {
        Total::Complete(value) => (show(value), None),
        Total::Partial(value, reason) => (format!("≥{}", show(value)), Some(sanitize(reason))),
        Total::Unknown(reason) => (unavailable(), Some(sanitize(reason))),
    }
}

/// A byte count in the units a reader compares sizes in.
pub(super) fn size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    match bytes {
        gib if gib >= GIB => format!("{:.1} GiB", gib as f64 / GIB as f64),
        mib if mib >= MIB => format!("{:.1} MiB", mib as f64 / MIB as f64),
        kib if kib >= KIB => format!("{:.1} KiB", kib as f64 / KIB as f64),
        bytes => format!("{bytes} B"),
    }
}

pub(super) fn basis_label(basis: &RetentionBasis) -> &'static str {
    match basis {
        RetentionBasis::ShellForeground => "shell foreground",
        RetentionBasis::Unverified => "unverified",
    }
}

/// The process holding a pane's foreground, when the evidence names a
/// non-shell one: a shell, an inconclusive foreground or no evidence at all
/// leaves nothing running to name.
pub(super) fn foreground_pid(foreground: Option<&ForegroundEvidence>) -> Option<i32> {
    match foreground? {
        ForegroundEvidence::NonShell { pid, .. } => Some(*pid),
        _ => None,
    }
}

/// Detail-panel content for the selected row's page.
///
/// Every page begins with the row's own title, so a page always says which row
/// it is describing; what follows is the facts that page is about. Splitting
/// them keeps identity, the live PID and the agent's activity in front of the
/// long published text, and leaves every fact the panel used to draw reachable
/// on some page.
/// The lines of one page, and where among them each openable block's summary
/// was drawn: what the render needs to report a click back to the block it
/// landed on.
#[derive(Default)]
pub(super) struct PageLines {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) markers: Vec<(Disclosure, usize)>,
    /// The process rows this page drew, in the order they were drawn: each row's
    /// birth identity, the line it was drawn on, and whether the table can fold
    /// its branch. The render turns these into the rectangles the pointer and the
    /// cursor work from; a row is its identity, never the line it landed on.
    pub(super) process_rows: Vec<(ProcessIdentity, usize, bool)>,
}

impl PageLines {
    fn push(&mut self, line: Line<'static>) {
        self.lines.push(line);
    }

    fn extend(&mut self, lines: Vec<Line<'static>>) {
        self.lines.extend(lines);
    }

    /// Adds a block's summary line, and remembers where its marker went.
    fn push_block(&mut self, key: &Disclosure, summary: Line<'static>) {
        self.markers.push((key.clone(), self.lines.len()));
        self.lines.push(summary);
    }
}

/// The openable blocks of the page being drawn: which the page offers, which of
/// them are open, and which one the keyboard is on.
pub(super) struct Blocks<'a> {
    targets: Vec<Disclosure>,
    highlighted: Option<Disclosure>,
    app: &'a App,
    width: usize,
}

impl<'a> Blocks<'a> {
    fn new(app: &'a App, width: usize) -> Self {
        Self {
            targets: app.disclosures(),
            highlighted: app.disclosure_target(),
            app,
            width,
        }
    }

    /// The cells this page's content is drawn in.
    fn content_width(&self) -> usize {
        self.width
    }

    /// Whether the page offers this block, and whether it is open. `None` is
    /// content with nothing to open, which is drawn as it always was.
    fn open(&self, key: &Disclosure) -> Option<bool> {
        self.targets
            .contains(key)
            .then(|| self.app.disclosure_open(key))
    }

    /// A block's marker and label with no facts beside them: for a block whose
    /// content is only ever its body.
    fn marker_line(&self, key: &Disclosure, open: Option<bool>, label: &str) -> Line<'static> {
        let mut spans = Vec::new();
        if let Some(open) = open {
            spans.push(self.marker(key, open));
        }
        spans.push(Span::styled(
            label.to_string(),
            Style::new().fg(theme::palette().subtle),
        ));
        Line::from(spans)
    }

    /// The block's marker: closed or open, in the ink the keyboard's target
    /// takes so the block Enter would open is the one the eye finds.
    fn marker(&self, key: &Disclosure, open: bool) -> Span<'static> {
        let glyph = if open { "▾ " } else { "▸ " };
        let ink = if self.highlighted.as_ref() == Some(key) {
            Style::new()
                .fg(theme::palette().heading)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme::palette().subtle)
        };
        Span::styled(glyph, ink)
    }

    /// A block's summary line: its marker, its label and the facts that stay
    /// visible whether it is open or closed. `None` is content with nothing to
    /// open, which is drawn without a marker rather than with one that answers
    /// to nothing.
    fn summary(
        &self,
        key: &Disclosure,
        open: Option<bool>,
        label: &str,
        value: String,
    ) -> Line<'static> {
        let mut spans = Vec::new();
        if let Some(open) = open {
            spans.push(self.marker(key, open));
        }
        spans.push(Span::styled(
            format!("{label}: "),
            Style::new().fg(theme::palette().subtle),
        ));
        spans.push(Span::raw(value));
        Line::from(spans)
    }
}

/// The opening of a long text: enough to recognise it, short enough to leave the
/// panel to the facts around it.
pub(super) const DIGEST_LIMIT: usize = 60;

pub(super) fn digest(text: &str) -> String {
    if text.chars().count() <= DIGEST_LIMIT {
        return text.to_string();
    }
    let kept: String = text.chars().take(DIGEST_LIMIT).collect();
    format!("{kept}…")
}

/// The page's lines, and the block each marker among them belongs to.
///
/// `selection` is the row of the process table the reader has selected, when one
/// is: the processes page describes that descendant, and every other page draws
/// its own row's facts. See [`process_lines`].
pub(super) fn detail_page_lines(
    state: &ObservationState,
    app: &App,
    page: DetailPage,
    width: u16,
    selection: Option<&ProcessIdentity>,
) -> PageLines {
    let mut page_lines = PageLines::default();
    let Some(row) = app.selected_row() else {
        page_lines.push(Line::from("no row selected"));
        return page_lines;
    };
    page_lines.push(Line::from(Span::styled(
        sanitize(row.node.row.title()),
        Style::new().add_modifier(Modifier::BOLD),
    )));
    let blocks = Blocks::new(app, width as usize);
    match page {
        DetailPage::Overview => overview_lines(state, &row, &blocks, &mut page_lines),
        DetailPage::Processes => process_lines(state, &row, &blocks, selection, &mut page_lines),
        DetailPage::Tasks => task_page_lines(&row, &blocks, &mut page_lines),
        DetailPage::Source => page_lines.extend(source_lines(state, &row)),
    }
    // The bus is a subsystem beside the source rather than part of it, so its
    // failure is written on every page: nothing here depends on it, and a
    // reader should not have to find a page to learn it is down.
    if let Some(line) = task_diagnostic(app) {
        page_lines.push(line);
    }
    page_lines
}

/// What Radar records about the container a row shows: its own identity, whether
/// it made the place, and the handle it is bound to.
///
/// An unfenced or unresolved binding is stated as it is: the handle is evidence,
/// and a row without a container is a place this daemon records nothing for.
pub(super) fn container_fields(
    container: Option<&crate::tree::ContainerRef>,
    page: &mut PageLines,
) {
    use crate::control_plane::containers::{Kind, Provenance};
    let Some(container) = container else {
        return;
    };
    page.push(field("container", container.container_id.clone()));
    page.push(field(
        "place",
        format!(
            "{} ({})",
            container.handle,
            match container.kind {
                Kind::Workspace => "workspace",
                Kind::Tab => "tab",
                Kind::Pane => "pane",
            }
        ),
    ));
    page.push(field(
        "owner",
        match container.provenance {
            Provenance::Created => "created by radar".to_string(),
            Provenance::Observed => "observed — not created by radar".to_string(),
        },
    ));
    if container.recipe {
        page.push(field("recipe", "held".to_string()));
    }
    if let Some(unresolved) = &container.unresolved {
        page.push(field("unresolved", sanitize(unresolved)));
    }
}

/// Overview: what this row is, in the compact form: location, the live PID and
/// the agent's activity and model, with the published assignment and what it
/// awaits kept to a summary. The command, the age and the measurements of the
/// process are the Processes page's.
pub(super) fn overview_lines(
    state: &ObservationState,
    row: &VisibleRow<'_>,
    blocks: &Blocks<'_>,
    page: &mut PageLines,
) {
    match &row.node.row.kind {
        RowKind::Workspace {
            workspace_id,
            label,
            number,
            container,
        } => {
            page.push(field("kind", "workspace".to_string()));
            page.push(field(
                "workspace",
                location_text(label.as_deref(), workspace_id),
            ));
            // A backend that reports no number has none to show: tmux names a
            // session and numbers nothing, so the field is left out rather than
            // drawn as a fact nothing could read.
            if let Some(number) = number {
                page.push(field("number", number.to_string()));
            }
            container_fields(container.as_ref(), page);
        }
        RowKind::Pane(pane) => {
            page.push(field(
                "kind",
                match pane.exited {
                    Some(_) => "pane (finished session)".to_string(),
                    None => "pane".to_string(),
                },
            ));
            page.push(field(
                "state",
                match &pane.exited {
                    Some(_) => "exited — no agent reported on this pane".to_string(),
                    None => unavailable(),
                },
            ));
            page.push(field(
                "location",
                location_line(
                    (pane.workspace_label.as_deref(), &pane.workspace_id),
                    (pane.tab_label.as_deref(), &pane.tab_id),
                    &pane.pane_id,
                ),
            ));
            if let Some(session) = &pane.exited {
                page.push(field("session", sanitize(&session.title)));
                // The row is the session now, so this is what the pane is
                // instead of it.
                page.push(field("pane now", sanitize(&pane.title)));
            }
            container_fields(pane.container.as_ref(), page);
            if let Some(line) = live_pid(state, pane.foreground.as_ref()) {
                page.push(line);
            }
            page.push(field("agent", unavailable()));
        }
        RowKind::Agent(agent) => {
            page.push(field(
                "kind",
                match &agent.retained {
                    Some(basis) => format!("agent (retained — {})", basis_label(basis)),
                    None => "agent (current)".to_string(),
                },
            ));
            page.push(field(
                "location",
                location_line(
                    (agent.workspace_label.as_deref(), &agent.workspace_id),
                    (agent.tab_label.as_deref(), &agent.tab_id),
                    &agent.pane_id,
                ),
            ));
            // A retained row has no live process, so it names no PID.
            if agent.retained.is_none()
                && let Some(line) = live_pid(state, agent.foreground.as_ref())
            {
                page.push(line);
            }
            page.push(field("agent", registration_identifier(agent)));
            container_fields(agent.container.as_ref(), page);
            match &agent.registry {
                Some(view) => registry_lines(state, agent, view, page),
                None => {
                    let status = agent
                        .status
                        .as_ref()
                        .map(|status| sanitize(&status.to_string()))
                        .unwrap_or_else(unavailable);
                    // The row draws the state derived from the pane; how the owner
                    // projects it, and the identities it published, are Source facts.
                    page.push(field(
                        "state",
                        format!("{} (derived from the pane)", sanitize(agent.state.word())),
                    ));
                    page.push(match &agent.retained {
                        Some(_) => field("status (last observed)", status),
                        None => field("status", status),
                    });
                }
            }
            activity_lines(&agent.facts, blocks, page);
        }
        RowKind::Lifecycle(lifecycle) => {
            let subject = &lifecycle.subject;
            page.push(field("kind", "unjoined lifecycle subject".to_string()));
            page.push(field("subject", subject.subject_id.clone()));
            page.push(field(
                "state",
                format!("{:?}", subject.state).to_lowercase(),
            ));
            page.push(field(
                "binding",
                subject
                    .bound_registration_id
                    .as_deref()
                    .map(|agent| format!("unresolved registration {agent}"))
                    .unwrap_or_else(|| "awaiting registration".to_string()),
            ));
            if let Some(process) = &subject.process {
                page.push(field("captured pid", process.pid.to_string()));
                page.push(field(
                    "process start ticks",
                    process.start_ticks.to_string(),
                ));
            } else {
                page.push(field("captured process", unavailable()));
            }
            page.push(field(
                "session",
                subject.session.clone().unwrap_or_else(unavailable),
            ));
        }
        RowKind::Task(task) => {
            page.push(field("kind", "background task".to_string()));
            page.push(field("task", sanitize(&task.id.id)));
            // A phase the pane or publisher reported is shown as reported; a
            // token entry naming only an id says nothing about a phase, so the
            // field is absent rather than drawn "unavailable".
            if let Some(phase) = task.phase.as_deref() {
                page.push(field("phase", sanitize(phase)));
            }
            page.push(field("source", task.basis().to_string()));
            page.push(field("owner", sanitize(&task.id.owner)));
            if let Some(session) = &task.id.session {
                page.push(field("session", sanitize(session)));
            }
        }
    }
}

/// What names this row's agent.
///
/// A backend's own agent name is used where it has one. A registry-backed row
/// has none, and that absence says nothing about the agent: the registration's
/// label, else the agent id it was registered under, names it instead. Reporting
/// both as `unavailable` would hide a registration the row is drawn from.
pub(super) fn registration_identifier(agent: &AgentRow) -> String {
    if let Some(name) = agent.name.as_deref() {
        return sanitize(name);
    }
    if let Some(view) = agent.registry.as_ref() {
        return match view.agent.registration.label.as_deref() {
            Some(label) => sanitize(label),
            None => sanitize(&view.agent.registration.registration_id),
        };
    }
    unavailable()
}

/// A registered agent's authorities, one labelled line each: where its pane
/// stands, what it last executed, what its owner assigned, its own facts, and the
/// state derived from them. Each carries its own freshness, so an expired report
/// reads as stale rather than as the state it last described.
pub(super) fn registry_lines(
    state: &ObservationState,
    agent: &AgentRow,
    view: &RegistryView,
    page: &mut PageLines,
) {
    let observed = |text: String| match &agent.retained {
        Some(_) => format!("{text} (last observed)"),
        None => text,
    };
    page.push(field(
        "physical",
        physical_text(&view.binding, agent.retained.is_some()),
    ));
    page.push(field(
        "execution",
        observed(with_source(
            execution_text(agent, &view.agent.execution),
            &view.agent.execution,
        )),
    ));
    page.push(field(
        "assignment",
        observed(with_source(
            assignment_text(agent, &view.agent.assignment),
            &view.agent.assignment,
        )),
    ));
    if let Some(reason) = agent.facts.waiting_reason.as_deref() {
        page.push(field("waiting reason", sanitize(reason)));
    }
    if let Some(outcome) = agent.facts.last_outcome.as_deref() {
        page.push(field("last outcome", sanitize(outcome)));
    }
    page.push(field(
        "facts",
        with_source(
            freshness_word(channel_freshness(&view.agent.facts)).to_string(),
            &view.agent.facts,
        ),
    ));
    page.push(field(
        "state",
        observed(format!(
            "{} ({})",
            sanitize(agent.state.word()),
            state_basis(&agent.state)
        )),
    ));
    if let Some(spawn) = &view.spawn {
        page.push(field("spawn", spawn_text(state, spawn)));
    }
}

/// Whether the pane is proven. A retained agent has no registration on its pane
/// now, and an unresolved one names the reason it does not join.
pub(super) fn physical_text(binding: &Binding, retained: bool) -> String {
    if retained {
        return "retained — no registration on this pane now".to_string();
    }
    match binding {
        Binding::Joined => "joined to this pane by process identity".to_string(),
        Binding::Unjoined(reason) => format!("unresolved — {}", unjoined_text(*reason)),
    }
}

pub(super) fn unjoined_text(reason: Unjoined) -> &'static str {
    match reason {
        Unjoined::NoLocation => "no pane reported",
        Unjoined::NoClaim => "no process identity claimed",
        Unjoined::PaneAbsent => "reported pane is not in the inventory; no exit is claimed",
        Unjoined::LocationMismatch => "reported backend, workspace or tab differs from the pane",
        Unjoined::ForegroundUnreadable => "the pane's foreground identity was not read",
        Unjoined::IdentityMismatch => "the pane's foreground is a different process",
        Unjoined::Ambiguous => "another registration claims the same process",
    }
}

/// The execution report, shown only while fresh: an expired or missing report is
/// never read as idle.
pub(super) fn execution_text(agent: &AgentRow, channel: &Option<PublicChannelFacts>) -> String {
    match channel_freshness(channel) {
        Some(Freshness::Fresh) => match &agent.status {
            Some(status) => sanitize(&status.to_string()),
            None => "unknown — report unreadable".to_string(),
        },
        Some(Freshness::Stale) => "stale — last report expired".to_string(),
        None => "unknown — nothing reported".to_string(),
    }
}

/// The owner's assignment report, shown only while fresh.
pub(super) fn assignment_text(agent: &AgentRow, channel: &Option<PublicChannelFacts>) -> String {
    match channel_freshness(channel) {
        Some(Freshness::Fresh) => match &agent.facts.state {
            Some(projected) => sanitize(AgentState::from(projected).word()),
            None => "reported without a state".to_string(),
        },
        Some(Freshness::Stale) => "stale — last report expired".to_string(),
        None => "unknown — nothing reported".to_string(),
    }
}

/// A report's publisher beside its freshness: the source that wrote it and, when
/// the registry names one, the owner it wrote for. An absent report names none.
pub(super) fn with_source(text: String, channel: &Option<PublicChannelFacts>) -> String {
    let Some(snapshot) = channel
        .as_ref()
        .and_then(|channel| channel.snapshot.as_ref())
    else {
        return text;
    };
    match &snapshot.reporting_owner {
        Some(owner) => format!(
            "{text} · from {} for {}",
            sanitize(&snapshot.source),
            sanitize(owner)
        ),
        None => format!("{text} · from {}", sanitize(&snapshot.source)),
    }
}

pub(super) fn freshness_word(freshness: Option<Freshness>) -> &'static str {
    match freshness {
        Some(Freshness::Fresh) => "fresh",
        Some(Freshness::Stale) => "stale",
        None => "not reported",
    }
}

/// Which authority a derived state comes from, so a reader can check it.
pub(super) fn state_basis(state: &AgentState) -> &'static str {
    match state {
        AgentState::Lost => "the assignment reports lost",
        AgentState::Settling => "the assignment reports settling",
        AgentState::Waiting => "idle with awaited items or background tasks",
        AgentState::Unknown => "execution unknown or unreported",
        _ => "from the execution report",
    }
}

/// The exact spawn edge: the parent is named by its registration when it is in
/// this view, and by its id alone when it is not. A child's edge is never a
/// process exit, so an unresolved edge is stated as that.
pub(super) fn spawn_text(state: &ObservationState, spawn: &SpawnBinding) -> String {
    let parent = state
        .inventory()
        .into_iter()
        .flat_map(|inventory| inventory.agents.iter())
        .filter_map(|agent| agent.registry.as_ref())
        .find(|view| view.agent.registration.registration_id == spawn.parent);
    let parent = match parent.and_then(|view| view.agent.registration.label.as_deref()) {
        Some(label) => format!("{} ({})", sanitize(label), sanitize(&spawn.parent)),
        None if parent.is_some() => sanitize(&spawn.parent),
        None => format!("{} (not in this view)", sanitize(&spawn.parent)),
    };
    let edge = match spawn.state {
        crate::control_plane::registry::SpawnState::Unbound => "not yet bound",
        crate::control_plane::registry::SpawnState::Bound => "bound",
        crate::control_plane::registry::SpawnState::Unresolved => "unresolved",
    };
    format!("child of {parent} ({edge})")
}

/// The row of the process table the reader has selected, when one is: the page
/// describes its own row's foreground unless a descendant's birth identity is
/// given here.
///
/// The rows the scan confirmed decide whether the process that was selected is
/// still there, so a process that vanished, or a pid the kernel handed to
/// another process, falls back to the root instead of drawing a stranger's facts
/// as the reader's own. Task 3.1 owns the cursor that produces one; until then
/// the page is drawn with none.
pub(super) fn selected_sample<'observation>(
    resources: &'observation ProcessResources,
    selection: Option<&ProcessIdentity>,
) -> Option<&'observation DescendantSample> {
    let selection = selection?;
    resources
        .descendants
        .members
        .iter()
        .find(|member| member.identity == *selection)
}

/// Processes: the process holding this row's location. A row whose process is
/// not currently observed says so rather than drawing last-good facts as live,
/// and a container row has no process to name.
///
/// The measurements come last, after the identity and command they belong to,
/// and they belong to this row's own process: a background task's published PID
/// is named without them, because a PID alone cannot authorise a reading of a
/// process that may have been replaced.
pub(super) fn process_lines(
    state: &ObservationState,
    row: &VisibleRow<'_>,
    blocks: &Blocks<'_>,
    selection: Option<&ProcessIdentity>,
    page: &mut PageLines,
) {
    let current = matches!(state.source_freshness(), SourceFreshness::Current);
    match &row.node.row.kind {
        RowKind::Workspace { .. } => page.push(field(
            "process",
            "none — a workspace holds panes, not a process".to_string(),
        )),
        RowKind::Pane(_) => {
            if !current {
                page.push(withheld("the source is not current"));
                return;
            }
            observed_process_lines(state, row, blocks, selection, page);
        }
        RowKind::Agent(agent) => {
            let unavailable_reason = match (&agent.retained, current) {
                (Some(_), _) => Some("this row is retained, not currently observed"),
                (None, false) => Some("the source is not current"),
                (None, true) => None,
            };
            if let Some(reason) = unavailable_reason {
                page.push(withheld(reason));
                return;
            }
            observed_process_lines(state, row, blocks, selection, page);
        }
        RowKind::Lifecycle(lifecycle) => {
            page.push(field(
                "process evidence",
                lifecycle
                    .subject
                    .process
                    .as_ref()
                    .map(|process| format!("captured pid {}", process.pid))
                    .unwrap_or_else(unavailable),
            ));
            page.push(field(
                "sample",
                "not a joined agent process; no live metrics are attributed".to_string(),
            ));
        }
        RowKind::Task(task) => {
            let published = task.published.as_ref();
            page.push(match published.and_then(|published| published.pid) {
                Some(pid) => field("pid", pid.to_string()),
                None => field("pid", unavailable()),
            });
            page.push(field("source", task.basis().to_string()));
            // A task's PID arrives without the identity of the process it
            // names, so no sample can be attributed to it: the publisher's
            // captured-at-spawn birth identity is still outstanding.
            page.push(field(
                "metrics",
                "unavailable — the publisher sends no process birth identity".to_string(),
            ));
        }
    }
}

/// The body of a Processes page whose row has a process that is current: what
/// the page describes, the runtime's own facts about the foreground process, its
/// measurements and the qualified sums of what is beneath it.
///
/// The page describes its own row's foreground unless the reader has selected a
/// descendant, and then the selection's own sample names and measures it, above
/// the root's. The binary comparison is made from the root's executable and from
/// nothing else, so a page drawing a child draws it under a heading of the
/// root's own: a descendant's sample says nothing about which build the root
/// runs.
pub(super) fn observed_process_lines(
    state: &ObservationState,
    row: &VisibleRow<'_>,
    blocks: &Blocks<'_>,
    selection: Option<&ProcessIdentity>,
    page: &mut PageLines,
) {
    let foreground = match &row.node.row.kind {
        RowKind::Pane(pane) => pane.foreground.as_ref(),
        RowKind::Agent(agent) => agent.foreground.as_ref(),
        _ => None,
    };
    // The name the runtime reports for the running process, and the sample the
    // scan took of it. The table's first row carries the name the `observed`
    // line above it does, so a reader reads one process in both.
    let (root_name, resources) = match foreground {
        Some(ForegroundEvidence::NonShell {
            name,
            command,
            local,
            ..
        }) => (
            observed_name(name.as_deref(), command.as_deref()),
            local.resources.as_ref(),
        ),
        // A shell, an inconclusive foreground and no evidence at all name no
        // process, and the identity lines below say which of them it is.
        _ => (unavailable(), None),
    };
    let selected = resources.and_then(|resources| selected_sample(resources, selection));
    if let Some(sample) = selected {
        // The reader's row, from its own sample and from nothing else: the
        // kernel's name for it, its own incarnation and the figures read for it.
        // Its name is the kernel's `comm`, never the arguments it was started
        // with and never a claim about what started it.
        page.push(field("pid", sample.identity.pid.to_string()));
        page.push(field("observed", sanitize(&sample.name)));
        page.extend(process_section(sample.state, sample.cpu, sample.rss_bytes));
        page.push(heading("foreground root"));
    }
    if let Some(line) = live_pid(state, foreground) {
        page.push(line);
    }
    // A live agent's running executable and the comparison made from it:
    // machine facts about the process, not the agent.
    identity_lines(foreground, blocks, page);
    if let RowKind::Pane(pane) = &row.node.row.kind {
        page.push(field("terminal", terminal_line(pane.terminal())));
        page.push(field(
            "running for",
            pane.running_for().map(duration).unwrap_or_else(unavailable),
        ));
    }
    match resources {
        Some(resources) => {
            // A picked child's page is drawn above the root's own: with the table
            // open, the root's own figures are its first row, and are not drawn a
            // second time here. The sums are not in the table and stay.
            if selected.is_none() || blocks.open(&Disclosure::ProcessTable) != Some(true) {
                page.extend(process_section(
                    resources.state,
                    resources.cpu,
                    resources.rss_bytes,
                ));
            }
            page.extend(descendant_section(resources));
        }
        None => page.push(field(
            "metrics",
            format!(
                "{} — this refresh took no sample of this process",
                unavailable()
            ),
        )),
    }
    process_table(row, &root_name, blocks, page);
}

/// Tasks: the background work this row knows about. The list, the pane's own
/// tokens and the publisher's facts stay together, so a count can be read
/// against the list it counts. A task's long text is behind its own disclosure,
/// with the identity and state it belongs to always drawn.
pub(super) fn task_page_lines(row: &VisibleRow<'_>, blocks: &Blocks<'_>, page: &mut PageLines) {
    match &row.node.row.kind {
        RowKind::Agent(agent) => {
            // The pane's raw token list is drawn only while it is still this
            // row's task source: a matched publisher's list is authoritative,
            // and printing both would contradict the rows beneath this one.
            let bus_authoritative = matches!(agent.tasks.source, Some(TaskSource::Bus));
            let before = page.lines.len();
            task_lines(agent, blocks, page);
            page.extend(task_fact_lines(&agent.facts, !bus_authoritative));
            if page.lines.len() == before {
                page.push(field("tasks", "none published".to_string()));
            }
        }
        RowKind::Task(task) => {
            let key = Disclosure::Task(task.id.clone());
            let open = blocks.open(&key);
            let summary = blocks.summary(&key, open, "task", task_row_summary(task));
            match open {
                Some(_) => page.push_block(&key, summary),
                None => page.push(summary),
            }
            match task.published.as_ref() {
                // A publisher that sent no detail for the task is a fact of its
                // own, and the page names it rather than drawing nothing.
                None => page.push(field(
                    "tasks",
                    "none — the publisher sent no detail for this task".to_string(),
                )),
                Some(published) => {
                    let now = now_unix_ms();
                    let mut detail = Vec::new();
                    // A command and a directory are the two fields that wrap
                    // over the panel, so they are what the disclosure holds; the
                    // measures below are one line each and stay drawn, because an
                    // exit code behind a marker is not one a reader can compare
                    // down the list.
                    if open != Some(false) {
                        if let Some(command) = published.command.as_deref() {
                            detail.push(field("command", bound_text(command)));
                        }
                        if let Some(cwd) = published.cwd.as_deref() {
                            detail.push(field("cwd", bound_text(cwd)));
                        }
                    }
                    if let Some(started) = published.started_at {
                        detail.push(field(
                            "started",
                            format!("{} ago", duration(elapsed(started, now))),
                        ));
                    }
                    if let Some(last_output) = published.last_output_at {
                        detail.push(field(
                            "last output",
                            format!("{} ago", duration(elapsed(last_output, now))),
                        ));
                    }
                    if let Some(bytes) = published.output_bytes {
                        detail.push(field("output", format!("{bytes} B")));
                    }
                    if let Some(code) = published.exit_code {
                        detail.push(field("exit", code.to_string()));
                    }
                    page.extend(detail);
                }
            }
        }
        RowKind::Pane(_) => page.push(field(
            "tasks",
            "none — this pane reports no agent".to_string(),
        )),
        RowKind::Workspace { .. } => page.push(field(
            "tasks",
            "none — tasks are published per pane".to_string(),
        )),
        RowKind::Lifecycle(_) => page.push(field(
            "tasks",
            "not available until this subject joins an agent registration".to_string(),
        )),
    }
}

/// A task row's own line on the Tasks page: what the block is, so the fields
/// behind its disclosure have something to hang from.
pub(super) fn task_row_summary(task: &TaskRow) -> String {
    match task.phase.as_deref() {
        Some(phase) => format!("{} · {}", sanitize(&task.id.id), sanitize(phase)),
        None => sanitize(&task.id.id),
    }
}

/// Source: how current the facts are, where this row's identity comes from, and
/// what the owner published about it — its projected state, the run, request and
/// ask it names, and the session behind them.
pub(super) fn source_lines(state: &ObservationState, row: &VisibleRow<'_>) -> Vec<Line<'static>> {
    let mut lines = vec![detail_freshness(state, row)];
    if let RowKind::Agent(agent) = &row.node.row.kind {
        if let Some(projected) = &agent.facts.state {
            lines.push(field(
                "projection",
                format!(
                    "{} (owner projection)",
                    sanitize(AgentState::from(projected).word())
                ),
            ));
        }
        lines.push(field(
            "identity",
            session_text(agent.session.as_ref()).unwrap_or_else(unavailable),
        ));
        lines.extend(owner_identity_lines(&agent.facts));
        // The lineage the tree placed this row by. A retained row keeps its
        // observation, so its link stays readable after the agent goes away.
        let observation = state
            .inventory()
            .and_then(|inventory| inventory.agent_on_pane(&agent.pane_id))
            .or_else(|| {
                state
                    .retained()
                    .get(agent.pane_id.as_str())
                    .map(|retained| &retained.observation)
            });
        if let Some(observation) = observation {
            lines.extend(lineage_lines(state, observation));
        }
    }
    lines
}

/// The line a page draws when the facts it holds are not this row's to claim
/// right now, so a withheld value is never read as an absent one.
pub(super) fn withheld(reason: &str) -> Line<'static> {
    field("process", format!("withheld — {reason}"))
}

/// The live PID line for a row whose process was queried, when one is known.
///
/// A stale inventory's last-good evidence is not a process known to be running
/// now, so no PID is drawn from one.
pub(super) fn live_pid(
    state: &ObservationState,
    foreground: Option<&ForegroundEvidence>,
) -> Option<Line<'static>> {
    if !matches!(state.source_freshness(), SourceFreshness::Current) {
        return None;
    }
    foreground_pid(foreground).map(|pid| field("pid", pid.to_string()))
}

/// The row's foreground: the process's own command name as the runtime reports
/// it, the executable the kernel is running, and how that executable compares
/// with the program installed now.
///
/// The name is the runtime's report of a running process — for a bolt pane the
/// exec-replaced payload — so nothing here says what the pane was started with,
/// and the name is never taken for a launcher or an alias. The process's
/// arguments are not drawn at all: a harness takes its prompt and system prompt
/// as arguments, and nothing the runtime reports tells a prompt from a flag. A
/// shell or an inconclusive answer is not a process to name, and is stated
/// instead.
///
/// The page draws this identity short — the package once, the executable inside
/// it, a verdict only where there is one — and holds the long halves of the same
/// facts behind the page's block: the roots a comparison was made between, the
/// whole path, the birth identity, what a state means and what a sum is. A row
/// with no process draws none of it and is offered no block.
pub(super) fn identity_lines(
    foreground: Option<&ForegroundEvidence>,
    blocks: &Blocks<'_>,
    page: &mut PageLines,
) {
    let Some(ForegroundEvidence::NonShell {
        name,
        command,
        local,
        ..
    }) = foreground
    else {
        page.push(match foreground {
            Some(ForegroundEvidence::Shell) => field(
                "foreground",
                "shell — nothing in the foreground".to_string(),
            ),
            Some(ForegroundEvidence::Inconclusive) => {
                field("foreground", "unknown — PID fields disagree".to_string())
            }
            _ => field("foreground", unavailable()),
        });
        return;
    };
    page.push(field(
        "observed",
        observed_name(name.as_deref(), command.as_deref()),
    ));
    let binary = &local.binary;
    if let Some(running) = binary.running.as_deref() {
        page.push(field(
            "package",
            package_text(running, binary.installed.as_deref()),
        ));
    }
    // The executable within the package that carries it: the store root is
    // named by the line above, and its hash twice on one page is a hash a
    // reader has to compare with itself. A file outside that root is drawn
    // whole, and one that is the root says nothing the line above did not.
    if let Some(executable) = binary.executable.as_deref() {
        let executable = sanitize(executable);
        let running = binary.running.as_deref().map(sanitize);
        match relative_to(&executable, running.as_deref()) {
            Some(relative) => page.push(field("exe", relative.to_string())),
            None if Some(&executable) == running.as_ref() => {}
            None => page.push(field("exe", executable)),
        }
    }
    if let Some(verdict) = verdict_line(binary) {
        page.push(verdict);
    }
    let key = Disclosure::Process;
    if let Some(open) = blocks.open(&key) {
        page.push_block(&key, blocks.marker_line(&key, Some(open), "details"));
        if open {
            page.extend(process_detail_lines(foreground, blocks.content_width()));
        }
    }
}

/// The verbose half of a live process's identity, behind the page's block.
///
/// The two roots are the sides the comparison was made between, drawn whole so
/// the hash on the package line above can be read against the build it names.
/// The whole executable path is the one the relative form was cut from, and the
/// birth identity is what a sample belongs to: a PID alone is a number the
/// machine reuses, and the boot id and start ticks are what tell two of them
/// apart. The state letters travel as one word each, so what the kernel meant by
/// the one on the page is here, and a descendant sum is what it is only with
/// the sentence that says so.
pub(super) fn process_detail_lines(
    foreground: Option<&ForegroundEvidence>,
    width: usize,
) -> Vec<Line<'static>> {
    let Some(ForegroundEvidence::NonShell { local, .. }) = foreground else {
        return Vec::new();
    };
    let binary = &local.binary;
    let running = binary.running.as_deref().map(sanitize);
    let installed = binary.installed.as_deref().map(sanitize);
    let mut rows = Vec::new();
    // The roots, and the prefix they share stated once: two whole store paths
    // repeat a store, a name and a version on one panel, and the part that says
    // which build is which is the part past the prefix.
    let mut stated: Option<String> = None;
    match (running.as_deref(), installed.as_deref()) {
        (Some(running), Some(installed)) if running == installed => {
            rows.push(BlockRow::new("root", running.to_string()));
        }
        (Some(running), Some(installed)) => match shared_prefix(running, installed) {
            Some(prefix) => {
                rows.push(BlockRow::new("store", prefix.clone()));
                rows.push(BlockRow::new(
                    "running",
                    running[prefix.len()..].to_string(),
                ));
                rows.push(BlockRow::new(
                    "installed",
                    installed[prefix.len()..].to_string(),
                ));
                stated = Some(prefix);
            }
            None => {
                rows.push(BlockRow::new("running", running.to_string()));
                rows.push(BlockRow::new("installed", installed.to_string()));
            }
        },
        (Some(running), None) => rows.push(BlockRow::new("running", running.to_string())),
        (None, Some(installed)) => rows.push(BlockRow::new("installed", installed.to_string())),
        (None, None) => {}
    }
    if let Some(executable) = binary.executable.as_deref() {
        let executable = sanitize(executable);
        let roots = [running.as_deref(), stated.as_deref()];
        let relative = roots
            .iter()
            .flatten()
            .find_map(|root| relative_to(&executable, Some(root)));
        // A file that *is* the root says nothing the root's own row did not.
        let is_a_root = roots
            .iter()
            .flatten()
            .any(|root| *root == executable.as_str());
        match (relative, is_a_root) {
            (Some(relative), _) => rows.push(BlockRow::new("exe", relative.to_string())),
            (None, true) => {}
            (None, false) => rows.push(BlockRow::new("exe", executable)),
        }
    }
    if binary.freshness == BinaryFreshness::Unknown
        && let Some(reason) = binary.unknown
    {
        rows.push(BlockRow::new("unknown", unknown_reason(reason).to_string()));
    }
    if let Some(resources) = local.resources.as_ref() {
        let identity = &resources.identity;
        rows.push(BlockRow::new(
            "birth",
            format!(
                "pid {} · start ticks {}",
                identity.pid, identity.start_ticks,
            ),
        ));
        // The boot id gets a row of its own: a UUID is longer than any column a
        // half-width panel gives a value, and it reads better hyphen by hyphen
        // than mashing three short facts together to make room for it.
        rows.push(BlockRow::new("boot", sanitize(&identity.boot_id)));
        // The meaning of the state this row is showing, named where the row's
        // own word is drawn: the eight other kernel states are the kernel's,
        // and a list of them described is a paragraph where a fact belongs.
        rows.push(BlockRow::new(
            "state",
            format!(
                "{} — {}",
                state_word(resources.state),
                state_meaning(resources.state)
            ),
        ));
    }
    rows.push(BlockRow::new("note", DESCENDANT_SUM.to_string()));
    block_lines(&rows, width)
}

/// One labelled row of a block: the label, and the value drawn after the column
/// every row of that block shares.
pub(super) struct BlockRow {
    label: &'static str,
    text: String,
}

impl BlockRow {
    fn new(label: &'static str, text: String) -> Self {
        Self { label, text }
    }
}

/// Draws a block's rows: one label column, the value beside its label, and a
/// value too long for the panel continued under that same column rather than
/// under the label it belongs to.
pub(super) fn block_lines(rows: &[BlockRow], width: usize) -> Vec<Line<'static>> {
    let column = rows.iter().map(|row| row.label.len()).max().unwrap_or(0);
    let indent = column + 1;
    let value_width = width.saturating_sub(indent).max(MIN_VALUE_COLUMN);
    let mut lines = Vec::new();
    for row in rows {
        for (index, part) in wrap_value(&row.text, value_width).into_iter().enumerate() {
            let lead = if index == 0 {
                format!("{:<column$} ", row.label)
            } else {
                " ".repeat(indent)
            };
            lines.push(Line::from(vec![
                Span::styled(lead, Style::new().fg(theme::palette().subtle)),
                Span::raw(part),
            ]));
        }
    }
    lines
}

/// The label of the block the process table is held behind, where the page's
/// other block is labelled `details` and says only that it holds more.
pub(super) const TABLE_LABEL: &str = "process table";

/// The label while the reader is moving through the table's rows: the mode is
/// local to this page, and the page names it where the table is named.
pub(super) const TABLE_LABEL_NAVIGATING: &str = "process table · navigating";

/// The cells the table keeps at the start of every row for its fold marker: the
/// glyph that says whether a branch is open or folded, and the room beside it —
/// the same two-cell affordance the page's other markers are clicked by. A leaf's
/// marker is blank, so every row's name starts in the same cell.
pub(super) const TABLE_MARKER: usize = 2;

/// Cells between two columns of the process table.
pub(super) const TABLE_GAP: usize = 2;

/// Cells one level of a branch takes in the name's own column: the guide that
/// marks it, and the room beside it.
pub(super) const BRANCH_SEGMENT: usize = 3;

/// The narrowest name column the table draws before it gives up a metric column:
/// a branch guide and a few characters of a name. The name and the pid are what
/// identify a row and neither is given up; the metric columns go, widest first.
pub(super) const TABLE_NAME_MIN: usize = 6;

/// The cell the table draws for a value this machine did not measure. A table
/// has one cell for a reading and no room for the eleven of `unavailable`: a
/// dash is not a number and not a zero, and the table says what it means below
/// the rows it drew one in.
pub(super) const TABLE_UNMEASURED: &str = "—";

/// Draws the table: the columns it can name, one single-line row per process, and
/// what a dash means where one was drawn.
///
/// Every line is built to the panel's own width, so nothing here wraps into the
/// row below it: a name is shortened, and then the metric columns are dropped —
/// the resident set before the CPU — before a row is allowed past the panel's
/// edge. The branch guides are drawn inside the name's own column, so a deeper
/// tree spends the name's width rather than pushing the pid off the panel.
pub(super) fn process_table_lines(
    table: &ProcessTable,
    root_name: &str,
    width: usize,
    selected: &ProcessIdentity,
    folded: &HashSet<ProcessIdentity>,
    page: &mut PageLines,
) {
    let subtle = Style::new().fg(theme::palette().subtle);
    let cpu = |row: &ProcessRow| {
        row.cpu
            .map(cpu_percent)
            .unwrap_or_else(|| TABLE_UNMEASURED.to_string())
    };
    let rss = |row: &ProcessRow| {
        row.rss_bytes
            .map(size)
            .unwrap_or_else(|| TABLE_UNMEASURED.to_string())
    };
    // Each column is as wide as the widest thing drawn in it, its own heading
    // included: a tree whose pids are shorter than `pid` still lines its rows up
    // under the heading.
    let widest = |values: Vec<String>, heading: &str| {
        values
            .iter()
            .map(|value| display_cells(value))
            .max()
            .unwrap_or(0)
            .max(display_cells(heading))
    };
    let pid_width = widest(
        table
            .rows
            .iter()
            .map(|row| row.identity.pid.to_string())
            .collect(),
        "pid",
    );
    let cpu_width = widest(table.rows.iter().map(&cpu).collect(), "cpu");
    let rss_width = widest(table.rows.iter().map(&rss).collect(), "rss");
    // What the columns right of the name cost, given which of them the panel is
    // paying for. A panel too narrow for all of them gives up the widest fact
    // first, and keeps the name and the pid whatever happens.
    let tail = |with_cpu: bool, with_rss: bool| {
        TABLE_MARKER
            + pid_width
            + TABLE_GAP
            + if with_cpu { cpu_width + TABLE_GAP } else { 0 }
            + if with_rss { rss_width + TABLE_GAP } else { 0 }
    };
    let show_rss = width >= TABLE_NAME_MIN + tail(true, true);
    let show_cpu = width >= TABLE_NAME_MIN + tail(true, show_rss);
    let name_width = width.saturating_sub(tail(show_cpu, show_rss));
    // A value in a column of its own, as wide as the column: the spaces that
    // are left of it are counted in cells, so the value ends in the same cell in
    // every row whatever it says.
    let column = |value: &str, width: usize| {
        format!(
            "{:pad$}{value}",
            "",
            pad = width.saturating_sub(display_cells(value))
        )
    };
    // One row, from the cells it draws. The name column is filled to its own
    // width, so every column starts in the same cell whatever the name's length
    // or the width of its characters. The marker column is the caller's: it is
    // paid for out of the row, before the name.
    let cells = |branch: usize, name: &str, pid: &str, cpu: Option<&str>, rss: Option<&str>| {
        let name = shorten(name, name_width.saturating_sub(branch));
        let mut text = name.clone();
        let pad = name_width
            .saturating_sub(branch)
            .saturating_sub(display_cells(&name));
        text.push_str(&" ".repeat(pad));
        text.push_str(&column(pid, pid_width + TABLE_GAP));
        if let Some(cpu) = cpu {
            text.push_str(&column(cpu, cpu_width + TABLE_GAP));
        }
        if let Some(rss) = rss {
            text.push_str(&column(rss, rss_width + TABLE_GAP));
        }
        text
    };
    page.push(Line::from(Span::styled(
        format!(
            "{:marker$}{}",
            "",
            cells(
                0,
                "name",
                "pid",
                show_cpu.then_some("cpu"),
                show_rss.then_some("rss"),
            ),
            marker = TABLE_MARKER
        ),
        subtle,
    )));
    for row in table.visible() {
        // A branch is drawn out of the name's own cells, and the levels nearest
        // the process are the ones that say where it sits: a column too narrow
        // for the whole ancestry drops the outer guides — saying so with an
        // ellipsis — rather than drawing the row flush with the root's own
        // children. Every guide is one cell, so the levels are counted in
        // characters and the branch kept is at most the name's own width.
        let mut branch = String::new();
        for guide in &row.guides {
            branch.push_str(if *guide { "│  " } else { "   " });
        }
        if row.parent.is_some() {
            branch.push_str(if row.last { "└─ " } else { "├─ " });
        }
        let branch = if display_cells(&branch) < name_width {
            branch
        } else if name_width == 0 {
            // No cell for the guides and none for the ellipsis that says they
            // were left out: the row's own marker cell is all it has.
            String::new()
        } else {
            let levels = name_width.saturating_sub(1) / BRANCH_SEGMENT;
            let dropped = branch.chars().count() - levels * BRANCH_SEGMENT;
            let kept: String = branch.chars().skip(dropped).collect();
            format!("…{kept}")
        };
        let branch_width = display_cells(&branch);
        // The marker column: the fold of this row's branch, or a blank cell where
        // the row has no branch to fold. Both say the same thing about a process
        // beneath it, and neither names a command.
        let marker = match row.has_children {
            true if folded.contains(&row.identity) => "▸ ",
            true => "▾ ",
            false => "  ",
        };
        // The row's own process is named by the runtime, and every process under
        // it by the kernel's own read: both are external text, sanitized here
        // like everything else drawn from outside.
        let name = sanitize(row.name.as_deref().unwrap_or(root_name));
        let pid = row.identity.pid.to_string();
        // The reader's row is a band across the table: it is marked by the cells
        // it fills, which no name column has to pay for.
        let ink = |base: Style| match row.identity == *selected {
            true => Style::new()
                .bg(theme::palette().selection)
                .add_modifier(Modifier::BOLD),
            false => base,
        };
        let index = page.lines.len();
        page.process_rows
            .push((row.identity.clone(), index, row.has_children));
        page.push(Line::from(vec![
            Span::styled(format!("{marker}{branch}"), ink(subtle)),
            Span::styled(
                cells(
                    branch_width,
                    &name,
                    &pid,
                    show_cpu.then(|| cpu(row)).as_deref(),
                    show_rss.then(|| rss(row)).as_deref(),
                ),
                ink(Style::new()),
            ),
        ]));
    }
    let unmeasured = table
        .visible()
        .any(|row| (show_cpu && row.cpu.is_none()) || (show_rss && row.rss_bytes.is_none()));
    if unmeasured {
        page.push(Line::from(Span::styled(
            format!("{TABLE_UNMEASURED} not measured"),
            subtle,
        )));
    }
}

/// The cells a piece of text takes on the terminal, as Ratatui measures it for
/// layout: the characters of a name are not cells, so a name with characters
/// wider than one — or marks that take none — is measured by what will be drawn.
/// Everything this table aligns is measured here, because a column laid out in
/// characters shifts every cell after it.
pub(super) fn display_cells(text: &str) -> usize {
    Span::raw(text).width()
}

/// A name shortened to the cells a column gives it, keeping the front of it: a
/// process is recognised by how its name starts, and the pid beside it names the
/// row when the name is not enough. The ellipsis is paid for out of the column:
/// the characters kept are as many as leave room for it, and a character too
/// wide for what is left is dropped rather than cut.
pub(super) fn shorten(name: &str, width: usize) -> String {
    if display_cells(name) <= width {
        return name.to_string();
    }
    if width == 0 {
        // Not even the ellipsis has a cell to sit in.
        return String::new();
    }
    let mut kept = String::new();
    for ch in name.chars() {
        kept.push(ch);
        if display_cells(&kept) + 1 > width {
            kept.pop();
            break;
        }
    }
    kept.push('…');
    kept
}

/// The process table's block, where the page has a verified tree to draw: the
/// root's own row and a row for every process the same scan confirmed beneath it.
///
/// The table is offered only where the scan that sampled the root also enumerated
/// what is beneath it: an unreadable or budgeted scan is not a tree of nothing,
/// and the sums above already say it is not a measurement.
pub(super) fn process_table(
    row: &VisibleRow<'_>,
    root_name: &str,
    blocks: &Blocks<'_>,
    page: &mut PageLines,
) {
    let Some(table) = blocks.app.process_table(row) else {
        return;
    };
    let key = Disclosure::ProcessTable;
    let Some(open) = blocks.open(&key) else {
        return;
    };
    let label = match blocks.app.process_navigating() {
        true => TABLE_LABEL_NAVIGATING,
        false => TABLE_LABEL,
    };
    page.push_block(&key, blocks.marker_line(&key, Some(open), label));
    if open {
        let selected = blocks
            .app
            .process_selection()
            .unwrap_or(&table.root)
            .clone();
        process_table_lines(
            &table,
            root_name,
            blocks.content_width(),
            &selected,
            blocks.app.process_folded(),
            page,
        );
    }
}

/// The narrowest value column a block is wrapped to: past that, a fact is one
/// character per row and the wrapping costs more than the fact is worth.
pub(super) const MIN_VALUE_COLUMN: usize = 8;

/// Wraps `text` to `width` cells, at a space where one is near enough and inside
/// a word only when the word is longer than the whole line. The pieces come back
/// without indentation; the caller draws them in its value column.
pub(super) fn wrap_value(text: &str, width: usize) -> Vec<String> {
    let mut pieces: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split(' ') {
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + word.chars().count() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            pieces.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    if !current.is_empty() || pieces.is_empty() {
        pieces.push(current);
    }
    let mut lines = Vec::new();
    for piece in pieces {
        let mut rest = piece.as_str();
        loop {
            let count = rest.chars().count();
            if count <= width {
                lines.push(rest.to_string());
                break;
            }
            let split = split_word(rest, width);
            lines.push(rest[..split].to_string());
            rest = &rest[split..];
        }
    }
    lines
}

/// Where to cut a word that cannot fit the column whole: at the last separator
/// it can, so a store path breaks at a store and a boot id at a hyphen, and
/// inside a run of characters only when there is no separator to break at.
pub(super) fn split_word(word: &str, width: usize) -> usize {
    let boundary = word
        .char_indices()
        .filter(|(index, character)| *index < width && matches!(character, '/' | '-'))
        .map(|(index, character)| index + character.len_utf8())
        .next_back();
    boundary.unwrap_or_else(|| {
        word.char_indices()
            .nth(width)
            .map(|(index, _)| index)
            .unwrap_or(word.len())
    })
}

/// The prefix two paths share down to a component boundary, when each leaves
/// something past it: `/nix/store/aaa-pi` and `/nix/store/bbb-pi` share
/// `/nix/store/`, where two sibling files share their directory and not their
/// name.
pub(super) fn shared_prefix(a: &str, b: &str) -> Option<String> {
    let mut end = 0;
    for ((index, left), right) in a.char_indices().zip(b.chars()) {
        if left != right {
            break;
        }
        if left == '/' {
            end = index + 1;
        }
    }
    if end <= 1 || end >= a.len() || end >= b.len() {
        return None;
    }
    Some(a[..end].to_string())
}

/// A section heading: the word that says what the lines under it are.
pub(super) fn heading(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::new()
            .fg(theme::palette().heading)
            .add_modifier(Modifier::BOLD),
    ))
}

/// What the kernel's state letter means for the process that carries it, in the
/// page's words.
///
/// The letter says only what the scheduler is doing with the process: a sleeping
/// process may be waiting on a socket or on nothing, and a zombie has already
/// exited. One word of state is not readable without it.
pub(super) fn state_meaning(state: ProcessState) -> &'static str {
    match state {
        ProcessState::Running => "executing, or waiting its turn on a CPU",
        ProcessState::Sleeping => "waiting, but wakeable",
        ProcessState::DiskSleep => "blocked in the kernel, usually on I/O",
        ProcessState::Stopped => "stopped by a signal",
        ProcessState::TracingStop => "stopped because something is tracing it",
        ProcessState::Zombie => "exited, not yet reaped by its parent",
        ProcessState::Dead => "gone, or being torn down",
        ProcessState::Idle => "idle in the kernel, below the oldest run queue",
        ProcessState::Other(_) => "a state this kernel wrote that Radar does not name",
    }
}

/// The running process's command name as the runtime reports it, sanitized like
/// every other external text. A runtime that reports no name still reports the
/// command line, and its first word names the program the kernel is running.
pub(super) fn observed_name(name: Option<&str>, command: Option<&str>) -> String {
    let name = name
        .map(str::to_owned)
        .or_else(|| command.map(program_of).map(str::to_owned))
        .unwrap_or_else(unavailable);
    sanitize(&name)
}

/// Freshness for the selected row: a retained association is not current, and
/// a stale/unavailable source is stated on every row.
pub(super) fn detail_freshness(state: &ObservationState, row: &VisibleRow<'_>) -> Line<'static> {
    let retained = match &row.node.row.kind {
        RowKind::Agent(agent) => agent.retained.as_ref(),
        _ => None,
    };
    let source = match state.source_freshness() {
        SourceFreshness::Pending => "source: waiting for the first collection".to_string(),
        SourceFreshness::Current => "source: current".to_string(),
        SourceFreshness::Stale { diagnostic } => {
            format!("source: stale — {}", sanitize(diagnostic))
        }
        SourceFreshness::Unavailable { diagnostic } => {
            format!("source: unavailable — {}", sanitize(diagnostic))
        }
    };
    let text = match retained {
        // The basis is stated once, in `kind`: this line only has to say that
        // the facts are no longer current.
        Some(_) => format!("observation: retained, not currently observed; {source}"),
        None => format!("observation: current observation; {source}"),
    };
    let style =
        if retained.is_some() || !matches!(state.source_freshness(), SourceFreshness::Current) {
            Style::new().fg(theme::palette().retained)
        } else {
            Style::new().fg(theme::palette().done)
        };
    Line::from(Span::styled(text, style))
}

pub(super) fn field(label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label}: "),
            Style::new().fg(theme::palette().subtle),
        ),
        Span::raw(value),
    ])
}

/// What the agent is doing, for Overview: the facts Herdsman publishes about
/// it, each drawn only while the source publishes it — a fact nobody reported
/// is absent here, never a placeholder. The assignment is the one field long
/// enough to bury the rest, so it is drawn behind this page's disclosure.
///
/// Every value is text another process wrote — an assignment's display text
/// above all — so each is sanitized like any other runtime string.
pub(super) fn activity_lines(facts: &HerdsmanFacts, blocks: &Blocks<'_>, page: &mut PageLines) {
    if let Some(role) = facts.role.as_deref() {
        page.push(field("role", sanitize(role)));
    }
    // The backend's own display agent is where a worker's agent definition comes
    // from; it earns a line only when it says more than the role already does.
    if let Some(definition) = facts.definition.as_deref()
        && Some(definition) != facts.role.as_deref()
    {
        page.push(field("definition", sanitize(definition)));
    }
    if let Some(assignment) = facts.assignment.as_deref() {
        let key = Disclosure::Assignment;
        match blocks.open(&key) {
            Some(open) => {
                let text = sanitize(assignment);
                let value = if open { text } else { digest(&text) };
                let summary = blocks.summary(&key, Some(open), "assignment", value);
                page.push_block(&key, summary);
            }
            None => page.push(field("assignment", sanitize(assignment))),
        }
    }
    if let Some(assigned) = facts.assigned_for {
        page.push(field("assigned for", duration(assigned)));
    }
    if let Some(awaiting) = facts.awaiting() {
        page.push(field("awaiting", sanitize(&awaiting)));
    }
    if let Some(model) = facts.model.as_deref() {
        page.push(field("model", sanitize(model)));
    }
    if let Some(provider) = facts.provider.as_deref() {
        page.push(field("provider", sanitize(provider)));
    }
    if let Some(thinking) = facts.thinking.as_deref() {
        page.push(field("thinking", sanitize(thinking)));
    }
    // Counts, when a registry publisher gave them, are the better fact: the
    // percentage is derived from them. A token carries only the percentage.
    if let Some(window) = facts.context_window {
        page.push(field(
            "context usage",
            format!(
                "{}% ({} of {} tokens)",
                window.percent(),
                window.tokens,
                window.window
            ),
        ));
    } else if let Some(usage) = facts.context_usage.as_deref() {
        page.push(field("context usage", sanitize(usage)));
    }
}

/// The pane's own published background facts, for Tasks.
///
/// `tokens_listed` is false where a matched publisher's list is authoritative:
/// printing both would contradict the rows beneath the owner.
pub(super) fn task_fact_lines(facts: &HerdsmanFacts, tokens_listed: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if tokens_listed && !facts.background_tasks.is_empty() {
        lines.push(field(
            "background",
            sanitize(&facts.background_tasks.join(", ")),
        ));
    }
    // The publisher's own count speaks only for its running tasks; it is kept
    // beside the list so `0 running` with work outstanding reads as what it is.
    if let Some(running) = facts.background_running {
        lines.push(field("background running", running.to_string()));
    }
    if let Some(started) = facts.background_started.as_deref() {
        lines.push(field("background started", sanitize(started)));
    }
    lines
}

/// The identities the owner published for this row, for Source: what a reader
/// cross-checks against Herdsman's own records rather than against activity.
pub(super) fn owner_identity_lines(facts: &HerdsmanFacts) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(name) = facts.session_name.as_deref() {
        lines.push(field("session name", sanitize(name)));
    }
    if let Some(run) = facts.run.as_deref() {
        lines.push(field("run", sanitize(run)));
    }
    if let Some(request) = facts.request.as_deref() {
        lines.push(field("request", sanitize(request)));
    }
    if let Some(ask) = facts.ask.as_deref() {
        lines.push(field("ask", sanitize(ask)));
    }
    lines
}

/// This row's lineage, for Source: the session the owner published for it and the
/// parent it named, with what became of that link.
///
/// A worker that lands at a workspace root can be there because two panes
/// publish one session UUID, which makes its parent an ambiguity rather than an
/// owner. The tree does not fabricate a nesting, so the reason is said here
/// rather than left for a reader to infer from where the row landed.
pub(super) fn lineage_lines(
    state: &ObservationState,
    agent: &AgentObservation,
) -> Vec<Line<'static>> {
    let Some(lineage) = agent.lineage.as_ref() else {
        return Vec::new();
    };
    let mut lines = vec![field("session uuid", sanitize(lineage.session.as_str()))];
    if let Some(parent) = lineage.parent.as_ref() {
        lines.push(field("parent", parent_link(state, agent, parent)));
    }
    lines
}

/// What became of a parent link: the row it is nested under, or the reason the
/// tree left it where it is. The rule is the tree's own — one claimant in the
/// workspace, and never the row itself.
pub(super) fn parent_link(
    state: &ObservationState,
    agent: &AgentObservation,
    parent: &SessionUuid,
) -> String {
    let uuid = sanitize(parent.as_str());
    let owners: Vec<&AgentObservation> = state
        .inventory()
        .map(|inventory| {
            inventory
                .agents
                .iter()
                .filter(|other| {
                    other.location.workspace_id == agent.location.workspace_id
                        && other
                            .lineage
                            .as_ref()
                            .is_some_and(|lineage| &lineage.session == parent)
                })
                .collect()
        })
        .unwrap_or_default();
    match owners.as_slice() {
        [] => format!("{uuid} — no row publishes this session, so this row is not nested"),
        [owner] if owner.location.pane_id == agent.location.pane_id => {
            format!("{uuid} — this row's own session, so it is not nested")
        }
        [owner] => format!("{uuid} — nested under {}", owner_name(owner)),
        owners => format!(
            "{uuid} — {} rows publish this session, so this row is not nested",
            owners.len()
        ),
    }
}

/// The name an owner is linked by: the label or name it published, and the pane
/// that carries it.
pub(super) fn owner_name(agent: &AgentObservation) -> String {
    let name = agent
        .facts
        .label
        .as_deref()
        .or(agent.label.as_deref())
        .or(agent.facts.name.as_deref())
        .filter(|name| !name.is_empty());
    match name {
        Some(name) => format!("{} · {}", agent.location.pane_id, sanitize(name)),
        None => agent.location.pane_id.clone(),
    }
}

pub(super) fn unavailable() -> String {
    "unavailable".to_string()
}

/// The bus tasks attached to this agent row, and the note a disagreement with
/// the pane tokens earns.
///
/// The task rows, the badge and this summary all read the row's one projection,
/// so they cannot disagree with each other: the pane's own ids where no
/// publisher matches the row, and the publisher's complete list — an empty one
/// included — where one does. What the publisher sent is drawn as sent.
pub(super) fn task_lines(agent: &AgentRow, blocks: &Blocks<'_>, page: &mut PageLines) {
    let Some(TaskSource::Bus) = agent.tasks.source else {
        // The token fallback is the pane's own report: its ids and phases are
        // the rows beneath this one, and the `background` line above states
        // them as published. Nothing more is claimed about them.
        return;
    };

    if agent.tasks.tasks.is_empty() {
        // A published empty list and no publisher are different facts, and the
        // panel says which of the two it is.
        page.push(field("bus", "connected — no unresolved tasks".into()));
    } else {
        let now = now_unix_ms();
        for task in &agent.tasks.tasks {
            let Some(published) = task.published.as_ref() else {
                continue;
            };
            let key = Disclosure::Task(task.id.clone());
            let open = blocks.open(&key);
            let summary = blocks.summary(&key, open, "task", task_summary(published, now));
            match open {
                Some(_) => page.push_block(&key, summary),
                None => page.push(summary),
            }
            // The command and the directory are the lines that wrap over the
            // panel, so they are the ones the disclosure holds. The summary
            // above carries the identity, the state and the measures open or
            // closed, so nothing a reader compares across tasks moves.
            if open != Some(false) {
                if let Some(command) = published.command.as_deref() {
                    page.push(field("command", bound_text(command)));
                }
                if let Some(cwd) = published.cwd.as_deref() {
                    page.push(field("cwd", bound_text(cwd)));
                }
            }
        }
    }
    // The list outranks the count, so the count is never drawn in its place;
    // where the two disagree the panel states it instead of resolving it. The
    // token counts running tasks, so only running tasks are comparable with it.
    if let Some(reported) = agent.facts.background_running
        && reported as usize != agent.tasks.running()
    {
        page.push(field(
            "tokens",
            format!(
                "report {reported} running; the bus lists {}",
                agent.tasks.running()
            ),
        ));
    }
}

/// One task as the details draw it: its id, its state word as published, how
/// long it has run, how long ago it last produced output, how much it has
/// produced, and its exit code once there is one. A field the publisher did not
/// send draws nothing — absent is not a zero.
/// A published task's identity and the measures beside it, as one line's value:
/// the facts that never collapse.
pub(super) fn task_summary(task: &Task, now_unix_ms: u64) -> String {
    let mut parts = vec![sanitize(&task.id), sanitize(&task.state.to_string())];
    if let Some(started) = task.started_at {
        parts.push(duration(elapsed(started, now_unix_ms)));
    }
    if let Some(last_output) = task.last_output_at {
        parts.push(format!(
            "output {} ago",
            duration(elapsed(last_output, now_unix_ms))
        ));
    }
    if let Some(bytes) = task.output_bytes {
        parts.push(format!("{bytes} B"));
    }
    if let Some(code) = task.exit_code {
        parts.push(format!("exit {code}"));
    }
    parts.join(" · ")
}

/// How long ago a published Unix-millisecond timestamp was.
pub(super) fn elapsed(then_unix_ms: u64, now_unix_ms: u64) -> Duration {
    Duration::from_millis(now_unix_ms.saturating_sub(then_unix_ms))
}

/// Unix milliseconds now, for the age of a published timestamp. Read where the
/// panel is drawn: an age is true of the frame that shows it, as every other
/// fact on screen is.
pub(super) fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// Most of a published `command` or `cwd` the details draw. The contract bounds
/// both to 256 characters at the publisher; Radar bounds them again here, where
/// an arbitrary string reaches the screen.
pub(super) const BUS_TEXT_LIMIT: usize = 256;

/// External text as the details draw it: control characters stripped, and the
/// value bounded so that one published field cannot fill the panel.
pub(super) fn bound_text(text: &str) -> String {
    let cleaned = sanitize(text);
    if cleaned.chars().count() <= BUS_TEXT_LIMIT {
        return cleaned;
    }
    let kept: String = cleaned.chars().take(BUS_TEXT_LIMIT).collect();
    format!("{kept}…")
}

/// The bus's own diagnostic, when the listener is not running. The bus is a
/// subsystem beside the source rather than part of it, so its failure is
/// written on a line of its own.
pub(super) fn task_diagnostic(app: &App) -> Option<Line<'static>> {
    let diagnostic = app.task_diagnostic()?;
    Some(field(
        "bus",
        format!("unavailable — {}", sanitize(diagnostic)),
    ))
}

pub(super) fn location_text(label: Option<&str>, id: &str) -> String {
    match label {
        _ if id.is_empty() => unavailable(),
        Some(label) => format!("{} ({})", sanitize(label), sanitize(id)),
        None => sanitize(id),
    }
}

/// One line of runtime location: workspace, tab and pane, innermost last.
pub(super) fn location_line(
    workspace: (Option<&str>, &str),
    tab: (Option<&str>, &str),
    pane_id: &str,
) -> String {
    format!(
        "{} · {} · {}",
        location_text(workspace.0, workspace.1),
        location_text(tab.0, tab.1),
        location_text(None, pane_id)
    )
}

/// Reported identity text: an explicit UUID, or the source-qualified reported
/// reference, exactly as reported.
pub(super) fn session_text(session: Option<&SessionIdentity>) -> Option<String> {
    match session? {
        SessionIdentity::Uuid(uuid) => Some(format!("uuid {}", sanitize(uuid.as_str()))),
        SessionIdentity::Reported { source, value } => Some(match source {
            Some(source) => format!("{} {}", sanitize(source), sanitize(value)),
            None => sanitize(value),
        }),
    }
}

/// Strips terminal-control characters from runtime-provided text.
///
/// Whitespace-like controls become a space so words stay separated; every
/// other control character (notably ESC, so ANSI sequences are never emitted)
/// is removed.
pub(super) fn sanitize(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\t' | '\n' | '\r' => cleaned.push(' '),
            _ if ch.is_control() => {}
            _ => cleaned.push(ch),
        }
    }
    cleaned
}
