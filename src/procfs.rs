//! What this machine can say about a process that the runtime cannot.
//!
//! Herdr reports which command holds a pane's foreground. The kernel reports
//! four further facts Radar presents: how long that process has been alive,
//! whether it has taken the terminal over, whether it is running the program
//! that is installed now, and what it is using. None is in the Herdr API, so
//! all are read here and nowhere else — a process that is gone, or a platform
//! whose state cannot be read, yields unknown facts rather than a guess.
//!
//! Comparison is by package family for the Pi-Bolt variants and by file name for
//! everything else. A bolt process holds `<package>/lib/pi-bolt/pi` while `PATH`
//! resolves `<package>/bin/pi-bolt`, so the two files never share a name and a
//! comparison by name can decide nothing about them: those families are resolved
//! from the running executable's own store package and compared root by root,
//! and the name the runtime reports — `pi`, the name of the payload every
//! variant replaced its shell with — selects no program. A process's environment
//! is never read, so Radar's own `PATH` is the one that decides.
//!
//! Resources are the one fact with a memory. Interval CPU is the difference
//! between two readings of the same process incarnation, so [`Sampler`] keeps
//! the previous reading of each identity across refreshes and is handed to the
//! next one with them, pruned so the history is bounded by the processes one
//! refresh saw rather than by every process that has ever run. Every read a
//! sample is made of goes through [`Procs`], so the arithmetic is exercised
//! against a scripted process table instead of this machine's.
//!
//! A refresh also takes one bounded snapshot of the whole process table
//! ([`Sampler::scan`]), and every root sampled in that refresh sums the
//! processes beneath it from that one snapshot: the kernel's ancestry, read
//! once, with links that cannot be real rejected and a snapshot that could not
//! be read in full reported as such rather than as an empty tree. The scan then
//! reads every process it took a second time, once for the whole refresh, and a
//! reading only counts if the same incarnation is still beneath the same parent:
//! a member that exited, whose pid was handed on or that was reparented is left
//! out with its unverifiable subtree, and the totals that would have carried it
//! say so. Those members are also what a root publishes one by one, so a reader
//! can see which child spent the CPU the total reports: the rows are the members
//! the totals are made of, never a second walk of the table.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::model::{
    BinaryFreshness, BinaryIdentity, BinaryUnknown, CpuPercent, DescendantResources,
    DescendantSample, LocalFacts, ProcessIdentity, ProcessResources, ProcessState, TerminalMode,
    Total,
};

/// The kernel's marker on an executable link whose file has been unlinked or
/// replaced.
const DELETED: &str = " (deleted)";

/// One refresh's installed-program lookups.
///
/// The search directories come from Radar's own `PATH`, read once, and each
/// distinct program name is resolved through them once, so a refresh pays one
/// lookup per program rather than one per pane. Injecting the directories lets
/// tests exercise the comparison without this machine's store.
pub struct BinaryIndex {
    directories: Vec<PathBuf>,
    resolved: HashMap<String, Option<PathBuf>>,
}

impl Default for BinaryIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl BinaryIndex {
    /// A resolver over Radar's own `PATH`.
    pub fn new() -> Self {
        let directories = std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect())
            .unwrap_or_default();
        Self::searching(directories)
    }

    /// A resolver over explicit search directories, as tests and callers that
    /// must not read the environment supply.
    pub fn searching(directories: Vec<PathBuf>) -> Self {
        Self {
            directories,
            resolved: HashMap::new(),
        }
    }

    /// The installed executable for `program`, symlinks followed, or `None`
    /// when no search directory has an executable one. Each name is looked up
    /// once.
    ///
    /// The search follows `PATH` command resolution: only an executable regular
    /// file qualifies, so a directory or a same-named non-executable file earlier
    /// on the search path cannot mask the program. A name containing a path
    /// separator is a path, not a command name, and is not searched for.
    fn installed(&mut self, program: &str) -> Option<PathBuf> {
        if let Some(cached) = self.resolved.get(program) {
            return cached.clone();
        }
        let found = if program.contains(std::path::MAIN_SEPARATOR) {
            // A path, not a command name: nothing to search for on PATH.
            None
        } else {
            self.directories
                .iter()
                .find_map(|directory| executable_target(&directory.join(program)))
        };
        self.resolved.insert(program.to_string(), found.clone());
        found
    }
}

/// The resolved target of a search-path candidate, when it is a command.
///
/// `metadata` follows symlinks, so a launcher that points at the program is
/// judged by the file it resolves to. A directory, or a file with no execute
/// bit, is not a command and is passed over.
fn executable_target(candidate: &Path) -> Option<PathBuf> {
    let metadata = std::fs::metadata(candidate).ok()?;
    if !metadata.is_file() || !is_executable(&metadata) {
        return None;
    }
    std::fs::canonicalize(candidate).ok()
}

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &std::fs::Metadata) -> bool {
    false
}

/// The package families whose running payload is resolved by package rather
/// than by file name.
///
/// Pi-Bolt's variants are one product built as several packages: a pane's
/// process holds `<package>/lib/pi-bolt/pi` while the command `PATH` resolves is
/// `<package>/bin/pi-bolt`, so the two files never share a name. These are the
/// exact package names resolved this way, and a name that merely starts with one
/// of them is not one of them: `pi-bolt` never stands for `pi-bolt-child`.
const BOLT_FAMILIES: [&str; 2] = ["pi-bolt", "pi-bolt-child"];

/// The largest launcher script this resolver reads.
///
/// A launcher that leads a process is a short setup script. A file larger than
/// this is not the shape being looked for, and is not read to find out.
const MAX_LAUNCHER_BYTES: u64 = 64 * 1024;

/// The package name and version of a Nix store derivation root name.
///
/// A store path is `<store>/<hash>-<name>-<version>`, so the first `-` ends the
/// hash and the last one ends the package name: `hxdd…-pi-bolt-0.7.1` is
/// `pi-bolt` at `0.7.1`. A root with no version — an unversioned launcher
/// package, or any other name — is not a versioned package, and a version that
/// does not start with a digit is not read as one. Nothing is inferred from a
/// name that does not have the shape.
fn store_package(root_name: &str) -> Option<(&str, &str)> {
    let (_, package) = root_name.split_once('-')?;
    let (name, version) = package.rsplit_once('-')?;
    version
        .starts_with(|c: char| c.is_ascii_digit())
        .then_some((name, version))
}

/// The bolt family the running executable belongs to, when it is one.
///
/// Read from the running file's own store package and nothing else, so the same
/// payload is the same family however the runtime names the process.
fn bolt_family(running: &Path) -> Option<&'static str> {
    let root = installation(running);
    let (name, _version) = store_package(file_name(&root)?)?;
    BOLT_FAMILIES.into_iter().find(|family| *family == name)
}

/// The package root whose payload an installed entrypoint runs, or why it could
/// not be identified.
///
/// Two shapes are recognized and nothing else:
///
/// * An entrypoint inside a versioned package of the same family is that
///   package's own launcher — the shape a Nix package has, where `bin/pi-bolt`
///   and `lib/pi-bolt/pi` share one store root — so the package root is the
///   payload root and nothing is read.
/// * Otherwise the entrypoint must be a launcher script whose shape is
///   unambiguous, and the package root of its one target is the payload root
///   (see [`script_target`]).
///
/// A root that is not a versioned package of the family is not a payload root
/// merely because the entrypoint sits inside it: an unversioned launcher package
/// has a root of its own that holds no payload.
pub fn counterpart_root(installed: &Path, family: &str) -> Result<PathBuf, BinaryUnknown> {
    let root = installation(installed);
    if let Some((name, _version)) = file_name(&root).and_then(store_package)
        && name == family
    {
        return Ok(root);
    }
    // Only a launcher script is read, and only its one declared entrypoint is
    // followed — and that entrypoint must itself be a versioned package of
    // this exact family. A launcher naming another family's package, an
    // unrelated program, an unversioned root or a file inside a package says
    // nothing about which payload this family runs, so it is refused rather
    // than taken as an answer.
    let payload = installation(&script_target(installed)?);
    let Some((name, _version)) = file_name(&payload).and_then(store_package) else {
        return Err(BinaryUnknown::UnsupportedLauncher);
    };
    (name == family)
        .then_some(payload)
        .ok_or(BinaryUnknown::UnsupportedLauncher)
}

/// The store path one launcher script runs, when its shape says so.
///
/// The recognized shape is a short script whose only mention of `exec` is one
/// line at the start of a line naming one absolute store path literally. Setup
/// may precede it — flags, an exported variable, a test of a file — because that
/// is what the launchers in use contain, and
///
/// * the `exec` must be the last thing the script does: an `exec` inside a
///   branch is followed by that branch's `fi`, `done` or `else`, and so is any
///   line after it,
/// * the script must mention `exec` exactly once, so several possible targets,
///   an indented or compound `exec` are refused rather than chosen between,
/// * the target must be one literal store entrypoint, `<root>/bin/<program>`:
///   a quoted, computed or variable target does not say where the process
///   goes, and neither does a bare store file or a deeper path, and
/// * the file must be a script at all — a readable `#!` file no larger than
///   [`MAX_LAUNCHER_BYTES`].
///
/// This reads a launcher's declared target. It is not a shell interpreter, it
/// executes nothing, and it reads no compiled content: every other shape is
/// unknown rather than guessed at.
fn script_target(script: &Path) -> Result<PathBuf, BinaryUnknown> {
    let unsupported = || BinaryUnknown::UnsupportedLauncher;
    let metadata = std::fs::metadata(script).map_err(|_| unsupported())?;
    if metadata.len() > MAX_LAUNCHER_BYTES {
        return Err(unsupported());
    }
    let text = std::fs::read_to_string(script).map_err(|_| unsupported())?;
    let mut lines = text.lines();
    if !lines.next().is_some_and(|first| first.starts_with("#!")) {
        return Err(unsupported());
    }
    let mut target: Option<PathBuf> = None;
    for line in lines {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if !line.split_whitespace().any(|word| word == "exec") {
            // Setup, which may precede the one target but not follow it.
            if target.is_some() {
                return Err(unsupported());
            }
            continue;
        }
        let Some(rest) = line.strip_prefix("exec ") else {
            return Err(unsupported());
        };
        let Some(word) = rest.split_whitespace().next() else {
            return Err(unsupported());
        };
        if target.is_some() || !literal_store_entrypoint(word) {
            return Err(unsupported());
        }
        target = Some(PathBuf::from(word));
    }
    target.ok_or_else(unsupported)
}

/// Whether `word` is one literal store entrypoint and nothing else.
///
/// The token must be the whole target: no quoting, no substitution, no trailing
/// punctuation. The recognized shape is `<root>/bin/<program>` — a package
/// root, its `bin` directory and one program file — because that is what a
/// launcher names. A bare store file, a deeper path inside a package and any
/// `.` or `..` component are not that shape. The root name is not judged here:
/// whether it is this family's versioned package is [`counterpart_root`]'s
/// question.
fn literal_store_entrypoint(word: &str) -> bool {
    let Some(rest) = word.strip_prefix("/nix/store/") else {
        return false;
    };
    let mut components = rest.split('/');
    let (Some(root), Some("bin"), Some(program), None) = (
        components.next(),
        components.next(),
        components.next(),
        components.next(),
    ) else {
        return false;
    };
    [root, program].into_iter().all(|part| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
    })
}

/// How the running executable compares with the program installed for it,
/// through `binaries`.
///
/// `running` is the executable's link target and `deleted` whether the kernel
/// marked it so. `program` is the name the runtime reports, which is used only
/// where a payload is resolved by file name: a bolt variant is resolved from the
/// running file's own package family, so one the runtime names `pi` — or does
/// not name at all — is still compared. This is the comparison behind [`facts`],
/// exposed so it can be driven with injected paths.
pub fn identity(
    running: &Path,
    deleted: bool,
    program: Option<&str>,
    binaries: &mut BinaryIndex,
) -> BinaryIdentity {
    match bolt_family(running) {
        Some(family) => bolt_identity(running, deleted, family, binaries),
        None => named_identity(running, deleted, program, binaries),
    }
}

