# Process evidence on the Processes page

How the dashboard names a pane's foreground process, compares it with the program
installed now, and measures it. This is reference for reading the Processes page,
not something to configure.

## Process identity

The Processes page names the process itself, for an agent row and for an
ordinary pane row whose foreground evidence is current:

- `observed` is the command name the runtime reports for the process holding the
  foreground. It is an observation of a running process, not an invocation: for a
  bolt pane it is the exec-replaced payload's own `pi`, never the `pi-bolt` or
  `pi-bolt-child` launcher the pane was started from and never the `pi` alias.
- `package` names the running installation once: the package's name and version,
  and the first characters of the store hash when the comparison has two builds
  of that one version to tell apart. A path outside the Nix store is named whole.
- `exe` is the file the kernel is running inside that package, relative to the
  package root (`lib/pi-bolt/pi`) rather than the whole store path that names
  it twice. A file outside that root is drawn where it is.
- `binary` is how the running executable compares with the program `PATH`
  resolves now, and it is drawn only when it has something to say: a current
  comparison has no verdict at all, because the absence of a stale mark is the
  verdict. Stale is marked and names the installed target as compactly as the
  package line names the running one — `<mark> (stale) installed
  pi-bolt-0.7.1 3f9b1c4` — in the configured stale ink; a deliberate other build
  says `other build` and leaves both roots to the block; and a comparison that
  could not be made gives its reason in a word or two — no counterpart, payload
  unknown, unreadable, not the named program.
- `▸ details` is the page's block, and holds the long halves of the same facts:
  the store the two roots sit in, stated once, with each root's own part beside
  it (`store /nix/store/`, then `running aaa-pi-bolt-0.7.1` and
  `installed bbb-pi-bolt-0.7.1`), the executable's place under the running root,
  the birth identity the reading belongs to (PID, boot id and start ticks), the
  full sentence behind a comparison that could not be made, what the kernel
  state this row is showing means, and what a descendant sum is. Every fact is
  a labelled row in one column, and a fact too long for the panel continues
  under that column rather than under its label. Space and Enter, or a click on
  the marker, open it, and the body scrolls with the page like any other row.

The process's arguments are never drawn on this page: a harness takes its prompt
and system prompt as arguments, and nothing the runtime reports tells a prompt
from a flag (a pane's fleet row still leads with the command it is running). A
current identity stays readable rather than being omitted because there is
nothing to mark, and a retained row, a stale or unavailable source, a shell and
an inconclusive foreground draw none of these fields — and offer no block,
because a marker that opens onto nothing is worse than no marker.

## How the binary comparison resolves

The comparison is by **package family** for the Pi-Bolt variants and by matching
file name for every other program. A bolt pane's process holds
`<package>/lib/pi-bolt/pi` while `PATH` resolves `<package>/bin/pi-bolt`, so the
two files never share a name and only the package identifies either. For those
variants Radar reads the family from the running executable's own Nix store
package — the exact `pi-bolt` or `pi-bolt-child` name in its derivation root,
never a prefix — and compares the running package root with the payload package
root of the counterpart `PATH` resolves for that exact family. Two store roots
that differ are `stale` even when both packages carry the same version: a rebuild
of the same version is a different build, and no version string, build stamp or
manifest is consulted to excuse it. A program outside the handled families keeps
the earlier by-name comparison, and its non-store paths are unchanged.

The installed counterpart is read in one of two shapes:

- **Same-root entrypoint (primary).** When `PATH` resolves a family name to an
  entrypoint inside a versioned package of that same family — the shape a Nix
  package has, where `bin/pi-bolt` and `lib/pi-bolt/pi` share one root — that
  package root is the payload root, and nothing is read from the entrypoint.
  An unversioned launcher root is not a payload root merely because the
  entrypoint sits inside it.
- **Cross-root launcher (transitional).** While the profile still installs a
  separate launcher script, Radar reads its declared target: a short `#!` script
  whose only mention of `exec` is one strictly literal
  `exec /nix/store/<root>/bin/<program> [args…]` line the script ends with, whose
  target is a versioned package of the exact same family. Any other shape — no
  `exec`, several, a computed, quoted or conditional target, a loop or variable
  indirection, a non-store target, another family's package, an unrelated
  program, an unversioned root, a target inside a package but not under `bin`, a
  `.` or `..` component, a non-script file or a further hop — is `unknown`
  rather than guessed at. Nothing is executed, no compiled content is read, and
  no process's environment is read. The branch is compatibility for the
  packaging that is shipping, depends on no fixed script text, and is removed
  once each derivation provides its own same-root entrypoints.

Radar reads its own `PATH`, never a pane's environment, so the profile in effect
is the one that decides. The `pi` the runtime reports for a bolt payload is an
observed name and selects no program: it is not compared through the `pi` alias
as though the alias were the launcher. On the current multi-user profile the
lookup is `/etc/profiles/per-user/<user>/…`, not `~/.nix-profile`, and no fixed
profile path or version is assumed.

Resources describe processes, not work. A process's CPU is the interval between
two readings of that same process incarnation — its first reading has none — and
its resident set counts pages shared with another process in each process that
maps them. Beneath a sampled process Radar sums the descendants the kernel
reports for it, from one snapshot of the process table taken per refresh and
confirmed before any row is measured: a descendant whose birth identity or
parent link changed in that window, and anything reached through it, is left
out. That sum is an observed process sum, not a cgroup, workload or
agent-ownership total, so a build a pane launched is never the pane's own CPU,
and a total that could not cover every process it saw is shown as a lower bound
with the reason instead of as a zero. A process Radar cannot read is not in the
sum it would have been part of. A published task's PID is never sampled: without
a birth identity captured at spawn, a PID that has been reused cannot authorise
metrics. That captured identity is a publisher contract Radar does not have yet;
until it does, a task's PID is named and the measurements beside it stay absent.

On the page a process's own figures and its descendants' are drawn apart and
never added together, and the shared-page warning sits beside the sum. Every
value Radar could not measure says so with its reason rather than as a zero: a
first reading has no interval to report, and a figure a row can no longer
observe is drawn as withheld, not as the last one anyone saw.

Those confirmed descendants are drawn individually too, one row each, in a table
held behind the page's own `▸ process table` marker and closed until a reader
opens it. The root is the table's first row: every process beneath it is indented
under the one it was read beneath, and the siblings of one parent are ordered by
PID, so an unchanged table draws the same tree on every refresh. Each row is one
line of the panel — a name, a PID, an interval CPU and a resident set, aligned
down the columns — whatever the panel's width: as it narrows the resident set is
dropped first and then the CPU, and a name its column cannot hold is shortened,
while the PID is never the column that goes. A `—` is a value this machine did
not measure and never the zero a measured idle interval is, and a table that drew
one says so beneath its rows. The name is the kernel's own short name for the
process, sanitized like every other external text: it is not the arguments the
process was started with, and it is never a claim about what started it. A table
is offered only where the scan that sampled the root enumerated what is beneath
it, so a scan that could not be read leaves the sums saying so rather than
drawing an empty tree, and a stale or retained row draws none of it.

