use anyhow::Result;
use rten::Model;
use std::path::Path;

/// Model source enum - replica of CLI pattern
#[derive(Debug, Clone)]
pub enum ModelSource {
    /// Local file path
    File(String),
    /// Remote URL (not used in our case but keeping for CLI compatibility)
    Url(String),
}

impl ModelSource {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Self {
        Self::File(path.as_ref().to_string_lossy().to_string())
    }
}

/// Load model from source - replica of CLI model loading
pub fn load_model(source: &ModelSource) -> Result<Model> {
    match source {
        ModelSource::File(path) => Model::load_file(path)
            .map_err(|e| anyhow::anyhow!("Failed to load model from {}: {}", path, e)),
        ModelSource::Url(_url) => {
            // For now, we only support local files
            // The CLI implementation would download and cache here
            anyhow::bail!("URL model loading not implemented - use local files")
        }
    }
}

/// Default model paths - following CLI pattern
pub struct DefaultModels {
    pub detection: String,
    pub recognition: String,
}

impl DefaultModels {
    pub fn local(detection_path: &str, recognition_path: &str) -> Self {
        Self {
            detection: detection_path.to_string(),
            recognition: recognition_path.to_string(),
        }
    }
}
