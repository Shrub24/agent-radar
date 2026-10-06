#![cfg(unix)]
//! Binary staleness: the executable comparison against an injected PATH.
//!
//! The classification of store roots is exercised beside the reader in
//! `src/procfs.rs`, where fabricated paths need no files. Everything here drives
//! the resolver with directories this test creates, so nothing depends on the
//! machine's own Nix store, `PATH` or installed programs.

use std::fs;
use std::path::{Path, PathBuf};

use agent_radar::BinaryFreshness;
use agent_radar::procfs::{BinaryIndex, identity};

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

#[test]
fn a_path_match_is_resolved_and_compared() {
    let directory = workspace("resolved");
    let inner = installed(&directory);
    let mut index = BinaryIndex::searching(vec![directory.join("bin")]);

    let identity = identity(&inner, false, "pi", &mut index);
    assert_eq!(identity.freshness, BinaryFreshness::Current);
    assert_eq!(identity.running.as_deref(), inner.to_str());
    assert_eq!(identity.installed.as_deref(), inner.to_str());
    fs::remove_dir_all(&directory).expect("clean up");
}

#[test]
fn a_replaced_executable_is_stale() {
    let directory = workspace("deleted");
    let inner = installed(&directory);
    let mut index = BinaryIndex::searching(vec![directory.join("bin")]);

    // The kernel marks a link whose file was unlinked or replaced; the file
    // need not be gone for the mark to mean the process lost its binary.
    let identity = identity(&inner, true, "pi", &mut index);
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

    let identity = identity(&elsewhere, false, "pi", &mut index);
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

    let identity = identity(&inner, false, "pi", &mut index);
    assert_eq!(identity.freshness, BinaryFreshness::Unknown);
    assert_eq!(identity.running, None);
    assert_eq!(identity.installed, None);
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
    let identity = identity(&python, false, "pi", &mut index);
    assert_eq!(identity.freshness, BinaryFreshness::Unknown);
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

    let first = identity(&program, false, "pi", &mut index);
    assert_eq!(first.freshness, BinaryFreshness::Current);
    // The answer is cached for the refresh: removing the program does not make
    // a second lookup disagree, because there is no second lookup.
    fs::remove_file(&program).expect("remove program");
    assert_eq!(identity(&program, false, "pi", &mut index), first);
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

    let identity = identity(&inner, false, "pi", &mut index);
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

    let identity = identity(&inner, false, "pi", &mut index);
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

    let identity = identity(&inner, false, "pi", &mut index);
    assert_eq!(identity.freshness, BinaryFreshness::Unknown);
    assert_eq!(identity.installed, None);
    fs::remove_dir_all(&directory).expect("clean up");
}

#[test]
fn a_program_with_a_path_separator_is_not_a_path_name() {
    let directory = workspace("separator");
    let inner = installed(&directory);
    let mut index = BinaryIndex::searching(vec![directory.join("bin")]);

    // `./pi` is a path, not a command name: it is not searched for on PATH.
    let identity = identity(&inner, false, "./pi", &mut index);
    assert_eq!(identity.freshness, BinaryFreshness::Unknown);
    assert_eq!(identity.installed, None);
    fs::remove_dir_all(&directory).expect("clean up");
}

#[test]
fn a_gone_process_has_unknown_facts() {
    // A pid that cannot exist: nothing is older than the process table. The
    // read is Radar's own `/proc`, not an installed store.
    let mut index = BinaryIndex::searching(Vec::new());
    let facts = agent_radar::procfs::facts(i32::MAX, Some("pi"), &mut index);
    assert!(facts.running_for.is_none());
    assert_eq!(facts.binary.freshness, BinaryFreshness::Unknown);
}
