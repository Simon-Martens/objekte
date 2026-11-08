//! Opening-dialog draft lifecycle, separate from the active repository.

use crate::controller::open_detection::DetectedSource;

use super::model::{OpenDraft, OpenMode};


/// Explicit source interpretation selected under Advanced options.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SourceChoice {
    #[default]
    Automatic,
    Folder,
    ConfiguredRepository,
}


/// Advisory mode detection for the current first folder.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum DetectionState {
    #[default]
    Idle,
    Pending {
        path: String,
    },
    Ready {
        path: String,
        source: DetectedSource,
    },
    Failed {
        path: String,
        message: String,
    },
}


/// Editable modal state; none of its changes open data before confirmation.
#[derive(Clone, Debug)]
pub struct OpenDialogState {
    pub visible: bool,
    pub folders: Vec<String>,
    pub source_choice: SourceChoice,
    pub advanced_expanded: bool,
    pub detection: DetectionState,
    pub generation: u64,
    pub picker_generation: u64,
    pub pending_picker_slot: Option<usize>,
    pub error: Option<String>,
}


impl Default for OpenDialogState {
    fn default() -> Self {
        Self {
            visible: false,
            folders: vec![String::new()],
            source_choice: SourceChoice::Automatic,
            advanced_expanded: false,
            detection: DetectionState::Idle,
            generation: 0,
            picker_generation: 0,
            pending_picker_slot: None,
            error: None,
        }
    }
}


impl OpenDialogState {
    pub fn show(&mut self) {
        self.visible = true;
        self.error = None;
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.generation = self.generation.wrapping_add(1);
        self.picker_generation = self.picker_generation.wrapping_add(1);
        self.pending_picker_slot = None;
        self.detection = DetectionState::Idle;
        self.error = None;
    }

    pub fn edit_folder(&mut self, slot: usize, path: String) -> bool {
        let Some(folder) = self.folders.get_mut(slot) else {
            return false;
        };
        if *folder == path {
            return false;
        }
        *folder = path;
        self.invalidate_detection();
        true
    }

    pub fn add_folder(&mut self) -> usize {
        self.folders.push(String::new());
        self.invalidate_detection();
        self.folders.len() - 1
    }

    pub fn remove_folder(&mut self, slot: usize) -> bool {
        if self.folders.len() <= 1 || slot >= self.folders.len() {
            return false;
        }
        self.folders.remove(slot);
        self.invalidate_detection();
        true
    }

    pub fn begin_detection(&mut self) -> Option<(u64, String)> {
        if !self.visible || self.folders.len() != 1 {
            self.detection = DetectionState::Idle;
            return None;
        }
        let path = self.folders[0].trim().to_owned();
        if path.is_empty() {
            self.detection = DetectionState::Idle;
            return None;
        }
        self.generation = self.generation.wrapping_add(1);
        self.detection = DetectionState::Pending { path: path.clone() };
        Some((self.generation, path))
    }

    pub fn accept_detection(
        &mut self,
        generation: u64,
        path: &str,
        result: Result<DetectedSource, String>,
    ) -> bool {
        if !self.visible
            || self.folders.len() != 1
            || self.generation != generation
            || self.folders[0].trim() != path
            || !matches!(&self.detection, DetectionState::Pending { path: pending } if pending == path)
        {
            return false;
        }
        self.detection = match result {
            Ok(source) => DetectionState::Ready {
                path: path.to_owned(),
                source,
            },
            Err(message) => DetectionState::Failed {
                path: path.to_owned(),
                message,
            },
        };
        true
    }

    pub fn begin_picker(&mut self, slot: usize) -> Option<u64> {
        if !self.visible || slot >= self.folders.len() {
            return None;
        }
        self.picker_generation = self.picker_generation.wrapping_add(1);
        self.pending_picker_slot = Some(slot);
        Some(self.picker_generation)
    }

    pub fn accept_picker(&mut self, generation: u64, slot: usize, path: String) -> bool {
        if !self.visible
            || self.picker_generation != generation
            || self.pending_picker_slot != Some(slot)
        {
            return false;
        }
        self.pending_picker_slot = None;
        self.edit_folder(slot, path)
    }

    pub fn to_open_draft(&self, template: &OpenDraft) -> Result<OpenDraft, String> {
        if !self.visible {
            return Err("Open dialog is closed".into());
        }
        if self.folders.iter().any(|path| path.trim().is_empty()) {
            return Err("Choose a folder for every row".into());
        }
        if self.folders.iter().any(|path| path.contains(['\n', '\r'])) {
            return Err("Enter one folder path per row".into());
        }
        let mode = if self.folders.len() > 1 {
            OpenMode::MultipleFolders
        } else {
            match self.source_choice {
                SourceChoice::Folder => OpenMode::Folder,
                SourceChoice::ConfiguredRepository => OpenMode::ConfiguredRepository,
                SourceChoice::Automatic => match &self.detection {
                    DetectionState::Ready {
                        source: DetectedSource::PlainFolder,
                        ..
                    } => OpenMode::Folder,
                    DetectionState::Ready {
                        source: DetectedSource::ConfiguredRepository { .. },
                        ..
                    } => OpenMode::ConfiguredRepository,
                    DetectionState::Ready {
                        source: DetectedSource::InvalidConfiguration { message },
                        ..
                    }
                    | DetectionState::Failed { message, .. } => {
                        return Err(format!(
                            "Configured repository could not be read: {message}. Choose Folder under Advanced to override."
                        ));
                    }
                    DetectionState::Idle | DetectionState::Pending { .. } => {
                        return Err("Wait for folder detection before opening".into());
                    }
                },
            }
        };
        let mut draft = template.clone();
        draft.mode = mode;
        draft.paths = self
            .folders
            .iter()
            .map(|path| path.trim())
            .collect::<Vec<_>>()
            .join("\n");
        Ok(draft)
    }

    fn invalidate_detection(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.detection = DetectionState::Idle;
        self.error = None;
    }
}


#[cfg(test)]
#[path = "open_dialog.test.rs"]
mod tests;
