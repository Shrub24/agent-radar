//! Frontend-owned discovery and terminal hosting. Interaction state stays in the daemon.
use crate::control_plane::public_contracts::ui_session::{InteractionStage, UiAction};
use crate::ui_session::client::Wake;
use crate::{App, config::TuiConfig};
use std::process::ExitStatus;

type PopupExit = (String, Result<ExitStatus, String>);
use std::sync::mpsc;

#[derive(Default)]
pub struct Presentation {
    preparing: Option<String>,
    prepared: Option<mpsc::Receiver<(String, crate::project::Candidates)>>,
    hosted: Option<String>,
    popup: Option<mpsc::Receiver<PopupExit>>,
}

impl Presentation {
    pub fn poll(&mut self, app: &mut App, config: &TuiConfig, detached: bool, wake: &Wake) -> bool {
        let Some(mirror) = app.session_mirror() else {
            return false;
        };
        let blocking = mirror.state.blocking.as_ref();
        if let Some(rx) = &self.popup
            && let Ok((id, status)) = rx.try_recv()
        {
            if blocking.is_some_and(|b| b.interaction_id == id) {
                app.submit_session(UiAction::PresentationFailed {
                    interaction_id: id,
                    message: match status {
                        Ok(status) => format!("popup closed before resolution ({status})"),
                        Err(error) => error,
                    },
                });
            }
            self.popup = None;
        }
        if let Some(rx) = &self.prepared
            && let Ok((id, found)) = rx.try_recv()
        {
            if blocking.is_some_and(|b| b.interaction_id == id) {
                app.submit_session(UiAction::ProvideChoices {
                    interaction_id: id,
                    choices: found
                        .directories
                        .into_iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect(),
                    note: found.note,
                });
            }
            self.prepared = None;
            return true;
        }
        let Some(b) = blocking.filter(|b| b.owner == mirror.attachment_id) else {
            return false;
        };
        if b.stage == InteractionStage::Preparing
            && self.preparing.as_deref() != Some(&b.interaction_id)
        {
            self.preparing = Some(b.interaction_id.clone());
            let id = b.interaction_id.clone();
            let config = config.clone();
            let wake = wake.clone();
            let (tx, rx) = mpsc::channel();
            self.prepared = Some(rx);
            std::thread::spawn(move || {
                let base = config
                    .project_base
                    .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
                    .unwrap_or_default();
                let found = crate::project::candidates(
                    config.project_source.as_deref(),
                    config.project_filter,
                    &base,
                );
                let _ = tx.send((id, found));
                wake.notify();
            });
        }
        if b.stage == InteractionStage::AwaitingResult
            && self.hosted.as_deref() != Some(&b.interaction_id)
        {
            self.hosted = Some(b.interaction_id.clone());
            if detached {
                let result = std::env::current_exe()
                    .map_err(|e| e.to_string())
                    .and_then(|exe| {
                        crate::workspace::presenter_command(
                            &exe.to_string_lossy(),
                            &b.interaction_id,
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
                        self.popup = Some(rx);
                        let id = b.interaction_id.clone();
                        let wake = wake.clone();
                        std::thread::spawn(move || {
                            let _ = tx.send((id, child.wait().map_err(|error| error.to_string())));
                            wake.notify();
                        });
                    }
                    Err(message) => {
                        app.submit_session(UiAction::PresentationFailed {
                            interaction_id: b.interaction_id.clone(),
                            message,
                        });
                        return true;
                    }
                }
            } else {
                app.submit_session(UiAction::ClaimPresentation {
                    interaction_id: b.interaction_id.clone(),
                });
            }
            app.submit_session(UiAction::AcknowledgePresentation {
                effect_id: b.interaction_id.clone(),
            });
            return true;
        }
        false
    }
}
