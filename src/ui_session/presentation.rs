//! Frontend-owned discovery and terminal hosting. Interaction state stays in the daemon.
use crate::control_plane::public_contracts::ui_session::{InteractionStage, UiAction};
use crate::ui_session::client::Wake;
use crate::{App, config::ProjectsConfig};
use std::process::ExitStatus;
use std::sync::mpsc;

#[derive(Default)]
pub struct Presentation {
    current: Option<LocalWork>,
}

struct LocalWork {
    interaction_id: String,
    progress: Progress,
}

enum Progress {
    New,
    Discovering(mpsc::Receiver<crate::project::Candidates>),
    ChoicesProvided,
    Hosting(mpsc::Receiver<Result<ExitStatus, String>>),
    Presented,
}

impl Presentation {
    pub fn poll(
        &mut self,
        app: &mut App,
        projects: &ProjectsConfig,
        detached: bool,
        wake: &Wake,
    ) -> bool {
        let Some(mirror) = app.session_mirror() else {
            return false;
        };
        let Some(blocking) = mirror
            .state
            .blocking
            .as_ref()
            .filter(|b| b.owner == mirror.attachment_id)
        else {
            self.current = None;
            return false;
        };
        if self
            .current
            .as_ref()
            .is_none_or(|work| work.interaction_id != blocking.interaction_id)
        {
            self.current = Some(LocalWork {
                interaction_id: blocking.interaction_id.clone(),
                progress: Progress::New,
            });
        }
        let work = self.current.as_mut().expect("current interaction");
        match &work.progress {
            Progress::Discovering(rx) => {
                if let Ok(found) = rx.try_recv() {
                    app.submit_session(UiAction::ProvideChoices {
                        interaction_id: work.interaction_id.clone(),
                        choices: found
                            .directories
                            .into_iter()
                            .map(|p| p.to_string_lossy().into_owned())
                            .collect(),
                        note: found.note,
                    });
                    work.progress = Progress::ChoicesProvided;
                    return true;
                }
            }
            Progress::Hosting(rx) => {
                if let Ok(status) = rx.try_recv() {
                    app.submit_session(UiAction::PresentationFailed {
                        interaction_id: work.interaction_id.clone(),
                        message: match status {
                            Ok(status) => format!("popup closed before resolution ({status})"),
                            Err(error) => error,
                        },
                    });
                    work.progress = Progress::Presented;
                    return true;
                }
            }
            _ => {}
        }
        if blocking.stage == InteractionStage::Preparing && matches!(work.progress, Progress::New) {
            let projects = projects.clone();
            let wake = wake.clone();
            let (tx, rx) = mpsc::channel();
            work.progress = Progress::Discovering(rx);
            std::thread::spawn(move || {
                let base = projects
                    .base
                    .clone()
                    .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
                    .unwrap_or_default();
                let found =
                    crate::project::candidates(projects.source.as_deref(), projects.filter, &base);
                let _ = tx.send(found);
                wake.notify();
            });
        }
        if blocking.stage == InteractionStage::AwaitingResult
            && matches!(work.progress, Progress::New | Progress::ChoicesProvided)
        {
            work.progress = Progress::Presented;
            if detached {
                let result = std::env::current_exe()
                    .map_err(|e| e.to_string())
                    .and_then(|exe| {
                        crate::workspace::presenter_command(
                            &exe.to_string_lossy(),
                            &work.interaction_id,
                        )
                        .ok_or_else(|| "workspace popup host is unavailable".to_string())
                    })
                    .and_then(|argv| {
                        std::process::Command::new(&argv[0])
                            .args(&argv[1..])
                            .spawn()
                            .map_err(|e| e.to_string())
                    });
                match result {
                    Ok(mut child) => {
                        let (tx, rx) = mpsc::channel();
                        work.progress = Progress::Hosting(rx);
                        let wake = wake.clone();
                        std::thread::spawn(move || {
                            let _ = tx.send(child.wait().map_err(|error| error.to_string()));
                            wake.notify();
                        });
                    }
                    Err(message) => {
                        app.submit_session(UiAction::PresentationFailed {
                            interaction_id: work.interaction_id.clone(),
                            message,
                        });
                    }
                }
            } else {
                app.submit_session(UiAction::ClaimPresentation {
                    interaction_id: work.interaction_id.clone(),
                });
            }
            return true;
        }
        false
    }
}