/// How a bolt variant's running payload compares with the payload its installed
/// counterpart runs.
///
/// The counterpart is the exact family name the running package names, so a
/// child is never compared with the lead's launcher however the runtime names
/// it. Roots that differ are stale whatever version both packages carry: the
/// question is whether this process runs what `PATH` points at now, and a
/// rebuild of the same version is a different build.
fn bolt_identity(
    running: &Path,
    deleted: bool,
    family: &str,
    binaries: &mut BinaryIndex,
) -> BinaryIdentity {
    let running_installation = installation(running);
    let mut identity = BinaryIdentity {
        running: Some(running_installation.display().to_string()),
        executable: Some(running.display().to_string()),
        unknown: Some(BinaryUnknown::NoCounterpart),
        ..BinaryIdentity::default()
    };
    let payload = match binaries.installed(family) {
        Some(installed) => counterpart_root(&installed, family),
        None => Err(BinaryUnknown::NoCounterpart),
    };
    match payload {
        Ok(payload) => {
            identity.installed = Some(payload.display().to_string());
            identity.unknown = None;
            identity.freshness = if deleted
                || (in_store(&running_installation)
                    && in_store(&payload)
                    && running_installation != payload)
            {
                BinaryFreshness::Stale
            } else {
                BinaryFreshness::Current
            };
            identity
        }
        // The kernel's mark is a fact about the running file: the process lost
        // its binary, and is stale whether or not a counterpart was read.
        Err(_) if deleted => {
            identity.freshness = BinaryFreshness::Stale;
            identity.unknown = None;
            identity
        }
        Err(reason) => {
            identity.unknown = Some(reason);
            identity
        }
    }
}

/// How the running executable compares with the program `PATH` resolves for the
/// name the runtime reports.
///
/// This is the comparison for every program whose payload is not resolved by
/// package: the two files must share a name, and their installations are
/// compared. A file with another name — an interpreter, a helper, a build the
/// runtime misnames — is not attributed to a program at all.
fn named_identity(
    running: &Path,
    deleted: bool,
    program: Option<&str>,
    binaries: &mut BinaryIndex,
) -> BinaryIdentity {
    let executable = Some(running.display().to_string());
    let Some(program) = program else {
        return BinaryIdentity {
            executable,
            unknown: Some(BinaryUnknown::NotCompared),
            ..BinaryIdentity::default()
        };
    };
    match binaries.installed(program) {
        Some(installed) => compare_named(running, deleted, &installed),
        None => BinaryIdentity {
            executable,
            unknown: Some(BinaryUnknown::NoCounterpart),
            ..BinaryIdentity::default()
        },
    }
}

/// Reads what the operating system knows about `pid`, the process the runtime
/// reports as `program`.
///
/// `now` is the instant this refresh measures against — one for the whole
/// refresh, so every interval in one frame is measured over the same clock.
/// `sampler` turns it into this process's resources, or leaves them unknown
/// when it cannot say.
pub fn facts(
    pid: i32,
    program: Option<&str>,
    binaries: &mut BinaryIndex,
    sampler: &mut Sampler,
    now: Instant,
) -> LocalFacts {
    LocalFacts {
        resources: sampler.sample(pid, now),
        ..platform::facts(pid, program, binaries)
    }
}

/// The kernel facts a sample is read from.
///
/// Sampling goes through this seam so its arithmetic is tested against a
/// scripted process table rather than this machine's, and so a platform without
/// these files reports unknown resources instead of guessing.
pub trait Procs: Send {
    /// The boot identity this machine is running under, or `None` when the
    /// kernel does not report one.
    fn boot_id(&mut self) -> Option<String>;
    /// At most `limit` of the pids this machine currently reports, or `None`
    /// when the process table cannot be enumerated at all. Names that are not
    /// pids are not processes and are not listed. A table holding more than
    /// `limit` is cut short rather than enumerated in full — the caller asks for
    /// one more than it will read so it can tell the two apart — and enumeration
    /// stops when `cancel` is set.
    fn pids(&mut self, limit: usize, cancel: &AtomicBool) -> Option<Vec<i32>>;
    /// One process's `/proc/<pid>/stat` line, or `None` when it cannot be read.
    fn stat(&mut self, pid: i32) -> Option<String>;
    /// This kernel's page size in bytes, or `None` when it cannot be read.
    fn page_size(&mut self) -> Option<u64>;
}

/// The previous reading of one process incarnation.
struct Baseline {
    /// User plus system CPU time, in clock ticks. Both, and never their reaped
    /// children's: `cutime`/`cstime` would count a child's CPU here and again
    /// in the child's own sample.
    cpu_ticks: u64,
    /// When it was read, on the monotonic clock.
    at: Instant,
}

/// Processes one refresh reads at most, in its scan and in the pass that
/// confirms what the scan read.
///
/// A larger process table is summarised as partial rather than silently cut to
/// fit: the budget is what keeps a pathological table from making a refresh
/// unbounded work, and the coverage is what says the totals are lower bounds.
const SCAN_BUDGET: usize = 4096;

/// Why a scan could not describe the process table at all.
const NO_TABLE: &str = "the process table could not be read";

/// Why what a scan read was not counted: the refresh was cancelled before the
/// process could be confirmed, so nothing says it is still that process.
const CANCELLED: &str = "the process scan was cancelled";

/// One refresh's reading of the whole process table.
struct Snapshot {
    /// The processes it read, keyed by pid.
    entries: HashMap<i32, Entry>,
    /// Child pids by parent pid, from the same reads.
    children: HashMap<i32, Vec<i32>>,
    /// The kernel's page size, read once for the whole scan.
    page_size: Option<u64>,
    /// How much of the table the scan read.
    scan: Scan,
}

/// How much of the process table one scan read.
enum Scan {
    /// Every process the kernel reported was read.
    Complete,
    /// Something was not: the scan was cancelled, the table was larger than the
    /// budget, or a process could not be read.
    Partial(String),
    /// Nothing could be enumerated at all, with the reason.
    Unavailable(String),
}

/// One process, as the scan read it.
struct Entry {
    /// The identity that read carried.
    identity: ProcessIdentity,
    /// The name that read carried: the kernel's `comm`, which is not argv.
    name: String,
    /// The scheduler state that read carried.
    state: ProcessState,
    /// The parent it was read under, kept so the link can be confirmed later.
    parent: i32,
    /// Its own interval CPU, against the baseline of the same incarnation.
    /// `None` when this refresh is the first to read it, or when no interval can
    /// be measured.
    cpu: Option<CpuPercent>,
    /// Its resident set, `None` when its page count is not a size.
    rss_bytes: Option<u64>,
    /// `None` while a later read confirmed this one; `Some(reason)` once no
    /// later reading did, which is why the process is not counted and neither is
    /// anything reached through it.
    unconfirmed: Option<String>,
}

/// One process an ancestry walk reached: the entry the scan read, and the
/// identity of the process it was read beneath.
///
/// The parent is the entry the walk was standing on when it followed the link,
/// which is what makes the link a fact rather than a pid: a pid names a
/// different process once the kernel reuses it.
struct Member<'a> {
    parent: &'a ProcessIdentity,
    entry: &'a Entry,
}

impl Snapshot {
    /// A snapshot that could not be taken, for `reason`.
    fn unavailable(reason: &str) -> Self {
        Self {
            entries: HashMap::new(),
            children: HashMap::new(),
            page_size: None,
            scan: Scan::Unavailable(reason.to_string()),
        }
    }

    /// The processes beneath `root` in this snapshot, each with the process it
    /// was read beneath, and why any were left out.
    ///
    /// The walk follows the parent links the same reads reported, and refuses a
    /// link that cannot be real: a process that started before the one the link
    /// names as its parent was pointed at by a pid that had been reused, not by
    /// the process that started it. Each process is counted at most once — the
    /// kernel gives a process one parent — so a table read in pieces cannot
    /// double-count, and reaching a process twice is itself the sign of a cycle
    /// in those links.
    fn beneath(&self, root: i32) -> (Vec<Member<'_>>, Vec<String>) {
        let mut members = Vec::new();
        let mut why = Vec::new();
        let mut seen = HashSet::from([root]);
        let mut reached = vec![root];
        while let Some(pid) = reached.pop() {
            let Some(parent) = self.entries.get(&pid) else {
                continue;
            };
            for child in self.children.get(&pid).into_iter().flatten() {
                if !seen.insert(*child) {
                    push_once(&mut why, "the process table read contained a cycle");
                    continue;
                }
                let Some(entry) = self.entries.get(child) else {
                    continue;
                };
                if let Some(reason) = &entry.unconfirmed {
                    // Nothing reached through a process that is no longer that
                    // process can be counted either, so the subtree is refused
                    // with it.
                    push_once(&mut why, reason);
                    continue;
                }
                if entry.identity.start_ticks < parent.identity.start_ticks {
                    push_once(&mut why, "an ancestry link pointed at a reused pid");
                    continue;
                }
                members.push(Member {
                    parent: &parent.identity,
                    entry,
                });
                reached.push(*child);
            }
        }
        (members, why)
    }
}

/// Adds `reason` to `reasons` if it is not already there.
///
/// A walk reports a broken link once per root rather than once per link, so a
/// root's coverage reads as a sentence about that root.
fn push_once(reasons: &mut Vec<String>, reason: &str) {
    if !reasons.iter().any(|known| known == reason) {
        reasons.push(reason.to_string());
    }
}

/// `total` when every member contributed, otherwise a lower bound with why.
fn summed<T>(total: T, missing: u32, reasons: &[String], missing_reason: &str) -> Total<T> {
    let mut reasons = reasons.to_vec();
    if missing > 0 {
        reasons.push(format!("{missing_reason} for {missing} of them"));
    }
    if reasons.is_empty() {
        Total::Complete(total)
    } else {
        Total::Partial(total, reasons.join("; "))
    }
}

/// A root's descendants when the snapshot cannot describe them, for `reason`.
fn unavailable(reason: &str) -> DescendantResources {
    DescendantResources {
        observed: None,
        members: Vec::new(),
        rss_bytes: Total::Unknown(reason.to_string()),
        cpu: Total::Unknown(reason.to_string()),
    }
}

/// The fields of a `/proc/<pid>/stat` line a sample is built from.
///
/// Identity and counters are taken from one read of one line, so a sample
/// cannot mix two incarnations. The command name sits in parentheses and may
/// itself contain spaces and parentheses, which is why the fields are counted
/// from the *last* `)` rather than by splitting the whole line.
pub(crate) struct Stat {
    /// The kernel's name for the process (field 2), between the parentheses it
    /// writes it in: it may hold spaces and parentheses of its own, so it runs
    /// from the first `(` to the last `)`.
    name: String,
    /// Scheduler state (field 3).
    state: ProcessState,
    /// The process that started it (field 4).
    parent: i32,
    /// User plus system CPU time, in clock ticks (fields 14 and 15).
    cpu_ticks: u64,
    /// Start time since boot, in clock ticks (field 22).
    start_ticks: u64,
    /// Resident set size, in pages (field 24). Signed in the kernel's format,
    /// and a negative count is not a size.
    rss_pages: i64,
}

impl Stat {
    /// Reads those fields, or `None` when this is not a stat line.
    fn parse(text: &str) -> Option<Self> {
        // The name comes first and is parenthesised, so the fields below are
        // counted from what follows the `)` that closes it.
        let name = text.find('(')? + 1;
        let named_to = text.rfind(')').filter(|end| *end >= name)?;
        let fields: Vec<&str> = text[named_to + 1..].split_whitespace().collect();
        // The remainder begins at `state`, so a field is where proc(5) puts it,
        // counted from three less.
        let field = |number: usize| fields.get(number - 3).copied();
        Some(Self {
            name: text[name..named_to].to_string(),
            state: ProcessState::from_letter(field(3)?.chars().next()?),
            parent: field(4)?.parse().ok()?,
            cpu_ticks: field(14)?
                .parse::<u64>()
                .ok()?
                .checked_add(field(15)?.parse().ok()?)?,
            start_ticks: field(22)?.parse().ok()?,
            rss_pages: field(24)?.parse().ok()?,
        })
    }

    /// Extracts the process birth identity from a stat line. This shares the
    /// sampler's proc(5) parser so verification and sampling interpret PID reuse
    /// identically.
    pub(crate) fn birth_identity(text: &str) -> Option<(i32, u64)> {
        let pid = text.split_once(' ')?.0.parse().ok()?;
        let start_ticks = Self::parse(text)?.start_ticks;
        Some((pid, start_ticks))
    }

    /// Resident set size in bytes, or `None` when the kernel's page count is
    /// not a size: negative, or past what bytes can count.
    fn rss_bytes(&self, page_size: u64) -> Option<u64> {
        u64::try_from(self.rss_pages).ok()?.checked_mul(page_size)
    }
}

