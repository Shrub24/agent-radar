//! The directories a new workspace can start in, and the picker that chooses one.
//!
//! Candidates come from a configurable command that prints one directory per
//! line (by default `zoxide query -l`), filtered to git projects unless the
//! operator turns that off. A configured base directory is offered only when
//! the source yields nothing. The command runs in
//! the dashboard, never in the daemon, and its output is only ever a list of
//! paths to choose from: nothing it prints is run.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use crate::config::ProjectFilter;

/// How long a source command may take before the picker opens without it.
const SOURCE_TIMEOUT: Duration = Duration::from_secs(3);

/// The program and arguments run when the configuration names none.
pub const DEFAULT_SOURCE: [&str; 3] = ["zoxide", "query", "-l"];

/// What the picker offers, and why the source contributed nothing when it did not.
#[derive(Debug, PartialEq, Eq)]
pub struct Candidates {
    pub directories: Vec<PathBuf>,
    /// Set when the source could not be used, for one line in the footer.
    pub note: Option<String>,
}

/// Source-ordered absolute directories, with `base` as the empty-source fallback.
pub fn candidates(source: Option<&[String]>, filter: ProjectFilter, base: &Path) -> Candidates {
    let default: Vec<String> = DEFAULT_SOURCE.iter().map(|word| word.to_string()).collect();
    let source = source.unwrap_or(&default);
    let (lines, mut note) = match run_source(source) {
        Ok(lines) => (lines, None),
        Err(message) => (Vec::new(), Some(message)),
    };
    let mut directories = select(&lines, filter);
    if directories.is_empty() {
        if note.is_none() {
            note = Some("project source yielded no accepted directories".into());
        }
        if base.is_absolute() && base.is_dir() {
            directories.push(base.to_path_buf());
        }
    }
    let note = if directories.is_empty() {
        Some(format!(
            "{}; project base {} is not an absolute directory",
            note.as_deref()
                .unwrap_or("project source yielded no directories"),
            base.display()
        ))
    } else {
        note
    };
    Candidates { directories, note }
}

/// Keeps source order and removes duplicates; the fallback bypasses the git filter.
fn select(lines: &[String], filter: ProjectFilter) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    for line in lines {
        let path = PathBuf::from(line.trim());
        if !path.is_absolute() || !path.is_dir() || directories.contains(&path) {
            continue;
        }
        if filter == ProjectFilter::Git && !path.join(".git").exists() {
            continue;
        }
        directories.push(path);
    }
    directories
}

/// Runs the source and returns its stdout lines.
fn run_source(source: &[String]) -> Result<Vec<String>, String> {
    let Some((program, args)) = source.split_first() else {
        return Err("project source names no program".to_string());
    };
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("project source `{program}` did not start: {error}"))?;
    let mut stdout = child.stdout.take().expect("stdout was piped");
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let result = stdout.read_to_string(&mut text).map(|_| text);
        let _ = sender.send(result);
    });
    match receiver.recv_timeout(SOURCE_TIMEOUT) {
        Ok(Ok(text)) => {
            let status = child
                .wait()
                .map_err(|error| format!("project source `{program}`: {error}"))?;
            if status.success() {
                Ok(text.lines().map(str::to_string).collect())
            } else {
                Err(format!("project source `{program}` exited with {status}"))
            }
        }
        Ok(Err(error)) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(format!("project source `{program}`: {error}"))
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(format!("project source `{program}` took too long"))
        }
    }
}

/// The name a project workspace shows: the folder of the git root that contains
/// `directory`, or the directory's own folder when it is in no repository.
pub fn project_name(directory: &Path) -> String {
    let root = directory
        .ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
        .unwrap_or(directory);
    root.file_name()
        .or_else(|| directory.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "workspace".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("radar-project-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn source_order_is_cwd_independent_and_the_filter_keeps_only_git_projects() {
        let root = scratch("select");
        let repo = root.join("repo");
        let plain = root.join("plain");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(&plain).unwrap();
        let lines: Vec<String> = [
            repo.to_str().unwrap(),
            plain.to_str().unwrap(),
            repo.to_str().unwrap(),
            "relative/path",
            root.join("missing").to_str().unwrap(),
        ]
        .map(str::to_string)
        .to_vec();
        assert_eq!(
            select(&lines, ProjectFilter::Git),
            std::slice::from_ref(&repo),
            "no leading cwd; repeats, relative paths and missing directories are dropped"
        );
        assert_eq!(select(&lines, ProjectFilter::None), [repo, plain]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_project_is_named_for_its_git_root_not_the_folder_inside_it() {
        let root = scratch("name");
        let repo = root.join("agent-radar");
        let inner = repo.join("src/deep");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(&inner).unwrap();
        assert_eq!(project_name(&inner), "agent-radar");
        assert_eq!(project_name(&repo), "agent-radar");
        let plain = root.join("notes");
        fs::create_dir_all(&plain).unwrap();
        assert_eq!(project_name(&plain), "notes");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_missing_or_failing_source_uses_the_base_and_says_why() {
        let cwd = scratch("source");
        let missing = ["radar-no-such-program-xyz".to_string()];
        let found = candidates(Some(&missing), ProjectFilter::Git, &cwd);
        assert_eq!(found.directories, std::slice::from_ref(&cwd));
        assert!(
            found
                .note
                .is_some_and(|note| note.contains("did not start"))
        );
        let failing = ["false".to_string()];
        let found = candidates(Some(&failing), ProjectFilter::Git, &cwd);
        assert_eq!(found.directories, std::slice::from_ref(&cwd));
        assert!(found.note.is_some());
        let printing = ["echo".to_string(), cwd.to_string_lossy().into_owned()];
        let found = candidates(Some(&printing), ProjectFilter::None, &cwd);
        assert_eq!(
            found.directories,
            std::slice::from_ref(&cwd),
            "a source result is not repeated as a fallback"
        );
        assert!(found.note.is_none());
        let unusable = cwd.join("missing");
        let found = candidates(Some(&failing), ProjectFilter::Git, &unusable);
        assert!(found.directories.is_empty());
        assert!(found.note.unwrap().contains("project base"));
        assert!(select(&[], ProjectFilter::Git).is_empty());
        fs::remove_dir_all(&cwd).unwrap();
    }
}
