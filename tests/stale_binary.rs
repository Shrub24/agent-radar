#![cfg(unix)]
//! Binary staleness: the executable comparison against an injected PATH.
//!
//! The classification of store roots is exercised beside the reader in
//! `src/procfs.rs`, where fabricated paths need no files. Everything here drives
//! the resolver with directories this test creates, so nothing depends on the
//! machine's own Nix store, `PATH` or installed programs.
//!
//! The Pi-Bolt variants are the case this comparison exists for: a pane's
//! process holds `<package>/lib/pi-bolt/pi` while `PATH` resolves
//! `<package>/bin/pi-bolt`, so the two files never share a name and only the
//! package family identifies either. A store root is a string to this
//! comparison, so a running path may be a fabricated store path no file backs;
//! a launcher's own file is real, because it is read.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use agent_radar::procfs::{BinaryIndex, Sampler, counterpart_root, identity};
use agent_radar::{BinaryFreshness, BinaryUnknown};

/// A directory of this test's own, removed when the case ends.
fn workspace(name: &str) -> PathBuf {
    let directory =
        std::env::temp_dir().join(format!("radar-stale-binary-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).expect("test directory");
    directory
}

/// A program inside a package-shaped tree: `<dir>/libexec/pi/pi`, reachable as
/// `<dir>/bin/pi` through a symlink, the way a real installation is laid out.
fn installed(directory: &Path) -> PathBuf {
    let inner = directory.join("libexec/pi/pi");
    fs::create_dir_all(inner.parent().expect("parent")).expect("inner directory");
    fs::write(&inner, b"binary").expect("inner file");
    executable(&inner);
    let bin = directory.join("bin");
    fs::create_dir_all(&bin).expect("bin directory");
    std::os::unix::fs::symlink(&inner, bin.join("pi")).expect("launcher symlink");
    inner
}

/// Gives a fixture the execute bit, as a real program on `PATH` has.
fn executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .expect("make the fixture executable");
}

/// A launcher script a search directory holds: its file name is the command a
/// `PATH` lookup finds, its contents the one target it runs.
///
/// Nothing here runs a launcher, and the target it names need not exist: the
/// resolver reads where a script says it goes, and stops at what it can read.
fn launcher(directory: &Path, name: &str, body: &str) -> PathBuf {
    fs::create_dir_all(directory).expect("search directory");
    let path = directory.join(name);
    fs::write(&path, body).expect("launcher script");
    executable(&path);
    path
}

/// A launcher named after a command, in a search path of its own.
fn searching(directory: &Path, name: &str, body: &str) -> BinaryIndex {
    launcher(&directory.join("bin"), name, body);
    BinaryIndex::searching(vec![directory.join("bin")])
}

/// The installed package a launcher runs, as the store names it; and a build of
/// the same version that a running process still holds.
const LEAD: &str = "/nix/store/r8dayii6jzs99b5wwbvmg5q14jwa5b1l-pi-bolt-0.7.1";
const LEAD_RUNNING: &str = "/nix/store/hxdd4znbva7jbas38cr1piavy5ig67j8-pi-bolt-0.7.1";
const CHILD: &str = "/nix/store/xcxjlh3hfc6r7llg42l2s7wvqcr523p4-pi-bolt-child-0.7.1";
const CHILD_RUNNING: &str = "/nix/store/ijm3q5j0w25i5iw12sl8ql7kxfwmzns2-pi-bolt-child-0.7.1";

/// The payload every bolt package runs, whichever variant built it.
fn payload(root: &str) -> PathBuf {
    PathBuf::from(format!("{root}/lib/pi-bolt/pi"))
}

/// The shape the profile's launchers have: setup, then one literal `exec`.
fn shim(target: &str) -> String {
    format!(
        "#!/nix/store/10dxp0qxqxxsyiljrh2kp0xqhz6arhcx-bash-5.3p15/bin/bash\n\
         flags=(--no-extensions)\n\
         if [ -f \"$HOME/extra.ts\" ]; then flags+=(-e \"$HOME/extra.ts\"); fi\n\
         export PI_HERDSMAN_CHILD_COMMAND=pi-bolt-child\n\
         exec {target}/bin/pi-bolt \"${{flags[@]}}\" \"$@\"\n"
    )
}

#[test]
fn a_path_match_is_resolved_and_compared() {
    let directory = workspace("resolved");
    let inner = installed(&directory);
    let mut index = BinaryIndex::searching(vec![directory.join("bin")]);

    let identity = identity(&inner, false, Some("pi"), &mut index);
    assert_eq!(identity.freshness, BinaryFreshness::Current);
    assert_eq!(identity.running.as_deref(), inner.to_str());
    assert_eq!(identity.installed.as_deref(), inner.to_str());
    assert_eq!(identity.executable.as_deref(), inner.to_str());
    assert_eq!(identity.unknown, None);
    fs::remove_dir_all(&directory).expect("clean up");
}

#[test]
fn a_replaced_executable_is_stale() {
    let directory = workspace("deleted");
    let inner = installed(&directory);
    let mut index = BinaryIndex::searching(vec![directory.join("bin")]);

    // The kernel marks a link whose file was unlinked or replaced; the file
    // need not be gone for the mark to mean the process lost its binary.
    let identity = identity(&inner, true, Some("pi"), &mut index);
    assert_eq!(identity.freshness, BinaryFreshness::Stale);
    fs::remove_dir_all(&directory).expect("clean up");
}

#[test]
fn a_deliberate_other_build_is_not_stale_and_names_both_identities() {
    let directory = workspace("other-build");
    let inner = installed(&directory);
    let elsewhere = directory.join("checkout/target/debug/pi");
    fs::create_dir_all(elsewhere.parent().expect("parent")).expect("checkout directory");
    fs::write(&elsewhere, b"dev build").expect("dev binary");
    executable(&elsewhere);
    let mut index = BinaryIndex::searching(vec![directory.join("bin")]);

    let identity = identity(&elsewhere, false, Some("pi"), &mut index);
    // Not the installed program, but nothing was replaced.
    assert_eq!(identity.freshness, BinaryFreshness::Current);
    assert_eq!(identity.running.as_deref(), elsewhere.to_str());
    assert_eq!(identity.installed.as_deref(), inner.to_str());
    fs::remove_dir_all(&directory).expect("clean up");
}

#[test]
fn no_path_match_is_unknown() {
    let directory = workspace("no-match");
    let inner = installed(&directory);
    // A search path that does not contain the program.
    let mut index = BinaryIndex::searching(vec![directory.join("empty")]);

    let identity = identity(&inner, false, Some("pi"), &mut index);
    assert_eq!(identity.freshness, BinaryFreshness::Unknown);
    assert_eq!(identity.unknown, Some(BinaryUnknown::NoCounterpart));
    assert_eq!(identity.running, None);
    assert_eq!(identity.installed, None);
    // What is running is still nameable without being attributed.
    assert_eq!(identity.executable.as_deref(), inner.to_str());
    fs::remove_dir_all(&directory).expect("clean up");
}

#[test]
fn an_interpreter_is_not_attributed_to_the_agent() {
    let directory = workspace("interpreter");
    installed(&directory);
    let python = directory.join("usr/bin/python3");
    fs::create_dir_all(python.parent().expect("parent")).expect("bin directory");
    fs::write(&python, b"interpreter").expect("interpreter file");
    executable(&python);
    let mut index = BinaryIndex::searching(vec![directory.join("bin")]);

    // The runtime reports the agent as `pi` while the foreground process is the
    // interpreter running its script: nothing is claimed either way.
    let identity = identity(&python, false, Some("pi"), &mut index);
    assert_eq!(identity.freshness, BinaryFreshness::Unknown);
    assert_eq!(identity.unknown, Some(BinaryUnknown::NotCompared));
    assert_eq!(identity.running, None);
    assert_eq!(identity.installed, None);
    fs::remove_dir_all(&directory).expect("clean up");
}

#[test]
fn each_program_is_resolved_once_per_refresh() {
    let directory = workspace("cached");
    let program = directory.join("bin/pi");
    fs::create_dir_all(program.parent().expect("parent")).expect("bin directory");
    fs::write(&program, b"binary").expect("program file");
    executable(&program);
    let mut index = BinaryIndex::searching(vec![directory.join("bin")]);

    let first = identity(&program, false, Some("pi"), &mut index);
    assert_eq!(first.freshness, BinaryFreshness::Current);
    // The answer is cached for the refresh: removing the program does not make
    // a second lookup disagree, because there is no second lookup.
    fs::remove_file(&program).expect("remove program");
    assert_eq!(identity(&program, false, Some("pi"), &mut index), first);
    fs::remove_dir_all(&directory).expect("clean up");
}

#[test]
fn a_non_executable_file_cannot_mask_a_later_program() {
    let directory = workspace("non-executable");
    let inner = installed(&directory);
    // An earlier directory holds a same-named file with no execute bit.
    let earlier = directory.join("earlier");
    fs::create_dir_all(&earlier).expect("earlier directory");
    fs::write(earlier.join("pi"), b"not a program").expect("non-executable file");
    let mut index = BinaryIndex::searching(vec![earlier, directory.join("bin")]);

    let identity = identity(&inner, false, Some("pi"), &mut index);
    assert_eq!(identity.freshness, BinaryFreshness::Current);
    assert_eq!(identity.installed.as_deref(), inner.to_str());
    fs::remove_dir_all(&directory).expect("clean up");
}

#[test]
fn a_directory_cannot_mask_a_later_program() {
    let directory = workspace("directory");
    let inner = installed(&directory);
    // An earlier directory holds a directory named like the program.
    let earlier = directory.join("earlier");
    fs::create_dir_all(earlier.join("pi")).expect("directory named pi");
    let mut index = BinaryIndex::searching(vec![earlier, directory.join("bin")]);

    let identity = identity(&inner, false, Some("pi"), &mut index);
    assert_eq!(identity.freshness, BinaryFreshness::Current);
    assert_eq!(identity.installed.as_deref(), inner.to_str());
    fs::remove_dir_all(&directory).expect("clean up");
}

#[test]
fn a_path_with_no_executable_match_is_unknown() {
    let directory = workspace("no-executable");
    let inner = installed(&directory);
    // The only match on the search path is not executable: it is not a command.
    let only = directory.join("only");
    fs::create_dir_all(&only).expect("only directory");
    fs::write(only.join("pi"), b"not executable").expect("non-executable file");
    let mut index = BinaryIndex::searching(vec![only]);

    let identity = identity(&inner, false, Some("pi"), &mut index);
    assert_eq!(identity.freshness, BinaryFreshness::Unknown);
    assert_eq!(identity.unknown, Some(BinaryUnknown::NoCounterpart));
    assert_eq!(identity.installed, None);
    fs::remove_dir_all(&directory).expect("clean up");
}

#[test]
fn a_program_with_a_path_separator_is_not_a_path_name() {
    let directory = workspace("separator");
    let inner = installed(&directory);
    let mut index = BinaryIndex::searching(vec![directory.join("bin")]);

    // `./pi` is a path, not a command name: it is not searched for on PATH.
    let identity = identity(&inner, false, Some("./pi"), &mut index);
    assert_eq!(identity.freshness, BinaryFreshness::Unknown);
    assert_eq!(identity.unknown, Some(BinaryUnknown::NoCounterpart));
    assert_eq!(identity.installed, None);
    fs::remove_dir_all(&directory).expect("clean up");
}

#[test]
fn a_lead_running_another_build_of_the_same_version_is_stale() {
    let directory = workspace("bolt-lead");
    let mut index = searching(&directory, "pi-bolt", &shim(LEAD));

    // The profile's launcher is a script in a package of its own, and the
    // payload it names is the build `PATH` points at now.
    let running = payload(LEAD_RUNNING);
    let verdict = identity(&running, false, Some("pi"), &mut index);
    assert_eq!(verdict.freshness, BinaryFreshness::Stale);
    assert_eq!(verdict.running.as_deref(), Some(LEAD_RUNNING));
    assert_eq!(verdict.installed.as_deref(), Some(LEAD));
    assert_eq!(verdict.unknown, None);
    assert_eq!(verdict.executable.as_deref(), running.to_str());

    // The same launcher, with the process already running the build it names.
    let mut index = searching(&directory, "pi-bolt", &shim(LEAD_RUNNING));
    let current = identity(&running, false, Some("pi"), &mut index);
    assert_eq!(current.freshness, BinaryFreshness::Current);
    assert_eq!(current.installed.as_deref(), Some(LEAD_RUNNING));
    fs::remove_dir_all(&directory).expect("clean up");
}

#[test]
fn a_child_is_resolved_by_its_own_family() {
    let directory = workspace("bolt-child");
    let mut index = searching(&directory, "pi-bolt-child", &shim(CHILD));

    let verdict = identity(&payload(CHILD_RUNNING), false, Some("pi"), &mut index);
    assert_eq!(verdict.freshness, BinaryFreshness::Stale);
    assert_eq!(verdict.running.as_deref(), Some(CHILD_RUNNING));
    assert_eq!(verdict.installed.as_deref(), Some(CHILD));

    let mut index = searching(&directory, "pi-bolt-child", &shim(CHILD_RUNNING));
    let current = identity(&payload(CHILD_RUNNING), false, Some("pi"), &mut index);
    assert_eq!(current.freshness, BinaryFreshness::Current);
    fs::remove_dir_all(&directory).expect("clean up");
}

#[test]
fn a_child_is_never_compared_with_the_lead_or_the_alias() {
    let directory = workspace("bolt-cross-family");
    // Only the lead's launcher is installed, and the alias `pi` points at it:
    // neither stands for the child, and nothing is claimed about its build.
    let mut index = searching(&directory, "pi-bolt", &shim(LEAD));
    launcher(&directory.join("bin"), "pi", &shim(LEAD));

    let verdict = identity(&payload(CHILD_RUNNING), false, Some("pi"), &mut index);
    assert_eq!(verdict.freshness, BinaryFreshness::Unknown);
    assert_eq!(verdict.unknown, Some(BinaryUnknown::NoCounterpart));
    assert_eq!(verdict.running.as_deref(), Some(CHILD_RUNNING));
    assert_eq!(verdict.installed, None);
    fs::remove_dir_all(&directory).expect("clean up");
}

#[test]
fn a_runtime_name_never_selects_a_bolt_counterpart() {
    let directory = workspace("bolt-alias-only");
    // `pi` is the name of the payload both variants replace their shell with,
    // so a search path that has only `pi` resolves no bolt counterpart.
    let mut index = searching(&directory, "pi", &shim(LEAD));

    let verdict = identity(&payload(LEAD_RUNNING), false, Some("pi"), &mut index);
    assert_eq!(verdict.freshness, BinaryFreshness::Unknown);
    assert_eq!(verdict.unknown, Some(BinaryUnknown::NoCounterpart));
    // Neither is a program with no runtime name compared by one.
    let nameless = identity(&payload(LEAD_RUNNING), false, None, &mut index);
    assert_eq!(nameless.freshness, BinaryFreshness::Unknown);
    assert_eq!(nameless.unknown, Some(BinaryUnknown::NoCounterpart));
    fs::remove_dir_all(&directory).expect("clean up");
}

#[test]
fn a_deleted_executable_is_stale_without_a_counterpart() {
    let directory = workspace("bolt-deleted");
    // Nothing installed to compare with, and a launcher whose target cannot be
    // read: the kernel's mark is a fact about the running file either way.
    let mut index = BinaryIndex::searching(vec![directory.join("empty")]);
    let verdict = identity(&payload(LEAD_RUNNING), true, Some("pi"), &mut index);
    assert_eq!(verdict.freshness, BinaryFreshness::Stale);
    assert_eq!(verdict.unknown, None);
    assert_eq!(verdict.running.as_deref(), Some(LEAD_RUNNING));
    assert_eq!(verdict.installed, None);

    let mut index = searching(&directory, "pi-bolt", "#!/bin/sh\nprintf 'no target'\n");
    let unreadable = identity(&payload(LEAD_RUNNING), true, Some("pi"), &mut index);
    assert_eq!(unreadable.freshness, BinaryFreshness::Stale);
    assert_eq!(unreadable.unknown, None);
    fs::remove_dir_all(&directory).expect("clean up");
}

#[test]
fn a_versioned_packages_own_entrypoint_names_its_own_payload_root() {
    // The shape a derivation provides: `bin/pi-bolt` beside `lib/pi-bolt/pi` in
    // one root. The root is the answer, and the file is never read — which is
    // why a store path that no fixture can create is still testable.
    assert_eq!(
        counterpart_root(Path::new(&format!("{LEAD}/bin/pi-bolt")), "pi-bolt"),
        Ok(PathBuf::from(LEAD))
    );
    assert_eq!(
        counterpart_root(Path::new(&format!("{CHILD}/bin/pi-bolt")), "pi-bolt-child"),
        Ok(PathBuf::from(CHILD))
    );
    // A root that is not a versioned package of the family is not a payload
    // root merely because the entrypoint sits inside it: the unversioned
    // launcher package in the profile holds no payload.
    assert_eq!(
        counterpart_root(Path::new("/nix/store/zzz-pi-bolt/bin/pi-bolt"), "pi-bolt"),
        Err(BinaryUnknown::UnsupportedLauncher)
    );
    // And a versioned package of another family is not this family's payload.
    assert_eq!(
        counterpart_root(Path::new(&format!("{CHILD}/bin/pi-bolt")), "pi-bolt"),
        Err(BinaryUnknown::UnsupportedLauncher)
    );
}

#[test]
fn only_a_launcher_shape_that_says_where_it_goes_is_read() {
    let directory = workspace("launcher-shapes");
    let bin = directory.join("bin");
    fs::create_dir_all(&bin).expect("bin directory");
    let target = format!("{LEAD}/bin/pi-bolt");
    // The accepted shape: setup, then one literal target the script ends with.
    let accepted = launcher(&bin, "pi-bolt", &shim(LEAD));
    assert_eq!(
        counterpart_root(&accepted, "pi-bolt"),
        Ok(PathBuf::from(LEAD))
    );
    // A launcher naming a different build of the same family is accepted: the
    // differing root is the answer, and the stale verdict is by design.
    let other_build = launcher(&bin, "pi-bolt", &shim("/nix/store/bbb-pi-bolt-0.7.2"));
    assert_eq!(
        counterpart_root(&other_build, "pi-bolt"),
        Ok(PathBuf::from("/nix/store/bbb-pi-bolt-0.7.2"))
    );
    let cases: Vec<(&str, String)> = vec![
        (
            "no exec at all",
            "#!/bin/sh\nflags=(--no-extensions)\nprintf 'done'\n".to_string(),
        ),
        (
            "two execs",
            "#!/bin/sh\nexec /nix/store/aaa-pi-bolt-0.7.1/bin/pi-bolt\nexec /nix/store/bbb-pi-bolt-0.7.2/bin/pi-bolt\n".to_string(),
        ),
        (
            "an indented exec",
            "#!/bin/sh\n  exec /nix/store/aaa-pi-bolt-0.7.1/bin/pi-bolt\n".to_string(),
        ),
        (
            "a compound exec",
            "#!/bin/sh\n[ -x /bin/true ] && exec /nix/store/aaa-pi-bolt-0.7.1/bin/pi-bolt\n".to_string(),
        ),
        (
            "an inline conditional exec",
            "#!/bin/sh\nif [ -x /bin/true ]; then exec /nix/store/aaa-pi-bolt-0.7.1/bin/pi-bolt; fi\n".to_string(),
        ),
        (
            "a quoted target",
            "#!/bin/sh\nexec \"/nix/store/aaa-pi-bolt-0.7.1/bin/pi-bolt\"\n".to_string(),
        ),
        ("a variable target", "#!/bin/sh\nexec \"$TARGET\"\n".to_string()),
        (
            "a non-store target",
            "#!/bin/sh\nexec /opt/pi-bolt/bin/pi-bolt\n".to_string(),
        ),
        (
            "a target that is not the last thing the script does",
            "#!/bin/sh\nexec /nix/store/aaa-pi-bolt-0.7.1/bin/pi-bolt\necho later\n".to_string(),
        ),
        (
            "another family's package",
            format!("#!/bin/sh\nexec {CHILD}/bin/pi-bolt\n"),
        ),
        (
            "a versioned package of an unrelated program",
            "#!/bin/sh\nexec /nix/store/bbb-some-tool-1.2.3/bin/pi-bolt\n".to_string(),
        ),
        (
            "an unversioned launcher root as its own target",
            "#!/bin/sh\nexec /nix/store/zzz-pi-bolt/bin/pi-bolt\n".to_string(),
        ),
        (
            "a target inside a package but not under bin",
            format!("#!/bin/sh\nexec {LEAD}/lib/pi-bolt/pi\n"),
        ),
        (
            "a current-directory component",
            format!("#!/bin/sh\nexec {LEAD}/./bin/pi-bolt\n"),
        ),
        (
            "a parent-directory component",
            format!("#!/bin/sh\nexec {LEAD}/bin/../bin/pi-bolt\n"),
        ),
        (
            "a bare store file",
            "#!/bin/sh\nexec /nix/store/bbb-some-tool-1.2.3\n".to_string(),
        ),
        ("no shebang", format!("exec {target}\n")),
        (
            "a file that is not a script",
            "an ELF header, not a script".to_string(),
        ),
    ];
    for (name, body) in &cases {
        let path = launcher(&bin, "pi-bolt", body);
        let resolved = counterpart_root(&path, "pi-bolt");
        assert_eq!(
            resolved,
            Err(BinaryUnknown::UnsupportedLauncher),
            "the {name} case was not refused: {resolved:?}"
        );
    }

    // A file larger than any launcher is not read to find out what it is.
    let oversized = launcher(
        &bin,
        "pi-bolt",
        &format!("#!/bin/sh\n{}\nexec {target}\n", "# filler\n".repeat(9_000)),
    );
    assert_eq!(
        counterpart_root(&oversized, "pi-bolt"),
        Err(BinaryUnknown::UnsupportedLauncher)
    );
    fs::remove_dir_all(&directory).expect("clean up");
}

#[test]
fn an_unversioned_launcher_package_is_resolved_through_its_target() {
    let directory = workspace("unversioned-root");
    // The profile's own launcher package has no version in its root: only the
    // target it names says where the payload is.
    let path = launcher(&directory.join("bin"), "pi-bolt", &shim(LEAD));
    assert_eq!(counterpart_root(&path, "pi-bolt"), Ok(PathBuf::from(LEAD)));
    fs::remove_dir_all(&directory).expect("clean up");
}

#[test]
fn a_gone_process_has_unknown_facts() {
    // A pid that cannot exist: nothing is older than the process table. The
    // read is Radar's own `/proc`, not an installed store.
    let mut index = BinaryIndex::searching(Vec::new());
    let mut sampler = Sampler::new();
    let facts = agent_radar::procfs::facts(
        i32::MAX,
        Some("pi"),
        &mut index,
        &mut sampler,
        Instant::now(),
    );
    assert!(facts.running_for.is_none());
    assert_eq!(facts.binary.freshness, BinaryFreshness::Unknown);
    assert_eq!(facts.binary.unknown, Some(BinaryUnknown::Unreadable));
    // A process that cannot be read is not one to sample either.
    assert_eq!(facts.resources, None);
}