/// Samples process resources across refreshes.
///
/// One sampler lives for the run: interval CPU needs the previous reading of
/// the same incarnation, so the history — and nothing else — is what a refresh
/// inherits from the one before it. Baselines are keyed by [`ProcessIdentity`]
/// and never by pid: a reused pid names a different process whose counters
/// start again, and a delta across the two would measure neither.
pub struct Sampler {
    procs: Box<dyn Procs>,
    /// The boot identity, read at most once per refresh: it is one fact about
    /// the machine, and the outermost `None` means it has not been read yet.
    boot: Option<Option<String>>,
    baselines: HashMap<ProcessIdentity, Baseline>,
    /// Identities sampled since the last [`Self::finish`], which are the ones
    /// that survive pruning.
    observed: HashSet<ProcessIdentity>,
    /// This refresh's process table, once [`Self::scan`] has read it.
    snapshot: Option<Snapshot>,
    /// Processes one scan reads at most.
    budget: usize,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    /// A sampler reading this machine's process table.
    pub fn new() -> Self {
        Self::reading(platform::SystemProcs)
    }

    /// A sampler reading `procs`, as tests and callers that must not read this
    /// machine's process table supply.
    pub fn reading(procs: impl Procs + 'static) -> Self {
        Self {
            procs: Box::new(procs),
            boot: None,
            baselines: HashMap::new(),
            observed: HashSet::new(),
            snapshot: None,
            budget: SCAN_BUDGET,
        }
    }

    /// Scans the process table once for this refresh, then confirms what it
    /// read.
    ///
    /// Every root sampled during this refresh sums the processes beneath it from
    /// this one snapshot, so a refresh reads the table once rather than walking
    /// it per pane. `now` is that refresh's instant, so descendant intervals and
    /// root intervals are measured over the same clock. A scan that is
    /// cancelled, that finds a table larger than the budget, or that cannot read
    /// part of it still records what it saw and says what it could not cover.
    ///
    /// A table read is not an instant, so each process the scan read is read
    /// again before any root is sampled. The second pass is shared by every root
    /// of the refresh — one read per process, not one per root — and a reading
    /// only counts when the same incarnation is still beneath the same parent.
    /// Anything else, and anything reached through it, is left out of the totals
    /// and leaves no baseline behind.
    pub fn scan(&mut self, now: Instant, cancel: &AtomicBool) {
        let Some(boot_id) = self.boot_identity() else {
            self.snapshot = Some(Snapshot::unavailable("the boot identity could not be read"));
            return;
        };
        // One more pid than the scan will read, so a table larger than the budget
        // is recognisable as one rather than looking exactly full.
        let Some(pids) = self.procs.pids(self.budget.saturating_add(1), cancel) else {
            self.snapshot = Some(Snapshot::unavailable(NO_TABLE));
            return;
        };
        let page_size = self.procs.page_size();
        let mut entries = HashMap::new();
        let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
        // The pids the scan read, in the order it read them: the second pass
        // walks exactly these, and nothing else is claimed to be confirmed.
        let mut read = Vec::new();
        let mut unreadable = 0u32;
        let mut cancelled = false;
        for pid in pids.iter().copied().take(self.budget) {
            if cancel.load(Ordering::SeqCst) {
                cancelled = true;
                break;
            }
            let Some(stat) = self.procs.stat(pid).and_then(|line| Stat::parse(&line)) else {
                unreadable += 1;
                continue;
            };
            let identity = ProcessIdentity {
                boot_id: boot_id.clone(),
                pid,
                start_ticks: stat.start_ticks,
            };
            let cpu = self.cpu_percent(&identity, &stat, now);
            let rss_bytes = page_size.and_then(|page| stat.rss_bytes(page));
            // Every reading is kept as the baseline the next refresh measures
            // from: a process that is nobody's descendant today can be one
            // tomorrow, and it cannot be given a history retroactively. One that
            // is not confirmed below gives its baseline back.
            self.baselines.insert(
                identity.clone(),
                Baseline {
                    cpu_ticks: stat.cpu_ticks,
                    at: now,
                },
            );
            self.observed.insert(identity.clone());
            children.entry(stat.parent).or_default().push(pid);
            entries.insert(
                pid,
                Entry {
                    identity,
                    name: stat.name,
                    state: stat.state,
                    parent: stat.parent,
                    cpu,
                    rss_bytes,
                    unconfirmed: None,
                },
            );
            read.push(pid);
        }
        // A scan cancelled before its end confirms nothing, and a scan that ran
        // to the end confirms what the second pass reached.
        let mut unconfirmed_from = if cancelled { 0 } else { read.len() };
        if !cancelled {
            for (index, pid) in read.iter().enumerate() {
                if cancel.load(Ordering::SeqCst) {
                    unconfirmed_from = index;
                    break;
                }
                let entry = entries.get_mut(pid).expect("a read pid is an entry");
                let reason = match self.procs.stat(*pid).and_then(|line| Stat::parse(&line)) {
                    None => Some("a process beneath this one could not be read again"),
                    Some(stat) if stat.start_ticks != entry.identity.start_ticks => {
                        Some("a process beneath this one was replaced while the table was read")
                    }
                    Some(stat) if stat.parent != entry.parent => {
                        Some("a process beneath this one was reparented while the table was read")
                    }
                    Some(_) => None,
                };
                if let Some(reason) = reason {
                    self.unconfirm(entry, reason);
                }
            }
        }
        // Whatever the second pass did not reach is unaccounted for rather than
        // trusted, and says so wherever it would have been counted.
        for pid in &read[unconfirmed_from..] {
            let entry = entries.get_mut(pid).expect("a read pid is an entry");
            self.unconfirm(entry, CANCELLED);
        }
        self.snapshot = Some(Snapshot {
            entries,
            children,
            page_size,
            scan: if cancelled {
                Scan::Partial(CANCELLED.to_string())
            } else if pids.len() > self.budget {
                Scan::Partial(format!(
                    "the process table is larger than the {}-process scan budget",
                    self.budget
                ))
            } else if unreadable > 0 {
                Scan::Partial(format!("{unreadable} of the processes could not be read"))
            } else {
                Scan::Complete
            },
        });
    }

    /// Samples one process and remembers the reading for the next sample.
    ///
    /// `None` when this machine cannot say what the process is using: its line
    /// or the boot identity could not be read, the line is not one this kernel
    /// wrote, or the process did not survive its own sample. A process whose
    /// line was read always yields state and its identity, even when RSS cannot
    /// be converted or no interval is measurable yet.
    pub fn sample(&mut self, pid: i32, now: Instant) -> Option<ProcessResources> {
        let stat = Stat::parse(&self.procs.stat(pid)?)?;
        let identity = ProcessIdentity {
            boot_id: self.boot_identity()?,
            pid,
            start_ticks: stat.start_ticks,
        };
        let page_size = self.procs.page_size();
        // The fields above were read before the page size, and the incarnation
        // is confirmed after it: a pid reused in between, or a process that
        // exited, would otherwise publish a reading under an identity that was
        // no longer the process's. Nothing is committed for a process that did
        // not survive its own sample, so its successor starts without a
        // baseline and no delta crosses the two.
        let confirming = Stat::parse(&self.procs.stat(pid)?)?;
        if confirming.start_ticks != stat.start_ticks {
            return None;
        }
        // The interval comes from this refresh's scan when the scan read the same
        // incarnation: the scan has already recorded that reading as the new
        // baseline, so re-measuring here would compare the reading with itself.
        // Without a scan — a sample taken on its own — it is measured directly.
        let cpu = match self.scanned(&identity) {
            Some(entry) => entry.cpu,
            None => self.cpu_percent(&identity, &stat, now),
        };
        self.observed.insert(identity.clone());
        self.baselines.insert(
            identity.clone(),
            Baseline {
                cpu_ticks: stat.cpu_ticks,
                at: now,
            },
        );
        // The summary is computed against the identity the confirming read just
        // validated, so a reused pid cannot be handed its predecessor's
        // children.
        let descendants = self.descendants(&identity);
        Some(ProcessResources {
            identity,
            state: stat.state,
            rss_bytes: page_size.and_then(|page_size| stat.rss_bytes(page_size)),
            cpu,
            descendants,
        })
    }

    /// This refresh's scan entry for `identity`, when the scan read the same
    /// incarnation of that pid.
    fn scanned(&self, identity: &ProcessIdentity) -> Option<&Entry> {
        let entry = self.snapshot.as_ref()?.entries.get(&identity.pid)?;
        (entry.identity == *identity).then_some(entry)
    }

    /// Records that a reading is not counted, and gives its baseline back: an
    /// unconfirmed reading is no more a baseline for the next refresh than it is
    /// a member of a total.
    fn unconfirm(&mut self, entry: &mut Entry, reason: &str) {
        entry.unconfirmed = Some(reason.to_string());
        self.baselines.remove(&entry.identity);
        self.observed.remove(&entry.identity);
    }

    /// What this refresh's snapshot observed beneath `root`: the members
    /// themselves, and what they add up to.
    ///
    /// The root is only summarised against a snapshot that read the same
    /// incarnation: a scan taken before a reused pid would otherwise hand the new
    /// process the other one's children. The rows are those same members, so a
    /// reader can see which one spent what the total reports without a second
    /// reading of the table.
    fn descendants(&self, root: &ProcessIdentity) -> DescendantResources {
        let Some(snapshot) = &self.snapshot else {
            return unavailable("the process table was not scanned this refresh");
        };
        if let Scan::Unavailable(reason) = &snapshot.scan {
            return unavailable(reason);
        }
        let Some(entry) = snapshot.entries.get(&root.pid) else {
            return unavailable("this process was not in the process scan");
        };
        if entry.identity != *root {
            return unavailable("the process changed between the scan and its sample");
        }
        let (members, mut why) = snapshot.beneath(root.pid);
        if let Scan::Partial(reason) = &snapshot.scan {
            why.push(reason.clone());
        }
        let mut rss_total = 0u64;
        let mut rss_missing = 0u32;
        let mut cpu_total = 0u32;
        let mut cpu_missing = 0u32;
        for member in &members {
            match member.entry.rss_bytes {
                Some(bytes) => rss_total = rss_total.saturating_add(bytes),
                None => rss_missing += 1,
            }
            match member.entry.cpu {
                Some(percent) => cpu_total = cpu_total.saturating_add(percent.hundredths()),
                None => cpu_missing += 1,
            }
        }
        let rows = members
            .iter()
            .map(|member| DescendantSample {
                identity: member.entry.identity.clone(),
                parent: member.parent.clone(),
                name: member.entry.name.clone(),
                state: member.entry.state,
                cpu: member.entry.cpu,
                rss_bytes: member.entry.rss_bytes,
            })
            .collect();
        DescendantResources {
            observed: Some(members.len() as u32),
            members: rows,
            // A machine whose page size cannot be read converts no resident set
            // at all, which is not a total of zero.
            rss_bytes: match snapshot.page_size {
                None => Total::Unknown("this machine's page size could not be read".to_string()),
                Some(_) => summed(
                    rss_total,
                    rss_missing,
                    &why,
                    "the resident size could not be converted",
                ),
            },
            cpu: summed(
                CpuPercent::from_hundredths(cpu_total),
                cpu_missing,
                &why,
                "no CPU sample could be compared with",
            ),
        }
    }

    /// Ends a refresh: the baselines of processes it did not read are dropped,
    /// so the history is bounded by one refresh's processes instead of growing
    /// with every process that has ever exited. Pruning happens after the whole
    /// refresh, once every root has summed its descendants from the snapshot.
    pub fn finish(&mut self) {
        self.baselines
            .retain(|identity, _| self.observed.contains(identity));
        self.observed.clear();
        self.boot = None;
        self.snapshot = None;
    }

    /// The boot identity, read at most once per refresh: a refresh either has it
    /// for every process or samples none of them, and a kernel that will not say
    /// is not asked once per pane per second.
    fn boot_identity(&mut self) -> Option<String> {
        if self.boot.is_none() {
            self.boot = Some(self.procs.boot_id());
        }
        self.boot.clone().flatten()
    }

