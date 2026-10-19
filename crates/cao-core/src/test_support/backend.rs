//! The fake Asset Execution Backend.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::execution::{AssetExecutionBackend, OperationResult};
use crate::routing::{AssetOperations, ExecutionMode, MeshVariant, TextureVariant};

/// How the fake backend treats one Asset, chosen by a marker in its file name.
///
/// `unloadable` fails to load, `panics` panics while optimizing, `changes`
/// reports a change to persist; anything else is evaluated as unchanged. In
/// Apply a change is saved as `optimized <name>` (an Animation's output as
/// `converted <name>`), unless the name also contains `unsaveable`.
pub struct FakeBackend {
    pub calls: Arc<Mutex<Vec<String>>>,
    loaded: Option<PathBuf>,
}

impl FakeBackend {
    /// A backend appending each call, as `<call> <file name>`, to `calls`.
    pub fn new(calls: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            calls,
            loaded: None,
        }
    }

    fn record(&self, call: &str, path: &Path) {
        let name = path.file_name().unwrap().to_string_lossy();
        self.calls.lock().unwrap().push(format!("{call} {name}"));
    }

    fn evaluate(&self) -> OperationResult {
        let name = self
            .loaded
            .as_ref()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        if name.contains("panics") {
            panic!("the backend panicked while optimizing {name}");
        }
        if name.contains("changes") {
            OperationResult::changed()
        } else {
            OperationResult::unchanged()
        }
    }

    fn load(&mut self, call: &str, path: &Path) -> bool {
        self.record(call, path);
        self.loaded = Some(path.to_path_buf());
        !path.to_string_lossy().contains("unloadable")
    }

    /// The loaded Asset's file name.
    fn loaded_name(&self) -> String {
        let loaded = self.loaded.as_ref().unwrap();
        loaded.file_name().unwrap().to_string_lossy().into_owned()
    }

    /// Writes `optimized <name>` to the staged `path`, unless the loaded
    /// Asset's name contains `unsaveable`.
    fn save(&mut self, call: &str, path: &Path) -> bool {
        self.record(call, path);
        let name = self.loaded_name();
        if name.contains("unsaveable") {
            return false;
        }
        std::fs::write(path, format!("optimized {name}")).unwrap();
        true
    }
}

impl AssetExecutionBackend for FakeBackend {
    fn load_texture(&mut self, path: &Path, _variant: TextureVariant) -> bool {
        self.load("load_texture", path)
    }

    fn optimize_texture(
        &mut self,
        _operations: AssetOperations,
        _mode: ExecutionMode,
    ) -> OperationResult {
        self.record("optimize_texture", &self.loaded.clone().unwrap());
        self.evaluate()
    }

    fn save_texture(&mut self, path: &Path) -> bool {
        self.save("save_texture", path)
    }

    fn remove_texture(&mut self, path: &Path, remove_verified: &mut dyn FnMut() -> bool) -> bool {
        self.record("remove_texture", path);
        remove_verified()
    }

    fn load_mesh(&mut self, path: &Path, _variant: MeshVariant) -> bool {
        self.load("load_mesh", path)
    }

    fn optimize_mesh(
        &mut self,
        path: &Path,
        _mod_root: &Path,
        _mode: ExecutionMode,
    ) -> OperationResult {
        self.record("optimize_mesh", path);
        self.evaluate()
    }

    fn maintain_mesh_references(&mut self, _mode: ExecutionMode) -> OperationResult {
        self.record("maintain_mesh_references", &self.loaded.clone().unwrap());
        OperationResult::unchanged()
    }

    fn save_mesh(&mut self, path: &Path) -> bool {
        self.save("save_mesh", path)
    }

    fn optimize_animation(
        &mut self,
        path: &Path,
        output_path: Option<&Path>,
        mode: ExecutionMode,
    ) -> OperationResult {
        assert_eq!(
            output_path.is_some(),
            mode == ExecutionMode::Apply,
            "only Apply gets an Animation output path"
        );
        self.loaded = Some(path.to_path_buf());
        self.record("optimize_animation", path);
        let result = self.evaluate();
        if let Some(output) = output_path
            && result.would_change()
        {
            std::fs::write(output, format!("converted {}", self.loaded_name())).unwrap();
        }
        result
    }
}