    /// Interval CPU against the previous reading of the same incarnation.
    ///
    /// Unknown rather than zero when there is no interval to measure: no earlier
    /// reading of this identity, no elapsed time, a clock that went backwards, a
    /// counter that regressed — the identity matched but the counters did not
    /// belong to one process — or a kernel that will not say how many clock
    /// ticks a second is.
    fn cpu_percent(
        &self,
        identity: &ProcessIdentity,
        stat: &Stat,
        now: Instant,
    ) -> Option<CpuPercent> {
        let before = self.baselines.get(identity)?;
        let elapsed = now
            .checked_duration_since(before.at)
            .filter(|elapsed| !elapsed.is_zero())?;
        let ticks = stat.cpu_ticks.checked_sub(before.cpu_ticks)?;
        let hertz = clock_ticks()?;
        let percent = ticks as f64 / hertz as f64 / elapsed.as_secs_f64() * 100.0;
        Some(CpuPercent::from_hundredths((percent * 100.0).round() as u32))
    }
}

/// Kernel clock ticks per second (`_SC_CLK_TCK`), or `None` when the kernel
/// cannot say — a machine whose CPU time Radar cannot convert.
#[cfg(target_os = "linux")]
fn clock_ticks() -> Option<i64> {
    let hertz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    (hertz > 0).then_some(hertz)
}

/// No kernel counter is read on a platform Radar has no reader for.
#[cfg(not(target_os = "linux"))]
fn clock_ticks() -> Option<i64> {
    None
}

/// Compares a running executable's link target with the file `PATH` resolved
/// for the program the runtime named.
///
/// The comparison is between installations, not files: inside the Nix store a
/// wrapper and the binary it launches share a store root, so they agree. A
/// running file whose name is not that match's is not compared at all — an
/// interpreter or helper the runtime reports is never attributed to the agent.
fn compare_named(running: &Path, deleted: bool, installed: &Path) -> BinaryIdentity {
    let executable = Some(running.display().to_string());
    let same_name = file_name(running).is_some_and(|name| Some(name) == file_name(installed));
    if !same_name {
        return BinaryIdentity {
            executable,
            unknown: Some(BinaryUnknown::NotCompared),
            ..BinaryIdentity::default()
        };
    }
    let running_installation = installation(running);
    let installed_installation = installation(installed);
    let stale = deleted
        || (in_store(&running_installation)
            && in_store(&installed_installation)
            && running_installation != installed_installation);
    BinaryIdentity {
        freshness: if stale {
            BinaryFreshness::Stale
        } else {
            BinaryFreshness::Current
        },
        // The installations, not the files: a wrapper and its inner binary
        // inside one package name the same installation.
        running: Some(running_installation.display().to_string()),
        installed: Some(installed_installation.display().to_string()),
        executable,
        unknown: None,
    }
}

fn file_name(path: &Path) -> Option<&str> {
    path.file_name()?.to_str()
}

/// The installation a path belongs to.
///
/// Under the Nix store that is the store root (`/nix/store/<hash>-<name>`), so
/// a package's wrapper and its inner binary agree; anywhere else it is the
/// resolved path itself.
fn installation(path: &Path) -> PathBuf {
    if let Ok(rest) = path.strip_prefix("/nix/store")
        && let Some(root) = rest.components().next()
    {
        return Path::new("/nix/store").join(root);
    }
    path.to_path_buf()
}

/// Whether `path` is a store path's root, as [`installation`] produces.
fn in_store(path: &Path) -> bool {
    path.starts_with("/nix/store/")
}

#[cfg(target_os = "linux")]
mod platform {
    use std::ffi::CString;
    use std::os::fd::RawFd;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::{
        BinaryIdentity, BinaryIndex, BinaryUnknown, LocalFacts, Procs, Stat, TerminalMode, identity,
    };

    /// This machine's process table, as [`Procs`] reads it.
    pub struct SystemProcs;

    impl Procs for SystemProcs {
        fn boot_id(&mut self) -> Option<String> {
            let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
            let boot = boot.trim();
            // An identity file that is empty names no boot.
            (!boot.is_empty()).then(|| boot.to_string())
        }

        fn pids(&mut self, limit: usize, cancel: &AtomicBool) -> Option<Vec<i32>> {
            let entries = std::fs::read_dir("/proc").ok()?;
            let mut pids: Vec<i32> = Vec::new();
            for entry in entries {
                // The enumeration is bounded and cancellable like the reads
                // that follow it: a machine with more processes than the caller
                // will read is not enumerated in full to be cut afterwards.
                if pids.len() >= limit || cancel.load(Ordering::SeqCst) {
                    break;
                }
                // Names that are not pids — `self`, `net`, the kernel's own
                // files — are not processes.
                if let Ok(entry) = entry
                    && let Some(pid) = entry
                        .file_name()
                        .to_str()
                        .and_then(|name| name.parse().ok())
                {
                    pids.push(pid);
                }
            }
            // A read that stopped at the limit is sorted like any other, so the
            // pids a caller does get are in a fixed order.
            pids.sort_unstable();
            Some(pids)
        }

        fn stat(&mut self, pid: i32) -> Option<String> {
            std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()
        }

        fn page_size(&mut self) -> Option<u64> {
            let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
            (page > 0).then_some(page as u64)
        }
    }

    pub fn facts(pid: i32, program: Option<&str>, binaries: &mut BinaryIndex) -> LocalFacts {
        LocalFacts {
            running_for: running_for(pid),
            terminal: terminal_mode(pid),
            binary: binary_identity(pid, program, binaries),
            // Sampling is not a platform read of its own: it is driven by the
            // sampler the caller handed in.
            resources: None,
        }
    }

    /// How the process's executable compares with the program installed for it.
    fn binary_identity(
        pid: i32,
        program: Option<&str>,
        binaries: &mut BinaryIndex,
    ) -> BinaryIdentity {
        // The executable is read before the name is considered: a bolt variant
        // is compared from its own package, and a runtime name that is missing,
        // empty or `pi` says nothing about which program is running.
        let Ok(link) = std::fs::read_link(format!("/proc/{pid}/exe")) else {
            return BinaryIdentity {
                unknown: Some(BinaryUnknown::Unreadable),
                ..BinaryIdentity::default()
            };
        };
        // The kernel appends " (deleted)" when the file the process started
        // from has been unlinked or replaced — the ordinary result of an
        // upgrade, and a fact about the running file on its own.
        let link = link.to_string_lossy();
        let (running, deleted) = match link.strip_suffix(super::DELETED) {
            Some(path) => (PathBuf::from(path), true),
            None => (PathBuf::from(link.as_ref()), false),
        };
        identity(&running, deleted, program, binaries)
    }

    /// How long the process has been alive.
    ///
    /// From its own start time, not from when Radar first saw it: a build that
    /// has been running for an hour when Radar starts has been running for an
    /// hour.
    fn running_for(pid: i32) -> Option<Duration> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let ticks = Stat::parse(&stat)?.start_ticks;
        let hertz = super::clock_ticks()?;
        let uptime = uptime_seconds()?;
        let elapsed = uptime - ticks as f64 / hertz as f64;
        Some(Duration::from_secs_f64(elapsed.max(0.0)))
    }

    fn uptime_seconds() -> Option<f64> {
        let uptime = std::fs::read_to_string("/proc/uptime").ok()?;
        uptime.split_whitespace().next()?.parse().ok()
    }

    /// Whether the foreground program has taken the terminal over.
    ///
    /// Read from the terminal the process itself holds, which is why the
    /// process's own `fd 0` is followed rather than Radar's own terminal: the
    /// two are different devices, and only one of them belongs to the pane.
    fn terminal_mode(pid: i32) -> TerminalMode {
        let Ok(device) = std::fs::read_link(format!("/proc/{pid}/fd/0")) else {
            return TerminalMode::Unknown;
        };
        // Only a pty: `/dev/tty` would be Radar's own controlling terminal, and
        // a virtual console or a file is not a pane at all.
        let path = device.to_string_lossy().into_owned();
        if !path.starts_with("/dev/pts/") {
            return TerminalMode::Unknown;
        }
        let Ok(fd) = open_terminal(&path) else {
            return TerminalMode::Unknown;
        };
        let mode = read_attributes(fd).map_or(TerminalMode::Unknown, mode_from_attributes);
        unsafe { libc::close(fd) };
        mode
    }

    /// Opens a terminal device for reading, without making it ours.
    fn open_terminal(path: &str) -> Result<RawFd, std::ffi::NulError> {
        let path = CString::new(path)?;
        // `O_NOCTTY` matters: opening a terminal without it can hand this
        // process a controlling terminal it did not ask for.
        let fd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_RDONLY | libc::O_NOCTTY | libc::O_NONBLOCK,
            )
        };
        Ok(fd)
    }

    fn read_attributes(fd: RawFd) -> Option<libc::termios> {
        let mut attributes = std::mem::MaybeUninit::<libc::termios>::uninit();
        let read = unsafe { libc::tcgetattr(fd, attributes.as_mut_ptr()) };
        // A negative fd lands here too, which is an unreadable terminal and not
        // a reason to fail.
        (read == 0).then(|| unsafe { attributes.assume_init() })
    }

    /// A program that has turned off canonical mode *and* echo is drawing the
    /// screen itself. One that has not is printing to a terminal the shell
    /// still owns.
    fn mode_from_attributes(attributes: libc::termios) -> TerminalMode {
        let line_discipline = attributes.c_lflag & (libc::ICANON | libc::ECHO);
        if line_discipline == 0 {
            TerminalMode::FullScreen
        } else {
            TerminalMode::Line
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::model::BinaryFreshness;

        fn attributes(line_discipline: libc::tcflag_t) -> libc::termios {
            let mut attributes: libc::termios = unsafe { std::mem::zeroed() };
            attributes.c_lflag = line_discipline;
            attributes
        }

        #[test]
        fn raw_mode_is_what_a_full_screen_program_leaves_behind() {
            assert_eq!(
                mode_from_attributes(attributes(0)),
                TerminalMode::FullScreen
            );
            assert_eq!(
                mode_from_attributes(attributes(libc::ICANON)),
                TerminalMode::Line
            );
            assert_eq!(
                mode_from_attributes(attributes(libc::ICANON | libc::ECHO)),
                TerminalMode::Line
            );
        }

        #[test]
        fn this_process_is_known_and_young() {
            let mut binaries = BinaryIndex::searching(Vec::new());
            let facts = facts(std::process::id() as i32, None, &mut binaries);
            let running = facts.running_for.expect("our own process has a start");
            assert!(running < Duration::from_secs(3600), "{running:?}");
        }

        #[test]
        fn a_process_that_is_gone_yields_unknown_facts() {
            // A pid that cannot exist: nothing is older than the process table.
            let mut binaries = BinaryIndex::searching(Vec::new());
            let facts = facts(i32::MAX, Some("pi"), &mut binaries);
            assert_eq!(facts.running_for, None);
            assert_eq!(facts.terminal, TerminalMode::Unknown);
            assert_eq!(facts.binary.freshness, BinaryFreshness::Unknown);
            assert_eq!(facts.binary.unknown, Some(BinaryUnknown::Unreadable));
            assert_eq!(facts.binary.executable, None);
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use super::{BinaryIndex, LocalFacts, Procs};

    /// Nothing is read on a platform Radar has no reader for, and the row says
    /// nothing it cannot support.
    pub fn facts(_pid: i32, _program: Option<&str>, _binaries: &mut BinaryIndex) -> LocalFacts {
        LocalFacts::default()
    }

    /// A process table this platform cannot read: every sample is unknown, and
    /// the fleet runs without them.
    pub struct SystemProcs;

    impl Procs for SystemProcs {
        fn boot_id(&mut self) -> Option<String> {
            None
        }

        fn pids(&mut self, _limit: usize, _cancel: &AtomicBool) -> Option<Vec<i32>> {
            None
        }

        fn stat(&mut self, _pid: i32) -> Option<String> {
            None
        }

        fn page_size(&mut self) -> Option<u64> {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;

    fn identity_of(running: &str, deleted: bool, installed: &str) -> BinaryIdentity {
        compare_named(Path::new(running), deleted, Path::new(installed))
    }

    #[test]
    fn a_store_root_name_yields_its_package_and_version() {
        assert_eq!(
            store_package("hxdd4znbva7jbas38cr1piavy5ig67j8-pi-bolt-0.7.1"),
            Some(("pi-bolt", "0.7.1"))
        );
        assert_eq!(
            store_package("ijm3q5j0w25i5iw12sl8ql7kxfwmzns2-pi-bolt-child-0.7.1"),
            Some(("pi-bolt-child", "0.7.1"))
        );
        // An unversioned launcher package is not a versioned package, and a
        // name with no store hash is not a store root name.
        assert_eq!(
            store_package("zyhpxvlrpvm7xc29p8p41ysqwxs57ryh-pi-bolt"),
            None
        );
        assert_eq!(store_package("pi-bolt"), None);
        assert_eq!(store_package("pi-bolt-child"), None);
        // A version that does not start with a digit is not read as one.
        assert_eq!(store_package("aaa-release-candidate"), None);
    }

    #[test]
    fn only_a_versioned_bolt_package_names_a_family() {
        assert_eq!(
            bolt_family(Path::new("/nix/store/aaa-pi-bolt-0.7.1/lib/pi-bolt/pi")),
            Some("pi-bolt")
        );
        assert_eq!(
            bolt_family(Path::new(
                "/nix/store/bbb-pi-bolt-child-0.7.1/lib/pi-bolt/pi"
            )),
            Some("pi-bolt-child")
        );
        // A name that merely starts with a family is not one of them, and a
        // program outside the store has no package at all.
        assert_eq!(
            bolt_family(Path::new("/nix/store/ccc-pi-boltX-0.7.1/lib/pi-bolt/pi")),
            None
        );
        assert_eq!(bolt_family(Path::new("/usr/local/bin/pi")), None);
        assert_eq!(
            bolt_family(Path::new("/nix/store/ddd-pi-1.0.4/bin/pi")),
            None
        );
    }

    #[test]
    fn a_versioned_packages_own_entrypoint_names_its_own_root() {
        // The shape a derivation provides: `bin/pi-bolt` beside `lib/pi-bolt/pi`
        // in one root needs no reading, and the payload root is that root.
        let root = "/nix/store/aaa-pi-bolt-0.7.1";
        assert_eq!(
            counterpart_root(
                Path::new("/nix/store/aaa-pi-bolt-0.7.1/bin/pi-bolt"),
                "pi-bolt"
            ),
            Ok(PathBuf::from(root))
        );
        // The unversioned launcher package in the profile is not a payload root:
        // its root holds no payload, and it carries no versioned package name.
        assert_eq!(
            counterpart_root(Path::new("/nix/store/zzz-pi-bolt/bin/pi-bolt"), "pi-bolt"),
            Err(BinaryUnknown::UnsupportedLauncher)
        );
    }

    #[test]
    fn a_wrapper_and_its_inner_binary_are_one_installation() {
        // The shape a Nix package has: the PATH match is a different file from
        // the running one, inside one store root.
        let identity = identity_of(
            "/nix/store/aaa-pi-1.0.2/libexec/pi/pi",
            false,
            "/nix/store/aaa-pi-1.0.2/bin/pi",
        );
        assert_eq!(identity.freshness, BinaryFreshness::Current);
        assert_eq!(identity.running.as_deref(), Some("/nix/store/aaa-pi-1.0.2"));
        assert_eq!(
            identity.installed.as_deref(),
            Some("/nix/store/aaa-pi-1.0.2")
        );
    }

    #[test]
    fn a_changed_store_root_is_stale() {
        let identity = identity_of(
            "/nix/store/aaa-pi-1.0.2/libexec/pi/pi",
            false,
            "/nix/store/bbb-pi-1.0.3/bin/pi",
        );
        assert_eq!(identity.freshness, BinaryFreshness::Stale);
    }

    #[test]
    fn a_replaced_executable_is_stale_even_without_a_store_change() {
        let identity = identity_of(
            "/nix/store/aaa-pi-1.0.2/libexec/pi/pi",
            true,
            "/nix/store/aaa-pi-1.0.2/bin/pi",
        );
        assert_eq!(identity.freshness, BinaryFreshness::Stale);
    }

    #[test]
    fn a_deliberate_other_build_is_not_stale() {
        // A checkout or dev build is not the installed program, but nothing
        // about it says the running one was replaced.
        let identity = identity_of(
            "/home/dev/agent-radar/target/debug/pi",
            false,
            "/nix/store/bbb-pi-1.0.3/libexec/pi/pi",
        );
        assert_eq!(identity.freshness, BinaryFreshness::Current);
        assert_eq!(
            identity.running.as_deref(),
            Some("/home/dev/agent-radar/target/debug/pi")
        );
        assert_eq!(
            identity.installed.as_deref(),
            Some("/nix/store/bbb-pi-1.0.3")
        );
    }

    #[test]
    fn a_non_store_program_on_path_is_current() {
        let identity = identity_of("/usr/local/bin/pi", false, "/usr/local/bin/pi");
        assert_eq!(identity.freshness, BinaryFreshness::Current);
    }

    #[test]
    fn an_interpreter_is_never_attributed_to_the_agent() {
        // The runtime reports the agent as `pi`, but the foreground process is
        // a python interpreter: nothing is claimed.
        let identity = identity_of("/usr/bin/python3", false, "/nix/store/bbb-pi-1.0.3/bin/pi");
        assert_eq!(identity.freshness, BinaryFreshness::Unknown);
        assert_eq!(identity.unknown, Some(BinaryUnknown::NotCompared));
        assert_eq!(identity.running, None);
        assert_eq!(identity.installed, None);
        // What is running is still nameable, without being attributed.
        assert_eq!(identity.executable.as_deref(), Some("/usr/bin/python3"));
    }

    /// A stat line with the fields Radar reads, as the kernel writes them. The
    /// command name is a parenthesised name with parentheses and a space in it,
    /// which is the shape that hides the field boundaries.
    fn line(
        pid: i32,
        state: char,
        parent: i32,
        cpu_ticks: u64,
        start_ticks: u64,
        rss_pages: i64,
    ) -> String {
        named(
            pid,
            "we (ird) name",
            state,
            parent,
            cpu_ticks,
            start_ticks,
            rss_pages,
        )
    }

    /// The same line under a name a test chose, so the rows of two processes can
    /// be told apart by what the kernel called them.
    fn named(
        pid: i32,
        name: &str,
        state: char,
        parent: i32,
        cpu_ticks: u64,
        start_ticks: u64,
        rss_pages: i64,
    ) -> String {
        format!(
            "{pid} ({name}) {state} {parent} 42 42 0 -1 4194560 100 0 0 0 \
             {utime} {stime} 3 4 20 0 3 0 {start_ticks} 0 {rss_pages}",
            utime = cpu_ticks / 2,
            stime = cpu_ticks - cpu_ticks / 2,
        )
    }

    /// That line for pid 42, a child of pid 1: the process most tests sample.
    fn stat_line(state: char, cpu_ticks: u64, start_ticks: u64, rss_pages: i64) -> String {
        line(42, state, 1, cpu_ticks, start_ticks, rss_pages)
    }

    /// One process in a scripted table: the pid it answers as, and the line it
    /// answers with. A pid paired with `None` is a process Radar may not read.
    fn proc(
        pid: i32,
        parent: i32,
        start_ticks: u64,
        cpu_ticks: u64,
        rss_pages: i64,
    ) -> (i32, Option<String>) {
        (
            pid,
            Some(line(pid, 'S', parent, cpu_ticks, start_ticks, rss_pages)),
        )
    }

    /// The same process under a name and state a test chose.
    fn named_proc(
        pid: i32,
        name: &str,
        state: char,
        parent: i32,
        start_ticks: u64,
        cpu_ticks: u64,
        rss_pages: i64,
    ) -> (i32, Option<String>) {
        (
            pid,
            Some(named(
                pid,
                name,
                state,
                parent,
                cpu_ticks,
                start_ticks,
                rss_pages,
            )),
        )
    }

    /// A process table a test scripts, so nothing here reads this machine.
    ///
    /// The table is shared with every clone of the handle, so a test can change
    /// what a later read finds — a child that exits, appears or is replaced
    /// between refreshes — and can ask which pids were read. A pid absent from
    /// the table is one the kernel no longer reports; one listed with no line is
    /// a process Radar is not allowed to read.
    #[derive(Clone)]
    struct ScriptedProcs {
        table: Arc<Mutex<Table>>,
    }

    /// What one scripted machine answers, and what it was asked.
    struct Table {
        /// The boot identities successive refreshes see. The last one stands.
        boots: Vec<Option<String>>,
        /// The line a read of each pid answers, `None` when it cannot be read.
        lines: HashMap<i32, Option<String>>,
        /// Raw answers a pid gives to successive reads, where a test has to move
        /// the process between the reads of a single sample.
        scripted: HashMap<i32, Vec<Option<String>>>,
        /// Every pid `stat` was asked for, in order.
        reads: Vec<i32>,
        /// Every enumeration limit `pids` was asked for, in order.
        limits: Vec<usize>,
        /// Trips this flag once a given number of pids have been read, so a
        /// refresh can be cancelled at a point a test chose.
        trip: Option<(usize, Arc<AtomicBool>)>,
        page_size: Option<u64>,
    }

    impl ScriptedProcs {
        fn new() -> Self {
            Self {
                table: Arc::new(Mutex::new(Table {
                    boots: vec![Some("boot-1".to_string())],
                    lines: HashMap::new(),
                    scripted: HashMap::new(),
                    reads: Vec::new(),
                    limits: Vec::new(),
                    trip: None,
                    page_size: Some(4096),
                })),
            }
        }

        /// The processes this machine reports: each pid's line, and `None` for a
        /// process Radar may not read.
        fn holding(self, rows: Vec<(i32, Option<String>)>) -> Self {
            self.table.lock().expect("table").lines.extend(rows);
            self
        }

        /// How `pid` looks at each sample: every listed line is what both reads
        /// of that sample find.
        fn reading(self, pid: i32, samples: Vec<String>) -> Self {
            let answers = samples
                .into_iter()
                .flat_map(|stat| [Some(stat.clone()), Some(stat)])
                .collect();
            self.answers(pid, answers)
        }

        /// The raw answers `pid` gives to successive reads, `None` for a read
        /// that finds no process.
        fn answers(self, pid: i32, answers: Vec<Option<String>>) -> Self {
            self.table
                .lock()
                .expect("table")
                .scripted
                .insert(pid, answers);
            self
        }

        /// The boot identities a reboot between refreshes moves through.
        fn booting(self, boots: Vec<Option<String>>) -> Self {
            self.table.lock().expect("table").boots = boots;
            self
        }

        /// A machine that cannot say how large its pages are.
        fn without_page_size(self) -> Self {
            self.table.lock().expect("table").page_size = None;
            self
        }

        /// Sets `cancel` once `reads` pids have been asked for.
        fn cancelling_after(self, reads: usize, cancel: Arc<AtomicBool>) -> Self {
            self.table.lock().expect("table").trip = Some((reads, cancel));
            self
        }

        /// What a read of `pid` finds from now on, or that it finds nothing at
        /// all because the process is gone.
        fn now_reading(&self, pid: i32, stat: Option<String>) {
            let mut table = self.table.lock().expect("table");
            // A process that is gone is not read, so a script left for it cannot
            // answer for whatever holds the pid next.
            table.scripted.remove(&pid);
            match stat {
                Some(stat) => table.lines.insert(pid, Some(stat)),
                None => table.lines.remove(&pid),
            };
        }

        /// Which pids were read, in order, and forgets that they were.
        fn reads(&self) -> Vec<i32> {
            std::mem::take(&mut self.table.lock().expect("table").reads)
        }

        /// Which enumeration limits were asked for, and forgets that they were.
        fn enumeration_limits(&self) -> Vec<usize> {
            std::mem::take(&mut self.table.lock().expect("table").limits)
        }

        /// The next of `answers`, leaving the last one standing.
        fn next<T: Clone>(answers: &mut Vec<Option<T>>) -> Option<T> {
            if answers.len() > 1 {
                answers.remove(0)
            } else {
                answers.first().cloned().flatten()
            }
        }
    }

    impl Procs for ScriptedProcs {
        fn boot_id(&mut self) -> Option<String> {
            let mut table = self.table.lock().expect("table");
            if table.boots.len() > 1 {
                table.boots.remove(0)
            } else {
                table.boots.first().cloned().flatten()
            }
        }

        fn pids(&mut self, limit: usize, _cancel: &AtomicBool) -> Option<Vec<i32>> {
            let mut table = self.table.lock().expect("table");
            table.limits.push(limit);
            let mut pids: Vec<i32> = table.lines.keys().copied().collect();
            pids.sort_unstable();
            // A table larger than the caller will read is cut short here, as the
            // machine's own enumeration is, rather than enumerated in full.
            pids.truncate(limit);
            Some(pids)
        }

        fn stat(&mut self, pid: i32) -> Option<String> {
            let mut table = self.table.lock().expect("table");
            table.reads.push(pid);
            let tripped = table
                .trip
                .as_ref()
                .filter(|(after, _)| table.reads.len() >= *after);
            if let Some((_, cancel)) = tripped {
                cancel.store(true, Ordering::SeqCst);
            }
            match table.scripted.get_mut(&pid) {
                Some(answers) => Self::next(answers),
                None => table.lines.get(&pid).cloned().flatten(),
            }
        }

        fn page_size(&mut self) -> Option<u64> {
            self.table.lock().expect("table").page_size
        }
    }

    /// A sampler over `procs`, and the instant it samples at.
    fn sampler_over(procs: ScriptedProcs) -> (Sampler, Instant) {
        (Sampler::reading(procs), Instant::now())
    }

    #[test]
    fn a_stat_line_is_read_from_its_last_parenthesis() {
        let stat = Stat::parse(&stat_line('S', 30, 987_654, 8)).expect("a stat line");
        // The name is parenthesised and may hold spaces and parentheses of its
        // own: it is the first `(` to the last `)`, and the fields are counted
        // from what follows.
        assert_eq!(stat.name, "we (ird) name");
        assert_eq!(stat.state, ProcessState::Sleeping);
        assert_eq!(stat.cpu_ticks, 30);
        assert_eq!(stat.start_ticks, 987_654);
        assert_eq!(stat.rss_pages, 8);
        // A line the kernel did not write is not a sample of anything, and a
        // line whose fields are missing is only half a sample.
        assert!(Stat::parse("42 (short) R 1 2").is_none());
        assert!(Stat::parse("not a stat line").is_none());
        // Parentheses the wrong way round are not a name either, rather than a
        // slice of the line's own bytes.
        assert!(Stat::parse("42 ) S 1 (").is_none());
    }

    #[test]
    fn a_state_the_kernel_names_and_radar_does_not_is_still_reported() {
        let stat = Stat::parse(&stat_line('P', 0, 100, 0)).expect("a stat line");
        assert_eq!(stat.state, ProcessState::Other('P'));
    }

    #[test]
    fn a_first_sample_has_state_and_rss_but_no_cpu() {
        let (mut sampler, at) =
            sampler_over(ScriptedProcs::new().reading(42, vec![stat_line('R', 1_000, 100, 8)]));
        let sample = sampler.sample(42, at).expect("a sample");
        assert_eq!(
            sample.identity,
            ProcessIdentity {
                boot_id: "boot-1".to_string(),
                pid: 42,
                start_ticks: 100,
            }
        );
        assert_eq!(sample.state, ProcessState::Running);
        assert_eq!(sample.rss_bytes, Some(8 * 4096));
        // Nothing to compare with yet: unavailable, never zero.
        assert_eq!(sample.cpu, None);
    }

    #[test]
    fn a_second_sample_measures_the_interval_of_one_cpu() {
        let hertz = clock_ticks().expect("this kernel knows its tick rate") as u64;
        let (mut sampler, at) = sampler_over(ScriptedProcs::new().reading(
            42,
            vec![
                stat_line('R', 1_000, 100, 4),
                stat_line('R', 1_000 + hertz, 100, 4),
            ],
        ));
        sampler.sample(42, at).expect("the first sample");
        // One CPU-second of work over two seconds of wall clock is half of one
        // CPU, however many ticks this kernel counts in a second.
        let sample = sampler
            .sample(42, at + Duration::from_secs(2))
            .expect("the second sample");
        assert_eq!(sample.cpu, Some(CpuPercent::from_hundredths(5_000)));
    }

    #[test]
    fn work_on_more_than_one_cpu_can_exceed_one_hundred_percent() {
        let hertz = clock_ticks().expect("this kernel knows its tick rate") as u64;
        let (mut sampler, at) = sampler_over(ScriptedProcs::new().reading(
            42,
            vec![stat_line('R', 0, 100, 4), stat_line('R', 2 * hertz, 100, 4)],
        ));
        sampler.sample(42, at).expect("the first sample");
        // Two CPU-seconds over one second: this process ran on two CPUs at once.
        let sample = sampler
            .sample(42, at + Duration::from_secs(1))
            .expect("the second sample");
        assert_eq!(sample.cpu, Some(CpuPercent::from_hundredths(20_000)));
    }

    #[test]
    fn an_idle_interval_is_measured_zero_not_unknown() {
        let (mut sampler, at) = sampler_over(ScriptedProcs::new().reading(
            42,
            vec![stat_line('S', 1_000, 100, 4), stat_line('S', 1_000, 100, 4)],
        ));
        sampler.sample(42, at).expect("the first sample");
        let sample = sampler
            .sample(42, at + Duration::from_secs(1))
            .expect("the second sample");
        assert_eq!(sample.cpu, Some(CpuPercent::from_hundredths(0)));
    }

    #[test]
    fn an_interval_that_cannot_be_measured_is_unknown() {
        let hertz = clock_ticks().expect("this kernel knows its tick rate") as u64;

        // Two readings in one instant are no interval at all.
        let (mut sampler, at) = sampler_over(ScriptedProcs::new().reading(
            42,
            vec![
                stat_line('R', 1_000, 100, 4),
                stat_line('R', 1_000 + hertz, 100, 4),
            ],
        ));
        sampler.sample(42, at).expect("the first sample");
        let same = sampler.sample(42, at).expect("a sample");
        assert_eq!(same.cpu, None);

        // A counter that went backwards did not come from one process, however
        // well the identity matched.
        let (mut sampler, at) = sampler_over(ScriptedProcs::new().reading(
            42,
            vec![stat_line('R', 5_000, 100, 4), stat_line('R', 10, 100, 4)],
        ));
        sampler.sample(42, at).expect("the first sample");
        let regressed = sampler
            .sample(42, at + Duration::from_secs(1))
            .expect("the second sample");
        assert_eq!(regressed.cpu, None);

        // A clock that went backwards measures nothing either.
        let (mut sampler, at) = sampler_over(ScriptedProcs::new().reading(
            42,
            vec![
                stat_line('R', 1_000, 100, 4),
                stat_line('R', 1_000 + hertz, 100, 4),
            ],
        ));
        sampler.sample(42, at).expect("the first sample");
        let earlier = sampler
            .sample(42, at - Duration::from_secs(1))
            .expect("a sample");
        assert_eq!(earlier.cpu, None);
    }

    #[test]
    fn a_reused_pid_starts_a_new_baseline() {
        let hertz = clock_ticks().expect("this kernel knows its tick rate") as u64;
        // The same pid, a different start time: the earlier process exited and
        // the kernel handed its pid to another one.
        let (mut sampler, at) = sampler_over(ScriptedProcs::new().reading(
            42,
            vec![
                stat_line('R', 1_000, 100, 4),
                stat_line('R', 1_000 + hertz, 200, 8),
                stat_line('R', 1_000 + 2 * hertz, 200, 8),
            ],
        ));
        sampler.sample(42, at).expect("the first sample");
        let reused = sampler
            .sample(42, at + Duration::from_secs(1))
            .expect("the replacement");
        assert_eq!(reused.identity.start_ticks, 200);
        assert_eq!(reused.cpu, None, "no delta may cross two incarnations");
        // The replacement's own history is measured from its first reading.
        let measured = sampler
            .sample(42, at + Duration::from_secs(2))
            .expect("the next sample");
        assert_eq!(measured.cpu, Some(CpuPercent::from_hundredths(10_000)));
    }

    #[test]
    fn a_pid_reused_between_reads_yields_no_sample() {
        // The line describes one process and the read that confirms it another:
        // the pid was handed on between the two reads of the sample, so the
        // sample describes neither process and is not published.
        let (mut sampler, at) = sampler_over(ScriptedProcs::new().answers(
            42,
            vec![
                Some(stat_line('R', 0, 100, 4)),
                Some(stat_line('R', 0, 200, 8)),
            ],
        ));
        assert_eq!(sampler.sample(42, at), None);

        // The replacement's own history starts with it: no delta crosses the
        // two incarnations, and nothing was kept for the identity the abandoned
        // sample described.
        let replacement = sampler
            .sample(42, at + Duration::from_secs(1))
            .expect("a sample of the replacement");
        assert_eq!(replacement.identity.start_ticks, 200);
        assert_eq!(replacement.rss_bytes, Some(8 * 4096));
        assert_eq!(replacement.cpu, None);
    }

    #[test]
    fn a_process_that_disappears_between_reads_yields_no_sample() {
        let hertz = clock_ticks().expect("this kernel knows its tick rate") as u64;
        // The line was read, and the read that confirms it found nothing: the
        // process exited inside its own sample. The same process is readable
        // again on the next refresh — a read can fail and succeed — and a
        // baseline kept from the abandoned sample would be measured here.
        let (mut sampler, at) = sampler_over(ScriptedProcs::new().answers(
            42,
            vec![
                Some(stat_line('R', 1_000, 100, 4)),
                None,
                Some(stat_line('R', 1_000 + hertz, 100, 4)),
                Some(stat_line('R', 1_000 + 2 * hertz, 100, 4)),
            ],
        ));
        assert_eq!(sampler.sample(42, at), None);
        let first = sampler
            .sample(42, at + Duration::from_secs(1))
            .expect("a sample");
        assert_eq!(first.cpu, None, "an abandoned sample leaves no baseline");
        // Only a sample that was published can carry an interval: the next one
        // is measured against the sample above, not against the abandoned one.
        let second = sampler
            .sample(42, at + Duration::from_secs(2))
            .expect("a sample");
        assert_eq!(second.cpu, Some(CpuPercent::from_hundredths(10_000)));
    }

    #[test]
    fn a_reboot_starts_a_new_baseline() {
        let hertz = clock_ticks().expect("this kernel knows its tick rate") as u64;
        // The same pid and start ticks under a different boot identity: the
        // ticks are counted from a different boot, so they are not comparable.
        let (mut sampler, at) = sampler_over(
            ScriptedProcs::new()
                .booting(vec![Some("boot-1".to_string()), Some("boot-2".to_string())])
                .reading(
                    42,
                    vec![
                        stat_line('R', 1_000, 100, 4),
                        stat_line('R', 1_000 + hertz, 100, 4),
                    ],
                ),
        );
        sampler.sample(42, at).expect("the first sample");
        sampler.finish();
        let rebooted = sampler
            .sample(42, at + Duration::from_secs(1))
            .expect("the sample after the reboot");
        assert_eq!(rebooted.identity.boot_id, "boot-2");
        assert_eq!(rebooted.cpu, None);
    }

    #[test]
    fn a_process_seen_in_every_refresh_keeps_its_baseline() {
        let hertz = clock_ticks().expect("this kernel knows its tick rate") as u64;
        let (mut sampler, at) = sampler_over(ScriptedProcs::new().reading(
            42,
            vec![
                stat_line('R', 1_000, 100, 4),
                stat_line('R', 1_000 + hertz, 100, 4),
            ],
        ));
        sampler.sample(42, at).expect("the first sample");
        sampler.finish();
        let sample = sampler
            .sample(42, at + Duration::from_secs(1))
            .expect("the next refresh's sample");
        assert_eq!(sample.cpu, Some(CpuPercent::from_hundredths(10_000)));
    }

    #[test]
    fn the_history_is_pruned_to_the_processes_of_the_last_refresh() {
        let (mut sampler, at) = sampler_over(
            ScriptedProcs::new()
                .reading(42, vec![stat_line('R', 1_000, 100, 4)])
                .reading(43, vec![stat_line('R', 1_000, 100, 4)]),
        );
        sampler.sample(42, at).expect("a sample");
        sampler.finish();
        // A refresh that saw only the other process: the first one's counters
        // are not kept for a process that is no longer running.
        sampler.sample(43, at).expect("a sample");
        sampler.finish();
        let later = sampler
            .sample(42, at + Duration::from_secs(1))
            .expect("a sample");
        assert_eq!(later.identity.pid, 42);
        assert_eq!(later.cpu, None, "a pruned baseline has no interval");
    }

    #[test]
    fn a_process_that_cannot_be_read_yields_no_sample() {
        // Nothing in the scripted table: the process is gone before it is read.
        let (mut sampler, at) = sampler_over(ScriptedProcs::new());
        assert_eq!(sampler.sample(42, at), None);

        // A line the kernel did not write, and a boot identity the kernel does
        // not report, are both unknown rather than a guess.
        let (mut sampler, at) =
            sampler_over(ScriptedProcs::new().reading(42, vec!["   ".to_string()]));
        assert_eq!(sampler.sample(42, at), None);
        let (mut sampler, at) = sampler_over(
            ScriptedProcs::new()
                .booting(vec![None])
                .reading(42, vec![stat_line('R', 1_000, 100, 4)]),
        );
        assert_eq!(sampler.sample(42, at), None);
    }

    #[test]
    fn a_page_count_that_is_not_a_size_leaves_rss_unknown() {
        let (mut sampler, at) =
            sampler_over(ScriptedProcs::new().reading(42, vec![stat_line('R', 0, 100, -1)]));
        let sample = sampler.sample(42, at).expect("a sample");
        assert_eq!(sample.rss_bytes, None);
        // The rest of the sample is still this process's.
        assert_eq!(sample.state, ProcessState::Running);

        // A machine whose page size cannot be read converts nothing.
        let procs = ScriptedProcs::new()
            .reading(42, vec![stat_line('R', 0, 100, 4)])
            .without_page_size();
        let (mut sampler, at) = sampler_over(procs);
        let sample = sampler.sample(42, at).expect("a sample");
        assert_eq!(sample.rss_bytes, None);
        assert_eq!(sample.identity.pid, 42);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn this_machine_samples_its_own_process() {
        let mut binaries = BinaryIndex::searching(Vec::new());
        let mut sampler = Sampler::new();
        let facts = facts(
            std::process::id() as i32,
            None,
            &mut binaries,
            &mut sampler,
            Instant::now(),
        );
        let resources = facts.resources.expect("our own process has a sample");
        assert_eq!(resources.identity.pid, std::process::id() as i32);
        assert!(!resources.identity.boot_id.is_empty());
        assert!(resources.rss_bytes.expect("pages convert") > 0);
        // The first sample of a process has nothing to compare with.
        assert_eq!(resources.cpu, None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_pid_that_cannot_exist_yields_no_sample() {
        let mut binaries = BinaryIndex::searching(Vec::new());
        let mut sampler = Sampler::new();
        let facts = facts(
            i32::MAX,
            Some("pi"),
            &mut binaries,
            &mut sampler,
            Instant::now(),
        );
        assert_eq!(facts.resources, None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn this_machine_is_enumerated_and_read_within_the_budget() {
        let cancel = AtomicBool::new(false);
        let total = platform::SystemProcs
            .pids(usize::MAX, &cancel)
            .expect("this machine's process table")
            .len();
        let mut sampler = Sampler::new();
        sampler.budget = 3;
        sampler.scan(Instant::now(), &cancel);
        let snapshot = sampler.snapshot.as_ref().expect("a snapshot");
        // The scan read the pids it was allowed and the confirmation pass read
        // the same ones again, rather than the table being read in full.
        assert!(!snapshot.entries.is_empty(), "this machine has processes");
        assert!(snapshot.entries.len() <= total.min(3));
        if total > 3 {
            assert!(matches!(
                &snapshot.scan,
                Scan::Partial(reason) if reason.contains("3-process scan budget")
            ));
        }
    }

    /// A snapshot of `sampler`'s process table, at `at`, that runs to the end.
    fn scan(sampler: &mut Sampler, at: Instant) {
        sampler.scan(at, &AtomicBool::new(false));
    }

    #[test]
    fn a_root_sums_what_is_beneath_it_and_a_nested_root_counts_it_too() {
        let hertz = clock_ticks().expect("this kernel knows its tick rate") as u64;
        // A pane (100) over a shell (200) over a build (300), and a sibling (400)
        // that is beneath none of them.
        let procs = ScriptedProcs::new().holding(vec![
            proc(100, 1, 10, 0, 4),
            proc(200, 100, 20, 1_000, 2),
            proc(300, 200, 30, 2_000, 1),
            proc(400, 1, 40, 0, 8),
        ]);
        let table = procs.clone();
        let (mut sampler, at) = sampler_over(procs);
        scan(&mut sampler, at);
        let root = sampler.sample(100, at).expect("the root's sample");
        let shell = sampler.sample(200, at).expect("the shell's sample");

        // The root's own resident set is its own: the build's is not added to it,
        // the sibling's is not beneath it, and the shell's total is neither.
        assert_eq!(root.rss_bytes, Some(4 * 4096));
        assert_eq!(root.descendants.observed, Some(2));
        assert_eq!(root.descendants.rss_bytes, Total::Complete(3 * 4096));
        // A process beneath two sampled roots is counted in both of their totals
        // while the refresh reads it once — and confirms it once, in the pass the
        // whole refresh shares.
        assert_eq!(shell.descendants.observed, Some(1));
        assert_eq!(shell.descendants.rss_bytes, Total::Complete(4096));
        // One row per member the walk confirmed, and the nested root's row is
        // the build alone: what a total covers is what its rows say.
        assert_eq!(root.descendants.members.len(), 2);
        assert_eq!(shell.descendants.members.len(), 1);
        assert_eq!(
            table.reads(),
            vec![
                100, 200, 300, 400, // the scan
                100, 200, 300, 400, // its confirmation, once for every root
                100, 100, // the root's own sample, read and confirmed
                200, 200, // the nested root's sample
            ]
        );

        // The scan was the first reading of every process beneath the root, so
        // there is no interval to measure and the total is a lower bound rather
        // than a zero.
        assert_eq!(
            root.descendants.cpu,
            Total::Partial(
                CpuPercent::from_hundredths(0),
                "no CPU sample could be compared with for 2 of them".to_string()
            )
        );

        // A second refresh, over a table whose children have run throughout it.
        table.now_reading(200, Some(line(200, 'S', 100, 1_000 + hertz, 20, 2)));
        table.now_reading(300, Some(line(300, 'S', 200, 2_000 + hertz / 2, 30, 1)));
        let later = at + Duration::from_secs(1);
        scan(&mut sampler, later);
        let root = sampler.sample(100, later).expect("the root's sample");
        // The shell ran for the whole second and the build for half of it.
        assert_eq!(
            root.descendants.cpu,
            Total::Complete(CpuPercent::from_hundredths(10_000 + 5_000))
        );
    }

    #[test]
    fn a_root_carries_a_verified_row_for_each_process_beneath_it() {
        // A pane (100) running a shell (200) that started a build (300), and a
        // sibling (400) beneath none of them.
        let procs = ScriptedProcs::new().holding(vec![
            named_proc(100, "pi", 'S', 1, 10, 0, 4),
            named_proc(200, "bash", 'S', 100, 20, 1_000, 2),
            named_proc(300, "cargo", 'R', 200, 30, 2_000, 1),
            named_proc(400, "sleep", 'S', 1, 40, 0, 8),
        ]);
        let table = procs.clone();
        let (mut sampler, at) = sampler_over(procs);
        scan(&mut sampler, at);
        let root = sampler.sample(100, at).expect("the root's sample");
        let rows = &root.descendants.members;

        // The rows are the processes beneath the root: not the root itself, and
        // not a process that is nobody's descendant here.
        assert_eq!(root.descendants.observed, Some(2));
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .all(|row| row.identity.pid != 100 && row.identity.pid != 400),
            "{rows:?}"
        );

        // Each row is one process: the incarnation the scan read, the process it
        // was read beneath — an identity, not a pid that may name somebody else
        // by the next refresh — the kernel's own name for it, and its own
        // resources.
        let shell = rows
            .iter()
            .find(|row| row.identity.pid == 200)
            .expect("the shell's row");
        assert_eq!(shell.identity.start_ticks, 20);
        assert_eq!(shell.identity.boot_id, "boot-1");
        assert_eq!(
            (shell.parent.pid, shell.parent.start_ticks),
            (100, 10),
            "the row names the process it was read beneath"
        );
        assert_eq!(shell.name, "bash");
        assert_eq!(shell.state, ProcessState::Sleeping);
        assert_eq!(shell.rss_bytes, Some(2 * 4096));
        // The scan was the first reading of the shell, so its interval is
        // unavailable rather than zero.
        assert_eq!(shell.cpu, None);

        // The build is read beneath the shell, and the state the kernel reported
        // travels with it.
        let build = rows
            .iter()
            .find(|row| row.identity.pid == 300)
            .expect("the build's row");
        assert_eq!(build.parent, shell.identity);
        assert_eq!(build.name, "cargo");
        assert_eq!(build.state, ProcessState::Running);

        // The rows cost no reading of their own: the refresh's scan read the
        // table once, the confirmation read it again, and the root's own sample
        // was read and confirmed — and nothing else was asked of the machine.
        assert_eq!(
            table.reads(),
            vec![100, 200, 300, 400, 100, 200, 300, 400, 100, 100]
        );
    }

    #[test]
    fn a_member_row_measures_its_own_interval() {
        let hertz = clock_ticks().expect("this kernel knows its tick rate") as u64;
        let procs = ScriptedProcs::new().holding(vec![
            named_proc(100, "pi", 'S', 1, 10, 0, 4),
            named_proc(200, "cargo", 'R', 100, 20, 1_000, 2),
        ]);
        let table = procs.clone();
        let (mut sampler, at) = sampler_over(procs);
        scan(&mut sampler, at);
        let root = sampler.sample(100, at).expect("the root's sample");
        assert_eq!(
            root.descendants.members[0].cpu, None,
            "a first reading has no interval"
        );
        sampler.finish();

        // The build worked for half of the second the next refresh covers.
        table.now_reading(
            200,
            Some(named(200, "cargo", 'R', 100, 1_000 + hertz / 2, 20, 2)),
        );
        let later = at + Duration::from_secs(1);
        scan(&mut sampler, later);
        let root = sampler.sample(100, later).expect("the root's sample");
        let row = &root.descendants.members[0];
        assert_eq!(row.identity.pid, 200);
        assert_eq!(row.identity.start_ticks, 20);
        assert_eq!(row.cpu, Some(CpuPercent::from_hundredths(5_000)));
        // The row carries the reading the total is made of rather than a second
        // measurement of the same process.
        assert_eq!(
            root.descendants.cpu,
            Total::Complete(CpuPercent::from_hundredths(5_000))
        );
    }

    #[test]
    fn a_cycle_in_the_ancestry_walk_is_reported_and_ends() {
        // Two processes each naming the other as its parent, which one read of a
        // table being written is free to show: a walk that trusted it would never
        // come back.
        let procs =
            ScriptedProcs::new().holding(vec![proc(100, 200, 10, 0, 4), proc(200, 100, 20, 0, 2)]);
        let (mut sampler, at) = sampler_over(procs);
        scan(&mut sampler, at);
        let root = sampler.sample(100, at).expect("the root's sample");
        assert_eq!(root.descendants.observed, Some(1));
        // The one link that could be real is a row, with the process it was read
        // beneath: only the link that closed the cycle is refused.
        assert_eq!(root.descendants.members.len(), 1);
        assert_eq!(root.descendants.members[0].parent.pid, 100);
        assert_eq!(
            root.descendants.rss_bytes,
            Total::Partial(
                2 * 4096,
                "the process table read contained a cycle".to_string()
            )
        );
        assert_eq!(
            root.descendants.cpu,
            Total::Partial(
                CpuPercent::from_hundredths(0),
                "the process table read contained a cycle; \
                 no CPU sample could be compared with for 1 of them"
                    .to_string()
            )
        );
    }

    #[test]
    fn a_link_to_a_reused_pid_is_not_beneath_its_parent() {
        // The ancestry link names a pid that has been handed on: the process
        // holding it started before the one the link calls its parent.
        let procs =
            ScriptedProcs::new().holding(vec![proc(100, 1, 500, 0, 4), proc(300, 100, 100, 0, 1)]);
        let (mut sampler, at) = sampler_over(procs);
        scan(&mut sampler, at);
        let root = sampler.sample(100, at).expect("the root's sample");
        assert_eq!(root.descendants.observed, Some(0));
        assert!(
            root.descendants.members.is_empty(),
            "a row is only drawn for a link the walk confirmed"
        );
        assert_eq!(
            root.descendants.rss_bytes,
            Total::Partial(0, "an ancestry link pointed at a reused pid".to_string())
        );
        assert_eq!(
            root.descendants.cpu,
            Total::Partial(
                CpuPercent::from_hundredths(0),
                "an ancestry link pointed at a reused pid".to_string()
            )
        );
    }

    #[test]
    fn a_child_that_exits_stops_counting_and_one_that_appears_is_unmeasured() {
        let procs = ScriptedProcs::new()
            .holding(vec![proc(100, 1, 10, 0, 4), proc(200, 100, 20, 1_000, 2)]);
        let table = procs.clone();
        let (mut sampler, at) = sampler_over(procs);
        scan(&mut sampler, at);
        let root = sampler.sample(100, at).expect("the root's sample");
        assert_eq!(root.descendants.observed, Some(1));
        assert_eq!(root.descendants.rss_bytes, Total::Complete(2 * 4096));
        sampler.finish();

        // The child exits: it is not read, and its size leaves the total instead
        // of being carried as a process that is not there.
        table.now_reading(200, None);
        let later = at + Duration::from_secs(1);
        scan(&mut sampler, later);
        let root = sampler.sample(100, later).expect("the root's sample");
        assert_eq!(root.descendants.observed, Some(0));
        assert!(
            root.descendants.members.is_empty(),
            "a process that exited leaves no row"
        );
        assert_eq!(root.descendants.rss_bytes, Total::Complete(0));
        assert_eq!(
            root.descendants.cpu,
            Total::Complete(CpuPercent::from_hundredths(0))
        );
        sampler.finish();

        // A new child appears beneath the root. It is counted, and it has no
        // interval to measure yet, so its CPU is not a zero.
        table.now_reading(300, Some(line(300, 'S', 100, 0, 30, 1)));
        let later = later + Duration::from_secs(1);
        scan(&mut sampler, later);
        let root = sampler.sample(100, later).expect("the root's sample");
        assert_eq!(root.descendants.observed, Some(1));
        assert_eq!(root.descendants.rss_bytes, Total::Complete(4096));
        // The new member's own row says the same: it is counted, and it has
        // nothing to measure its CPU against yet.
        assert_eq!(root.descendants.members.len(), 1);
        assert_eq!(root.descendants.members[0].identity.pid, 300);
        assert_eq!(root.descendants.members[0].cpu, None);
        assert_eq!(
            root.descendants.cpu,
            Total::Partial(
                CpuPercent::from_hundredths(0),
                "no CPU sample could be compared with for 1 of them".to_string()
            )
        );
    }

    #[test]
    fn a_child_that_cannot_be_read_leaves_the_total_partial() {
        // The kernel lists the child and Radar is not allowed to read it: the
        // total covers what the scan could read and says so.
        let procs = ScriptedProcs::new().holding(vec![proc(100, 1, 10, 0, 4), (200, None)]);
        let (mut sampler, at) = sampler_over(procs);
        scan(&mut sampler, at);
        let root = sampler.sample(100, at).expect("the root's sample");
        assert_eq!(root.descendants.observed, Some(0));
        assert!(
            root.descendants.members.is_empty(),
            "a process that was never read has no row"
        );
        assert_eq!(
            root.descendants.rss_bytes,
            Total::Partial(0, "1 of the processes could not be read".to_string())
        );
        assert_eq!(
            root.descendants.cpu,
            Total::Partial(
                CpuPercent::from_hundredths(0),
                "1 of the processes could not be read".to_string()
            )
        );
    }

    #[test]
    fn a_cancelled_scan_reports_what_it_read_and_what_it_did_not() {
        let cancel = Arc::new(AtomicBool::new(false));
        let procs = ScriptedProcs::new()
            .holding(vec![
                proc(100, 1, 10, 0, 4),
                proc(200, 100, 20, 1_000, 2),
                proc(300, 100, 30, 1_000, 1),
            ])
            // The scan is cancelled after its first process.
            .cancelling_after(1, Arc::clone(&cancel));
        let table = procs.clone();
        let (mut sampler, at) = sampler_over(procs);
        sampler.scan(at, &cancel);
        assert_eq!(table.reads(), vec![100]);
        let root = sampler.sample(100, at).expect("the root's sample");
        assert_eq!(root.descendants.observed, Some(0));
        assert!(
            root.descendants.members.is_empty(),
            "a scan that confirmed nothing has no rows"
        );
        assert_eq!(
            root.descendants.rss_bytes,
            Total::Partial(0, "the process scan was cancelled".to_string())
        );
    }

    #[test]
    fn a_table_larger_than_the_scan_budget_is_a_lower_bound() {
        let procs = ScriptedProcs::new().holding(vec![
            proc(50, 1, 5, 0, 8),
            proc(100, 1, 10, 0, 4),
            proc(200, 100, 20, 1_000, 2),
        ]);
        let table = procs.clone();
        let mut sampler = Sampler::reading(procs);
        // A budget small enough for the table to exhaust, so the cut is a case a
        // test reaches rather than one it has to imagine.
        sampler.budget = 2;
        let at = Instant::now();
        scan(&mut sampler, at);
        // The machine is asked for one more pid than the scan will read, so a
        // table larger than the budget is recognisable as one rather than
        // enumerated in full, and the confirmation stays inside the budget too.
        assert_eq!(table.enumeration_limits(), vec![3]);
        assert_eq!(table.reads(), vec![50, 100, 50, 100]);

        // The root was read and the child was not: the root's total says that the
        // rest of the table is not in it.
        let root = sampler.sample(100, at).expect("the root's sample");
        assert_eq!(root.descendants.observed, Some(0));
        assert!(root.descendants.members.is_empty());
        assert_eq!(
            root.descendants.rss_bytes,
            Total::Partial(
                0,
                "the process table is larger than the 2-process scan budget".to_string()
            )
        );

        // A root the scan never reached has no totals at all rather than an empty
        // tree.
        let unreached = sampler.sample(200, at).expect("the child's own sample");
        assert_eq!(unreached.descendants.observed, None);
        assert!(unreached.descendants.members.is_empty());
        assert_eq!(
            unreached.descendants.rss_bytes,
            Total::Unknown("this process was not in the process scan".to_string())
        );
        assert_eq!(
            unreached.descendants.cpu,
            Total::Unknown("this process was not in the process scan".to_string())
        );
    }

    #[test]
    fn a_root_replaced_after_the_scan_is_given_no_totals() {
        let procs = ScriptedProcs::new()
            .holding(vec![proc(100, 1, 10, 0, 4), proc(200, 100, 20, 1_000, 2)]);
        let table = procs.clone();
        let (mut sampler, at) = sampler_over(procs);
        scan(&mut sampler, at);

        // The pane exited and its pid was handed on after the scan: the snapshot
        // describes the process that is gone, so the replacement is handed none
        // of its children.
        table.now_reading(100, Some(line(100, 'S', 1, 0, 500, 4)));
        let root = sampler.sample(100, at).expect("the replacement's sample");
        assert_eq!(root.identity.start_ticks, 500);
        assert_eq!(root.descendants.observed, None);
        assert!(
            root.descendants.members.is_empty(),
            "a replacement is handed none of the other process's rows"
        );
        assert_eq!(
            root.descendants.rss_bytes,
            Total::Unknown("the process changed between the scan and its sample".to_string())
        );
        assert_eq!(
            root.descendants.cpu,
            Total::Unknown("the process changed between the scan and its sample".to_string())
        );
    }

    #[test]
    fn a_descendant_reparented_before_its_confirmation_is_left_out_with_its_subtree() {
        // The shell (200) is read beneath the pane (100), and by the time the
        // table's reading is confirmed the shell has been reparented: one read of
        // a live table cannot tell that, which is why the link is confirmed too.
        let procs = ScriptedProcs::new()
            .holding(vec![
                proc(100, 1, 10, 0, 4),
                proc(200, 100, 20, 1_000, 2),
                proc(300, 200, 30, 1_000, 1),
            ])
            .answers(
                200,
                vec![
                    Some(line(200, 'S', 100, 1_000, 20, 2)),
                    Some(line(200, 'S', 1, 1_000, 20, 2)),
                ],
            );
        let table = procs.clone();
        let (mut sampler, at) = sampler_over(procs);
        scan(&mut sampler, at);
        let root = sampler.sample(100, at).expect("the root's sample");

        // The build beneath the shell is left out with it, and the root says why
        // instead of reporting a total it did not confirm.
        assert_eq!(root.descendants.observed, Some(0));
        assert!(
            root.descendants.members.is_empty(),
            "the refuted subtree leaves no rows"
        );
        assert_eq!(
            root.descendants.rss_bytes,
            Total::Partial(
                0,
                "a process beneath this one was reparented while the table was read".to_string()
            )
        );
        // Every process of the refresh was read once and confirmed once, the
        // refuted shell included: the confirmation is shared, not repeated per
        // root.
        assert_eq!(
            table.reads(),
            vec![
                100, 200, 300, // the scan
                100, 200, 300, // its confirmation
                100, 100, // the root's own sample
            ]
        );
    }

    #[test]
    fn a_descendant_replaced_before_its_confirmation_leaves_no_baseline() {
        let procs = ScriptedProcs::new()
            .holding(vec![proc(100, 1, 10, 0, 4), proc(200, 100, 20, 1_000, 2)])
            .answers(
                200,
                vec![
                    Some(line(200, 'S', 100, 1_000, 20, 2)),
                    Some(line(200, 'S', 100, 1_000, 500, 2)),
                ],
            );
        let (mut sampler, at) = sampler_over(procs);
        scan(&mut sampler, at);
        let root = sampler.sample(100, at).expect("the root's sample");

        assert_eq!(root.descendants.observed, Some(0));
        assert!(
            root.descendants.members.is_empty(),
            "the replaced reading leaves no row"
        );
        assert_eq!(
            root.descendants.rss_bytes,
            Total::Partial(
                0,
                "a process beneath this one was replaced while the table was read".to_string()
            )
        );
        // The reading that was not confirmed is not a baseline, and neither is
        // the process that took the pid: the next refresh measures from the last
        // reading that was confirmed.
        assert!(sampler.baselines.keys().all(|identity| identity.pid != 200));
    }

    #[test]
    fn a_descendant_that_vanishes_before_its_confirmation_is_left_out() {
        let procs = ScriptedProcs::new()
            .holding(vec![proc(100, 1, 10, 0, 4), proc(200, 100, 20, 1_000, 2)])
            .answers(200, vec![Some(line(200, 'S', 100, 1_000, 20, 2)), None]);
        let (mut sampler, at) = sampler_over(procs);
        scan(&mut sampler, at);
        let root = sampler.sample(100, at).expect("the root's sample");

        assert_eq!(root.descendants.observed, Some(0));
        assert!(
            root.descendants.members.is_empty(),
            "a process that vanished before its confirmation leaves no row"
        );
        assert_eq!(
            root.descendants.rss_bytes,
            Total::Partial(
                0,
                "a process beneath this one could not be read again".to_string()
            )
        );
        assert_eq!(
            root.descendants.cpu,
            Total::Partial(
                CpuPercent::from_hundredths(0),
                "a process beneath this one could not be read again".to_string()
            )
        );
        assert!(sampler.baselines.keys().all(|identity| identity.pid != 200));
    }

    #[test]
    fn a_refresh_cancelled_before_its_confirmation_counts_nothing_as_confirmed() {
        let cancel = Arc::new(AtomicBool::new(false));
        let procs = ScriptedProcs::new()
            .holding(vec![
                proc(100, 1, 10, 0, 4),
                proc(200, 100, 20, 1_000, 2),
                proc(300, 200, 30, 1_000, 1),
            ])
            // The last read of the scan sets the flag, so it lands on the
            // confirmation pass the scan would otherwise run.
            .cancelling_after(3, Arc::clone(&cancel));
        let table = procs.clone();
        let (mut sampler, at) = sampler_over(procs);
        sampler.scan(at, &cancel);
        assert_eq!(table.reads(), vec![100, 200, 300]);

        // Nothing was confirmed, so nothing is counted and nothing is kept: the
        // root says the scan was cancelled instead of reporting the shell as a
        // complete total.
        let root = sampler.sample(100, at).expect("the root's sample");
        assert_eq!(root.descendants.observed, Some(0));
        assert!(
            root.descendants.members.is_empty(),
            "a refresh that confirmed nothing has no rows"
        );
        assert_eq!(
            root.descendants.rss_bytes,
            Total::Partial(0, "the process scan was cancelled".to_string())
        );
        assert!(sampler.baselines.keys().all(|identity| identity.pid != 200));
    }
}
