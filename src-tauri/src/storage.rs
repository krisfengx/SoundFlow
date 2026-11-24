use crate::error::{AppError, AppResult};
use serde::Serialize;
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

const DATA_ROOT_ENV: &str = "SOUNDFLOW_DATA_ROOT";
// Explicit override for the managed runtime environments directory. When set,
// it wins over every built-in default (useful for portable/CI layouts).
const RUNTIME_ENVS_OVERRIDE_ENV: &str = "SOUNDFLOW_RUNTIME_ENVS_OVERRIDE";
const DATA_ROOT_DIR_NAME: &str = ".soundflow";
/// Older releases kept development/portable data in a plain `data/` folder and
/// packaged data in `~/.soundflow`. Both remain readable as legacy fallbacks.
const LEGACY_LOCAL_DATA_ROOT_DIR_NAME: &str = "data";
static JSON_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
// Multiple WebView windows can autosave the same store at once. Serialize the temporary-file
// replacement in the Rust process so a second writer never races the first ReplaceFileW call.
static JSON_WRITE_LOCK: Mutex<()> = Mutex::new(());
#[cfg(windows)]
const PORTABLE_MARKER_FILE_NAME: &str = "soundflow.portable";
/// Pointer written outside the data root so changing the root cannot relocate
/// the file that tells the app where the root is.
const DATA_ROOT_POINTER_FILE_NAME: &str = "soundflow-root.json";
const DATA_ROOT_POINTER_KEY: &str = "dataRoot";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppPathsPayload {
    pub data_root: String,
    pub settings_dir: String,
    pub models_dir: String,
    pub outputs_dir: String,
    pub editor_projects_dir: String,
    pub logs_dir: String,
    pub temp_dir: String,
}

pub fn home_dir(app: &AppHandle) -> AppResult<PathBuf> {
    app.path()
        .home_dir()
        .map_err(|error| AppError::Worker(error.to_string()))
}

fn executable_dir() -> AppResult<PathBuf> {
    std::env::current_exe()?
        .parent()
        .map(PathBuf::from)
        .ok_or_else(|| AppError::Worker("failed to resolve executable directory".into()))
}

fn packaged_data_root_dir(_app: &AppHandle) -> AppResult<PathBuf> {
    // Application-owned data belongs next to the installed executable, not in
    // the user profile. A legacy home-directory root is adopted below.
    Ok(executable_dir()?.join(DATA_ROOT_DIR_NAME))
}

fn development_data_root_dir() -> AppResult<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let project_root = manifest_dir
        .parent()
        .ok_or_else(|| AppError::Worker("failed to resolve project root".into()))?;
    Ok(project_root.join(DATA_ROOT_DIR_NAME))
}

fn portable_data_root_dir() -> AppResult<Option<PathBuf>> {
    #[cfg(windows)]
    {
        let exe_dir = executable_dir()?;
        if exe_dir.join(PORTABLE_MARKER_FILE_NAME).exists() {
            return Ok(Some(exe_dir.join(DATA_ROOT_DIR_NAME)));
        }
    }
    Ok(None)
}

fn data_root_has_user_models(root: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(root.join("models")) else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name != "_cache" {
            return true;
        }
    }
    false
}

fn legacy_data_root_dir(app: &AppHandle, development: bool) -> Option<PathBuf> {
    let candidates = if development {
        vec![PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()?
            .join(LEGACY_LOCAL_DATA_ROOT_DIR_NAME)]
    } else {
        let exe_dir = executable_dir().ok()?;
        vec![
            exe_dir.join(LEGACY_LOCAL_DATA_ROOT_DIR_NAME),
            home_dir(app).ok()?.join(DATA_ROOT_DIR_NAME),
        ]
    };
    candidates.into_iter().find(|path| path.is_dir())
}

fn is_cargo_profile_executable(exe: &Path, target_dir: &Path) -> bool {
    let Some(profile_dir) = exe.parent() else {
        return false;
    };
    let profile = profile_dir.file_name().and_then(|value| value.to_str());
    if !matches!(profile, Some("debug" | "release")) {
        return false;
    }
    let Some(profile_parent) = profile_dir.parent() else {
        return false;
    };
    // Native builds live at target/{debug,release}; cross-compiled artifacts add exactly one
    // target-triple directory. Bundle contents are deeper and must use packaged semantics even
    // while CI is inspecting them in target/**/bundle.
    profile_parent == target_dir || profile_parent.parent() == Some(target_dir)
}

pub fn is_development_executable() -> bool {
    let target_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::canonicalize(path).ok())
        .zip(std::fs::canonicalize(target_dir).ok())
        .map(|(exe, target)| is_cargo_profile_executable(&exe, &target))
        .unwrap_or(false)
}

fn resolve_data_root(
    env_root: Option<PathBuf>,
    development_root: PathBuf,
    portable_root: Option<PathBuf>,
    legacy_root: PathBuf,
    is_development: bool,
) -> PathBuf {
    if let Some(root) = env_root {
        return root;
    }
    if is_development {
        return development_root;
    }
    if let Some(root) = portable_root {
        return root;
    }
    legacy_root
}

fn data_root_pointer_path(app: &AppHandle) -> AppResult<PathBuf> {
    #[cfg(windows)]
    if let Some(portable) = portable_data_root_dir()? {
        let _ = portable;
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                return Ok(exe_dir.join(DATA_ROOT_POINTER_FILE_NAME));
            }
        }
    }
    if is_development_executable() {
        let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .ok_or_else(|| AppError::Worker("failed to resolve project root".into()))?
            .to_path_buf();
        return Ok(project_root.join(DATA_ROOT_POINTER_FILE_NAME));
    }
    Ok(home_dir(app)?.join(DATA_ROOT_POINTER_FILE_NAME))
}

fn path_from_data_root_pointer(value: &Value) -> Option<PathBuf> {
    let raw = value.get(DATA_ROOT_POINTER_KEY)?.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    let path = PathBuf::from(raw);
    path.is_absolute().then_some(path)
}

fn configured_data_root_from_pointer(app: &AppHandle) -> Option<PathBuf> {
    let path = data_root_pointer_path(app).ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let root = path_from_data_root_pointer(&value)?;
    root.is_dir().then_some(root)
}

pub fn data_root_dir(app: &AppHandle) -> AppResult<PathBuf> {
    let env_root = std::env::var_os(DATA_ROOT_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    // An explicit environment override always wins. Otherwise a pointer written
    // outside the data root takes precedence over every built-in default.
    if env_root.is_none() {
        if let Some(root) = configured_data_root_from_pointer(app) {
            return Ok(root);
        }
    }
    let development = is_development_executable();
    let default_root = resolve_data_root(
        env_root,
        development_data_root_dir()?,
        portable_data_root_dir()?,
        packaged_data_root_dir(app)?,
        development,
    );
    // A freshly pre-created empty directory is not a real data root: dev-env
    // may create `.soundflow/models/_cache` before the app ever writes state.
    // Adopt the old root when the default has no settings store yet.
    let default_has_state = default_root.join("settings").join("app.json").is_file()
        && data_root_has_user_models(&default_root);
    if !default_has_state {
        if let Some(legacy) = legacy_data_root_dir(app, development) {
            if legacy.join("settings").join("app.json").is_file() && data_root_has_user_models(&legacy) {
                return Ok(legacy);
            }
        }
    }
    Ok(default_root)
}

fn move_data_root_entry(source: &Path, destination: &Path) -> AppResult<()> {
    match std::fs::rename(source, destination) {
        Ok(()) => return Ok(()),
        Err(error) => {
            // Windows ERROR_NOT_SAME_DEVICE / Unix EXDEV: copy tree then remove source.
            if !matches!(error.raw_os_error(), Some(17 | 18)) {
                return Err(error.into());
            }
        }
    }
    if source.is_dir() {
        std::fs::create_dir_all(destination)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            move_data_root_entry(&entry.path(), &destination.join(entry.file_name()))?;
        }
        std::fs::remove_dir(source)?;
    } else {
        std::fs::copy(source, destination)?;
        std::fs::remove_file(source)?;
    }
    Ok(())
}

/// Move the whole application data root to a user-chosen directory.
///
/// The pointer lives outside the root (project root in dev, exe dir for portable
/// builds, home dir otherwise), so the file telling the app where the root is
/// never moved. The change takes effect on the next launch; the frontend must
/// tell the user to restart.
fn display_root(path: &Path) -> String {
    let text = path.to_string_lossy().to_string();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    if cfg!(windows) {
        text.replace('/', "\\")
    } else {
        text.to_string()
    }
}

fn rewrite_json_paths(value: &mut Value, old_root: &str, new_root: &str) {
    let normalize = |text: &str| {
        if cfg!(windows) { text.replace('/', "\\") } else { text.to_string() }
    };
    let old_root = normalize(old_root);
    let new_root = normalize(new_root);
    match value {
        Value::String(text) => {
            let normalized = normalize(text);
            if normalized == old_root {
                *text = new_root.clone();
            } else if let Some(rest) = normalized.strip_prefix(&old_root) {
                if rest.starts_with('\\') || rest.starts_with('/') {
                    *text = format!("{new_root}{rest}");
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                rewrite_json_paths(item, &old_root, &new_root);
            }
        }
        Value::Object(entries) => {
            for entry in entries.values_mut() {
                rewrite_json_paths(entry, &old_root, &new_root);
            }
        }
        _ => {}
    }
}

fn rewrite_settings_paths(settings_dir: &Path, old_root: &str, new_root: &str) -> AppResult<()> {
    if !settings_dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(settings_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let mut value: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        rewrite_json_paths(&mut value, old_root, new_root);
        write_json_file(&path, &value)?;
    }
    Ok(())
}

fn path_contains_files(path: &Path) -> AppResult<bool> {
    if path.is_file() {
        return Ok(true);
    }
    for entry in std::fs::read_dir(path)? {
        if path_contains_files(&entry?.path())? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn prepare_data_root_target(target: &Path) -> AppResult<()> {
    if !target.exists() {
        std::fs::create_dir_all(target)?;
        return Ok(());
    }
    if !target.is_dir() {
        return Err(AppError::Worker("target is not a directory".into()));
    }
    let allowed = ["models", "plugins", "temp"];
    for entry in std::fs::read_dir(target)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !entry.path().is_dir() || !allowed.contains(&name.as_str()) || path_contains_files(&entry.path())? {
            return Err(AppError::Worker("target directory must be empty or contain only empty app cache directories".into()));
        }
    }
    // Dev scripts pre-create empty cache directories; clear them so the move
    // below sees an empty target.
    for entry in std::fs::read_dir(target)? {
        std::fs::remove_dir_all(entry?.path())?;
    }
    Ok(())
}

fn migrate_data_root_paths(current: &Path, target_raw: &str, pointer: &Path) -> AppResult<Value> {
    let target = PathBuf::from(target_raw.trim());
    if !target.is_absolute() {
        return Err(AppError::Worker("data root must be an absolute path".into()));
    }
    prepare_data_root_target(&target)?;
    let target = std::fs::canonicalize(&target)?;
    let current = std::fs::canonicalize(current)?;
    if current == target {
        return Ok(serde_json::json!({ "dataRoot": display_root(&target), "moved": [], "skipped": [] }));
    }
    if target.starts_with(&current) {
        return Err(AppError::Worker("target cannot be inside the current data root".into()));
    }
    if !current.is_dir() {
        return Err(AppError::Worker("current data root does not exist".into()));
    }

    let mut moved = Vec::new();
    for entry in std::fs::read_dir(&current)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        // Logs and temp files may still be open in this process; leaving them
        // behind is safer than failing the whole migration on a sharing lock.
        if name == "logs" || name == "temp" {
            continue;
        }
        let destination = target.join(entry.file_name());
        if destination.exists() {
            return Err(AppError::Worker(format!("target already contains {name}")));
        }
        let source = entry.path();
        move_data_root_entry(&source, &destination).map_err(|error| {
            AppError::Worker(format!(
                "failed to move {} -> {}: {error}",
                display_root(&source),
                display_root(&destination)
            ))
        })?;
        moved.push(name);
    }

    // Settings such as modelDir/outputDir are absolute and may point inside
    // the old data root; rewrite them before the pointer makes the new root live.
    rewrite_settings_paths(
        &target.join("settings"),
        &display_root(&current),
        &display_root(&target),
    )
    .map_err(|error| AppError::Worker(format!("failed to rewrite migrated settings: {error}")))?;

    if let Some(parent) = pointer.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_json_file(pointer, &serde_json::json!({ DATA_ROOT_POINTER_KEY: display_root(&target) }))
        .map_err(|error| AppError::Worker(format!("failed to write {}: {error}", pointer.display())))?;
    Ok(serde_json::json!({
        "dataRoot": display_root(&target),
        "moved": moved,
        "skipped": ["logs", "temp"],
    }))
}

/// Move the whole application data root to a user-chosen directory.
///
/// The pointer lives outside the root (project root in dev, exe dir for portable
/// builds, home dir otherwise), so the file telling the app where the root is
/// never moved. The change takes effect on the next launch; the frontend must
/// tell the user to restart.
pub fn migrate_data_root(app: &AppHandle, target_raw: &str) -> AppResult<Value> {
    let current = data_root_dir(app)?;
    let pointer = data_root_pointer_path(app)?;
    migrate_data_root_paths(&current, target_raw, &pointer)
}

pub fn settings_dir(app: &AppHandle) -> AppResult<PathBuf> {
    Ok(data_root_dir(app)?.join("settings"))
}

pub fn models_dir(app: &AppHandle) -> AppResult<PathBuf> {
    Ok(data_root_dir(app)?.join("models"))
}

/// Third-party libraries may materialise model weights in their own caches.
/// Keep all three caches inside the app model root in every distribution.
fn model_cache_paths(models: &Path) -> [PathBuf; 3] {
    let root = models.join("_cache");
    [
        root.join("hf"),
        root.join("torch"),
        root.join("modelscope"),
    ]
}

pub fn model_cache_dirs(app: &AppHandle) -> AppResult<[PathBuf; 3]> {
    Ok(model_cache_paths_from_root(&model_cache_root(app)?))
}

fn model_cache_dir_from_settings(value: &Value) -> Option<PathBuf> {
    let raw = value.get("modelCacheDir")?.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    let path = PathBuf::from(raw);
    path.is_absolute().then_some(path)
}

pub fn configured_model_cache_dir(app: &AppHandle) -> Option<PathBuf> {
    // Read the store file directly: `read_app_store` calls
    // `ensure_app_directories`, which in turn calls `model_cache_dirs`.
    let path = app_store_path(app, "app-settings").ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    model_cache_dir_from_settings(&value)
}

pub fn model_cache_root(app: &AppHandle) -> AppResult<PathBuf> {
    if let Some(root) = configured_model_cache_dir(app) {
        return Ok(root);
    }
    Ok(models_dir(app)?.join("_cache"))
}

fn model_cache_paths_from_root(root: &Path) -> [PathBuf; 3] {
    [
        root.join("hf"),
        root.join("torch"),
        root.join("modelscope"),
    ]
}

pub fn set_model_cache_dir(app: &AppHandle, path: &Path) -> AppResult<()> {
    let mut settings = read_app_store(app, "app-settings")?;
    if !settings.is_object() {
        settings = serde_json::json!({});
    }
    settings["modelCacheDir"] = serde_json::json!(display_root(path));
    write_json_file(&app_store_path(app, "app-settings")?, &settings)
}

pub fn migrate_model_cache_dir(app: &AppHandle, target_raw: &str) -> AppResult<Value> {
    let target = PathBuf::from(target_raw.trim());
    if !target.is_absolute() {
        return Err(AppError::Worker("model cache directory must be absolute".into()));
    }
    if target.exists() {
        if !target.is_dir() || path_contains_files(&target)? {
            return Err(AppError::Worker("target directory must be empty".into()));
        }
    }
    std::fs::create_dir_all(&target)?;
    let target = std::fs::canonicalize(&target)?;
    let current = model_cache_root(app)?;
    if std::fs::canonicalize(&current).ok().as_deref() == Some(target.as_path()) {
        return Ok(serde_json::json!({ "modelCacheDir": display_root(&target), "moved": [] }));
    }
    let mut moved = Vec::new();
    for name in ["hf", "torch", "modelscope"] {
        let source = current.join(name);
        if !source.exists() {
            continue;
        }
        let destination = target.join(name);
        move_data_root_entry(&source, &destination).map_err(|error| {
            AppError::Worker(format!("failed to move {}: {error}", source.display()))
        })?;
        moved.push(name);
    }
    set_model_cache_dir(app, &target)?;
    Ok(serde_json::json!({
        "modelCacheDir": display_root(&target),
        "moved": moved,
    }))
}

pub fn outputs_dir(app: &AppHandle) -> AppResult<PathBuf> {
    Ok(data_root_dir(app)?.join("outputs"))
}

pub fn editor_projects_dir(app: &AppHandle) -> AppResult<PathBuf> {
    Ok(data_root_dir(app)?.join("editor-projects"))
}

pub fn logs_dir(app: &AppHandle) -> AppResult<PathBuf> {
    Ok(data_root_dir(app)?.join("logs"))
}

pub fn temp_dir(app: &AppHandle) -> AppResult<PathBuf> {
    Ok(data_root_dir(app)?.join("temp"))
}

// Executable siblings and resource-specific fallbacks stay with their callers.
pub(crate) fn resource_roots(resource: &Path) -> [PathBuf; 3] {
    [
        resource.to_path_buf(),
        resource.join("_up_"),
        resource.join("resources"),
    ]
}

pub fn runtime_root_dir(app: &AppHandle) -> AppResult<PathBuf> {
    let resource = app.path().resource_dir().ok();
    let exe_dir = std::env::current_exe()?
        .parent()
        .map(PathBuf::from)
        .ok_or_else(|| AppError::Worker("failed to resolve executable directory".into()))?;
    Ok(runtime_root_from(resource.as_deref(), &exe_dir))
}

fn runtime_root_from(resource: Option<&Path>, exe_dir: &Path) -> PathBuf {
    let mut candidates: Vec<_> = resource
        .into_iter()
        .flat_map(resource_roots)
        .map(|root| root.join("python-runtime"))
        .collect();
    candidates.push(exe_dir.join("python-runtime"));
    candidates
        .into_iter()
        .find(|path| path.is_dir())
        .unwrap_or_else(|| exe_dir.join("python-runtime"))
}

pub fn bundled_runtime_envs_dir(app: &AppHandle) -> AppResult<Option<PathBuf>> {
    let resource = app.path().resource_dir().ok();
    let exe_dir = std::env::current_exe()?
        .parent()
        .map(PathBuf::from)
        .ok_or_else(|| AppError::Worker("failed to resolve executable directory".into()))?;
    Ok(bundled_runtime_envs_from(resource.as_deref(), &exe_dir))
}

fn bundled_runtime_envs_from(resource: Option<&Path>, exe_dir: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<_> = resource
        .into_iter()
        .flat_map(resource_roots)
        .map(|root| root.join("python-runtime").join("runtime-envs"))
        .collect();
    candidates.push(exe_dir.join("python-runtime").join("runtime-envs"));
    candidates
        .into_iter()
        .find(|path| path.is_dir())
}

/// In development, keep downloaded CUDA/CPU/ROCm runtimes inside the project's
/// `.venv` rather than `src-tauri/target/...`, so `cargo clean` (or wiping build
/// artifacts) can never delete an installed runtime again.
///
/// `CARGO_MANIFEST_DIR` is resolved at compile time to `<checkout>/src-tauri`,
/// so its parent is the checkout root on whichever machine builds the app.
/// A fresh clone therefore gets the same layout without any launcher script.
fn development_runtime_envs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|root| root.join(".venv").join("python").join("runtime-envs"))
        .unwrap_or_else(|| PathBuf::from(".venv/python/runtime-envs"))
}

/// Directory chosen by the user in Settings → Data directories. Stored in the
/// same `app-settings` JSON as `modelDir`, so the Rust process and the Python
/// worker always resolve the runtime root from one source of truth.
fn runtime_envs_dir_from_settings(value: &Value) -> Option<PathBuf> {
    let raw = value.get("runtimeEnvsDir")?.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    let path = PathBuf::from(raw);
    path.is_absolute().then_some(path)
}

fn configured_runtime_envs_dir(app: &AppHandle) -> Option<PathBuf> {
    runtime_envs_dir_from_settings(&read_app_store(app, "app-settings").ok()?)
}

pub fn runtime_envs_dir(app: &AppHandle) -> AppResult<PathBuf> {
    // 1. Explicit override (portable builds, CI, custom layouts).
    if let Some(dir) = std::env::var_os(RUNTIME_ENVS_OVERRIDE_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        return Ok(dir);
    }
    // 2. User-configured directory (winner over built-in defaults).
    if let Some(dir) = configured_runtime_envs_dir(app) {
        return Ok(dir);
    }
    // 3. Keep SOUNDFLOW_DATA_ROOT semantics: a custom data root owns runtimes too.
    if let Some(root) = std::env::var_os(DATA_ROOT_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        return Ok(root.join("runtime-envs"));
    }
    // 4. Development builds: project-local, persistent location.
    if is_development_executable() {
        return Ok(development_runtime_envs_dir());
    }
    // 5. Packaged builds on Unix keep the data-root layout.
    #[cfg(any(feature = "vst-lab", target_os = "macos", target_os = "linux"))]
    {
        Ok(data_root_dir(app)?.join("runtime-envs"))
    }
    // 6. Windows packages: keep managed environments in an install-local
    //    `.venv`, matching the development layout. Existing installs that
    //    already have environments under python-runtime/ or the data root are
    //    adopted so an upgrade never hides them.
    #[cfg(not(any(feature = "vst-lab", target_os = "macos", target_os = "linux")))]
    {
        let exe_dir = executable_dir()?;
        let default = exe_dir.join(".venv").join("python").join("runtime-envs");
        if default.exists() {
            return Ok(default);
        }
        let legacy_bundled = exe_dir.join("python-runtime").join("runtime-envs");
        if legacy_bundled.is_dir() {
            return Ok(legacy_bundled);
        }
        let legacy_data = data_root_dir(app)?.join("runtime-envs");
        if legacy_data.is_dir() {
            return Ok(legacy_data);
        }
        Ok(default)
    }
}

pub fn active_runtime_file(app: &AppHandle) -> AppResult<PathBuf> {
    Ok(runtime_envs_dir(app)?.join("active-runtime.json"))
}

/// pymss's registry of imported ("custom") models.
///
/// Kept under `settings/` rather than with the models themselves: the registry stores absolute
/// paths to files that usually live outside the model directory, so it is app state, not model
/// data, and must not be caught up in a model-directory migration.
pub fn user_models_file(app: &AppHandle) -> AppResult<PathBuf> {
    Ok(settings_dir(app)?.join("user_models.json"))
}

pub fn ensure_app_directories(app: &AppHandle) -> AppResult<()> {
    for dir in [
        data_root_dir(app)?,
        settings_dir(app)?,
        models_dir(app)?,
        outputs_dir(app)?,
        editor_projects_dir(app)?,
        logs_dir(app)?,
        temp_dir(app)?,
    ] {
        std::fs::create_dir_all(dir)?;
    }
    for dir in model_cache_dirs(app)? {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::create_dir_all(data_root_dir(app)?.join("plugins"))?;
    Ok(())
}

pub fn app_paths_payload(app: &AppHandle) -> AppResult<AppPathsPayload> {
    ensure_app_directories(app)?;
    Ok(AppPathsPayload {
        data_root: display_root(&data_root_dir(app)?),
        settings_dir: display_root(&settings_dir(app)?),
        models_dir: display_root(&models_dir(app)?),
        outputs_dir: display_root(&outputs_dir(app)?),
        editor_projects_dir: display_root(&editor_projects_dir(app)?),
        logs_dir: display_root(&logs_dir(app)?),
        temp_dir: display_root(&temp_dir(app)?),
    })
}

fn store_file_name(name: &str) -> AppResult<&'static str> {
    match name {
        "app-settings" => Ok("app.json"),
        "task-history" => Ok("tasks.json"),
        "model-state" => Ok("model-cache.json"),
        "editor-ui" => Ok("editor-ui.json"),
        "audio-tools" => Ok("audio-tools.json"),
        "plugin-tools" => Ok("plugin-tools.json"),
        "plugin-source" => Ok("plugin-source.json"),
        "workflow-state" => Ok("workflows.json"),
        "separate-state" => Ok("separate.json"),
        "lyrics-form" => Ok("lyrics-form.json"),
        "update-state" => Ok("update.json"),
        _ => Err(AppError::Worker(format!("unknown app store: {name}"))),
    }
}

pub fn app_store_path(app: &AppHandle, name: &str) -> AppResult<PathBuf> {
    Ok(settings_dir(app)?.join(store_file_name(name)?))
}

pub fn read_app_store(app: &AppHandle, name: &str) -> AppResult<Value> {
    ensure_app_directories(app)?;
    let path = app_store_path(app, name)?;
    if !path.is_file() {
        return Ok(Value::Null);
    }
    let content = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content)?)
}

pub fn write_app_store(app: &AppHandle, name: &str, data: &Value) -> AppResult<()> {
    ensure_app_directories(app)?;
    let path = app_store_path(app, name)?;
    write_json_file(&path, data)
}

fn plugin_tool_key(plugin_name: &str, view_id: &str) -> AppResult<String> {
    let valid = |value: &str, limit: usize| {
        !value.is_empty() && value.len() <= limit
            && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    };
    if !valid(plugin_name, 100) || !valid(view_id, 200) {
        return Err(AppError::Worker("Invalid plugin tool settings namespace".into()));
    }
    Ok(format!("{plugin_name}:{view_id}"))
}

pub fn load_plugin_tool_settings(app: &AppHandle, plugin_name: &str, view_id: &str) -> AppResult<Value> {
    let key = plugin_tool_key(plugin_name, view_id)?;
    let state = read_app_store(app, "plugin-tools")?;
    Ok(state.get(&key).cloned().unwrap_or_else(|| serde_json::json!({})))
}

pub fn save_plugin_tool_settings(app: &AppHandle, plugin_name: &str, view_id: &str, data: &Value) -> AppResult<()> {
    ensure_app_directories(app)?;
    save_plugin_tool_settings_file(&app_store_path(app, "plugin-tools")?, plugin_name, view_id, data)
}

fn save_plugin_tool_settings_file(path: &Path, plugin_name: &str, view_id: &str, data: &Value) -> AppResult<()> {
    let key = plugin_tool_key(plugin_name, view_id)?;
    if !data.is_object() || serde_json::to_vec(data)?.len() > 2 * 1024 * 1024 {
        return Err(AppError::Worker("Plugin tool settings must be an object of at most 2 MiB".into()));
    }
    // Keep the read/merge/write under the same lock used for atomic JSON publication.
    let _guard = JSON_WRITE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut state = if path.is_file() {
        serde_json::from_str::<Value>(&std::fs::read_to_string(path)?)?
    } else {
        serde_json::json!({})
    };
    let object = state.as_object_mut().ok_or_else(|| AppError::Worker("Invalid plugin tool settings store".into()))?;
    object.insert(key, data.clone());
    write_json_file_locked(path, &state)
}

pub(crate) fn write_json_file(path: &Path, data: &Value) -> AppResult<()> {
    let _write_guard = JSON_WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    write_json_file_locked(path, data)
}

pub(crate) fn write_editor_project_file(path: &Path, data: &Value) -> AppResult<()> {
    let _write_guard = JSON_WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    backup_pre_plugin_project(path, data).map_err(|error| {
        AppError::Worker(format!("Could not preserve the original editor project at {}: {error}", path.display()))
    })?;
    write_json_file_locked(path, data)
}

fn backup_pre_plugin_project(path: &Path, data: &Value) -> AppResult<()> {
    let backup = path.with_file_name("project.pre-plugins.json");
    match std::fs::symlink_metadata(&backup) {
        Ok(metadata) if metadata.is_file() => return Ok(()),
        Ok(_) => return Err(AppError::Worker(format!("The project backup is not a regular file: {}", backup.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let content = match std::fs::read(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let previous: Value = serde_json::from_slice(&content)?;
    let has_legacy_effects = previous.get("tracks").and_then(Value::as_array)
        .is_some_and(|tracks| tracks.iter().any(|track| track.get("effects").is_some_and(Value::is_object)));
    if previous.get("version").and_then(Value::as_u64).unwrap_or(0) >= 4
        || (!has_legacy_effects && data.get("version").and_then(Value::as_u64).unwrap_or(0) < 4)
    {
        return Ok(());
    }

    // Publish a fully synced copy before replacing the project. A second writer
    // cannot replace the first upgrade snapshot, including across processes.
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap_or_else(|| Path::new(".")))?;
    file.write_all(&content)?;
    file.as_file().sync_all()?;
    match file.persist_noclobber(&backup) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            if std::fs::symlink_metadata(&backup)?.is_file() {
                Ok(())
            } else {
                Err(error.error.into())
            }
        }
        Err(error) => Err(error.error.into()),
    }
}

fn write_json_file_locked(path: &Path, data: &Value) -> AppResult<()> {
    write_json_file_with_replace(path, data, replace_file)
}

fn write_json_file_with_replace(
    path: &Path,
    data: &Value,
    replace: impl FnOnce(&Path, &Path) -> AppResult<()>,
) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_vec_pretty(data)?;
    let sequence = JSON_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("store.json");
    let temporary = path.with_file_name(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        sequence,
    ));
    let result = (|| -> AppResult<()> {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&content)?;
        file.sync_all()?;
        drop(file);
        replace(&temporary, path)
    })();
    #[cfg(windows)]
    if let Err(AppError::Io(error)) = &result {
        // ReplaceFileW can remove/rename the old file before reporting these errors.
        // The complete, synced replacement is then the only copy of the latest save.
        if matches!(error.raw_os_error(), Some(1176 | 1177)) {
            return Err(AppError::Worker(format!(
                "Failed to save {}: {error}. Recovery file retained at {}",
                path.display(),
                temporary.display(),
            )));
        }
    }
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

pub fn mutate_workflow_store(app: &AppHandle, mutation: &Value) -> AppResult<Value> {
    ensure_app_directories(app)?;
    let path = app_store_path(app, "workflow-state")?;
    let _write_guard = JSON_WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let state = if path.is_file() {
        let content = std::fs::read_to_string(&path)?;
        serde_json::from_str::<Value>(&content)?
    } else {
        serde_json::json!({ "workflows": [], "selectedWorkflowId": "" })
    };
    let response = apply_workflow_mutation(state, mutation)?;
    if response.get("conflict").is_none() {
        let next_state = response
            .get("state")
            .ok_or_else(|| AppError::Worker("workflow mutation returned no state".into()))?;
        write_json_file_locked(&path, next_state)?;
    }
    Ok(response)
}

fn missing_workflow_id(item: &Value) -> bool {
    item.is_object()
        && item
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
}

fn legacy_workflow_index(workflow_list: &[Value], mutation: &Value) -> Option<usize> {
    let legacy_index = mutation
        .get("legacyIndex")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok());
    let legacy_entry = mutation.get("legacyEntry");
    let is_match = |item: &Value| {
        missing_workflow_id(item)
            && legacy_entry
                .map(|expected| expected == item)
                .unwrap_or(true)
    };
    legacy_index
        .filter(|index| workflow_list.get(*index).is_some_and(&is_match))
        .or_else(|| legacy_entry.and_then(|_| workflow_list.iter().position(is_match)))
}

fn apply_workflow_mutation(mut state: Value, mutation: &Value) -> AppResult<Value> {
    if !state.is_object() {
        state = serde_json::json!({ "workflows": [], "selectedWorkflowId": "" });
    }
    let state_object = state
        .as_object_mut()
        .ok_or_else(|| AppError::Worker("workflow store must be an object".into()))?;
    let selected_before = state_object
        .get("selectedWorkflowId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let workflows = state_object
        .entry("workflows")
        .or_insert_with(|| Value::Array(Vec::new()));
    if !workflows.is_array() {
        *workflows = Value::Array(Vec::new());
    }
    let workflow_list = workflows
        .as_array_mut()
        .ok_or_else(|| AppError::Worker("workflow list must be an array".into()))?;
    let action = mutation.get("action").and_then(Value::as_str).unwrap_or("");
    let mut conflict = None;

    match action {
        "upsert" => {
            let mut entry = mutation
                .get("entry")
                .cloned()
                .filter(Value::is_object)
                .ok_or_else(|| AppError::Worker("workflow entry is required".into()))?;
            let workflow_id = entry
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| AppError::Worker("workflow id is required".into()))?
                .to_string();
            let mut existing_index = workflow_list.iter().position(|item| {
                item.get("id").and_then(Value::as_str) == Some(workflow_id.as_str())
            });
            let has_legacy_reference =
                mutation.get("legacyIndex").is_some() || mutation.get("legacyEntry").is_some();
            let mut legacy_target_mismatch = false;
            if existing_index.is_none() {
                existing_index = legacy_workflow_index(workflow_list, mutation);
                if existing_index.is_none() && has_legacy_reference {
                    legacy_target_mismatch = true;
                }
            }
            let actual_updated_at = existing_index
                .and_then(|index| workflow_list[index].get("updatedAt"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let expected_updated_at = mutation.get("expectedUpdatedAt").and_then(Value::as_u64);
            let force = mutation
                .get("force")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if !force
                && (legacy_target_mismatch
                    || (expected_updated_at.is_some()
                        && expected_updated_at.unwrap_or(0) != actual_updated_at))
            {
                conflict = Some(serde_json::json!({
                    "workflowId": workflow_id,
                    "expectedUpdatedAt": expected_updated_at.unwrap_or(0),
                    "actualUpdatedAt": actual_updated_at,
                }));
            } else {
                if let Some(index) = existing_index {
                    let created_at = workflow_list[index]
                        .get("createdAt")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    if created_at > 0 {
                        entry["createdAt"] = Value::from(created_at);
                    }
                }
                let requested_updated_at =
                    entry.get("updatedAt").and_then(Value::as_u64).unwrap_or(0);
                entry["updatedAt"] =
                    Value::from(requested_updated_at.max(actual_updated_at.saturating_add(1)));
                if let Some(index) = existing_index {
                    workflow_list[index] = entry;
                } else {
                    workflow_list.push(entry);
                }
                workflow_list.sort_by(|left, right| {
                    let left_value = left.get("updatedAt").and_then(Value::as_u64).unwrap_or(0);
                    let right_value = right.get("updatedAt").and_then(Value::as_u64).unwrap_or(0);
                    right_value.cmp(&left_value)
                });
                state_object.insert("selectedWorkflowId".into(), Value::String(workflow_id));
            }
        }
        "delete" => {
            let workflow_id = mutation
                .get("workflowId")
                .and_then(Value::as_str)
                .unwrap_or("");
            let index = workflow_list
                .iter()
                .position(|item| item.get("id").and_then(Value::as_str) == Some(workflow_id))
                .or_else(|| legacy_workflow_index(workflow_list, mutation));
            let actual_updated_at = index
                .and_then(|index| workflow_list[index].get("updatedAt"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let expected_updated_at = mutation.get("expectedUpdatedAt").and_then(Value::as_u64);
            let has_legacy_reference =
                mutation.get("legacyIndex").is_some() || mutation.get("legacyEntry").is_some();
            if (index.is_none() && has_legacy_reference)
                || (expected_updated_at.is_some()
                    && expected_updated_at.unwrap_or(0) != actual_updated_at)
            {
                conflict = Some(serde_json::json!({
                    "workflowId": workflow_id,
                    "expectedUpdatedAt": expected_updated_at.unwrap_or(0),
                    "actualUpdatedAt": actual_updated_at,
                }));
            } else {
                let removed_selected = index
                    .map(|index| workflow_list.remove(index))
                    .and_then(|item| {
                        item.get("id")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .is_some_and(|removed_id| removed_id == selected_before);
                if selected_before == workflow_id || removed_selected {
                    let replacement = workflow_list
                        .first()
                        .and_then(|item| item.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    state_object.insert("selectedWorkflowId".into(), Value::String(replacement));
                }
            }
        }
        "select" => {
            let workflow_id = mutation
                .get("workflowId")
                .and_then(Value::as_str)
                .unwrap_or("");
            let existing_index = workflow_list
                .iter()
                .position(|item| item.get("id").and_then(Value::as_str) == Some(workflow_id));
            let legacy_index = existing_index
                .is_none()
                .then(|| legacy_workflow_index(workflow_list, mutation))
                .flatten();
            let has_legacy_reference =
                mutation.get("legacyIndex").is_some() || mutation.get("legacyEntry").is_some();
            if !workflow_id.is_empty()
                && existing_index.is_none()
                && legacy_index.is_none()
                && has_legacy_reference
            {
                conflict = Some(serde_json::json!({
                    "workflowId": workflow_id,
                    "expectedUpdatedAt": 0,
                    "actualUpdatedAt": 0,
                }));
            } else {
                if let Some(index) = legacy_index {
                    workflow_list[index]["id"] = Value::String(workflow_id.to_string());
                }
                if workflow_id.is_empty() || existing_index.is_some() || legacy_index.is_some() {
                    state_object.insert(
                        "selectedWorkflowId".into(),
                        Value::String(workflow_id.to_string()),
                    );
                }
            }
        }
        _ => {
            return Err(AppError::Worker(format!(
                "unknown workflow mutation: {action}"
            )))
        }
    }

    if let Some(conflict) = conflict {
        return Ok(serde_json::json!({ "state": state, "conflict": conflict }));
    }
    Ok(serde_json::json!({ "state": state }))
}

#[cfg(windows)]
fn replace_file(temporary: &Path, destination: &Path) -> AppResult<()> {
    if !destination.exists() {
        std::fs::rename(temporary, destination)?;
        return Ok(());
    }
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let temporary = temporary
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    retry_file_replacement(|| {
        let replaced = unsafe {
            ReplaceFileW(
                destination.as_ptr(),
                temporary.as_ptr(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if replaced != 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    })?;
    Ok(())
}

#[cfg(windows)]
fn retry_file_replacement(mut replace: impl FnMut() -> std::io::Result<()>) -> std::io::Result<()> {
    for attempt in 0..6 {
        match replace() {
            Ok(()) => return Ok(()),
            Err(error) => {
                // 1175 (ERROR_UNABLE_TO_REMOVE_REPLACED) leaves both names intact, so it
                // is safe to retry just like sharing violations. Never retry 1176/1177:
                // those indicate a partially completed replacement.
                let retryable = matches!(error.raw_os_error(), Some(5 | 32 | 33 | 1175));
                if !retryable || attempt == 5 {
                    return Err(error);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(40 * (attempt + 1) as u64));
    }
    unreachable!("ReplaceFileW retry loop must return")
}

#[cfg(not(windows))]
fn replace_file(temporary: &Path, destination: &Path) -> AppResult<()> {
    std::fs::rename(temporary, destination)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        apply_workflow_mutation, data_root_has_user_models, display_root, migrate_data_root_paths,
        model_cache_paths, move_data_root_entry, path_from_data_root_pointer, resolve_data_root,
        runtime_envs_dir_from_settings, store_file_name, write_json_file, JSON_WRITE_SEQUENCE,
    };
    use serde_json::{json, Value};
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;

    fn path(name: &str) -> PathBuf {
        PathBuf::from(name)
    }

    #[test]
    fn plugin_tool_settings_merge_concurrent_views_without_lost_updates() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("plugin-tools.json");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let threads = (0..8).map(|index| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                super::save_plugin_tool_settings_file(&path, "convert", &format!("view-{index}"), &json!({"rate": index})).unwrap();
            })
        }).collect::<Vec<_>>();
        for thread in threads { thread.join().unwrap(); }
        let state: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(state.as_object().unwrap().len(), 8);
        for index in 0..8 { assert_eq!(state[format!("convert:view-{index}")]["rate"], index); }
    }

    #[test]
    fn plugin_tool_settings_reject_invalid_names_and_preserve_corrupt_store() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("plugin-tools.json");
        assert!(super::save_plugin_tool_settings_file(&path, "../outside", "tool", &json!({})).is_err());
        assert!(super::save_plugin_tool_settings_file(&path, "convert", "tool", &json!([])).is_err());
        assert!(!path.exists());
        std::fs::write(&path, "invalid JSON").unwrap();
        assert!(super::save_plugin_tool_settings_file(&path, "convert", "tool", &json!({})).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "invalid JSON");
    }

    #[test]
    fn workflow_mutations_merge_different_editor_saves_into_the_latest_state() {
        let initial = json!({
            "workflows": [
                { "id": "first", "name": "First", "createdAt": 1, "updatedAt": 10 },
                { "id": "second", "name": "Second", "createdAt": 2, "updatedAt": 10 },
            ],
            "selectedWorkflowId": "first",
        });
        let first = apply_workflow_mutation(
            initial,
            &json!({
                "action": "upsert",
                "entry": { "id": "first", "name": "First changed", "createdAt": 1, "updatedAt": 20 },
                "expectedUpdatedAt": 10,
            }),
        )
        .unwrap();
        let second = apply_workflow_mutation(
            first["state"].clone(),
            &json!({
                "action": "upsert",
                "entry": { "id": "second", "name": "Second changed", "createdAt": 2, "updatedAt": 21 },
                "expectedUpdatedAt": 10,
            }),
        )
        .unwrap();
        let workflows = second["state"]["workflows"].as_array().unwrap();
        assert_eq!(workflows.len(), 2);
        assert!(workflows
            .iter()
            .any(|item| item["id"] == "first" && item["name"] == "First changed"));
        assert!(workflows
            .iter()
            .any(|item| item["id"] == "second" && item["name"] == "Second changed"));
    }

    #[test]
    fn workflow_mutation_reports_a_real_persisted_revision_conflict() {
        let state = json!({
            "workflows": [{ "id": "first", "name": "Remote", "createdAt": 1, "updatedAt": 20 }],
            "selectedWorkflowId": "first",
        });
        let response = apply_workflow_mutation(
            state.clone(),
            &json!({
                "action": "upsert",
                "entry": { "id": "first", "name": "Stale", "createdAt": 1, "updatedAt": 21 },
                "expectedUpdatedAt": 10,
            }),
        )
        .unwrap();
        assert_eq!(response["conflict"]["actualUpdatedAt"], 20);
        assert_eq!(response["state"], state);
    }

    #[test]
    fn workflow_mutation_can_clear_the_selected_workflow() {
        let state = json!({
            "workflows": [{ "id": "first", "name": "First", "createdAt": 1, "updatedAt": 10 }],
            "selectedWorkflowId": "first",
        });
        let response = apply_workflow_mutation(
            state,
            &json!({ "action": "select", "workflowId": "" }),
        )
        .unwrap();
        assert_eq!(response["state"]["selectedWorkflowId"], "");
        assert_eq!(response["state"]["workflows"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn workflow_mutation_migrates_a_legacy_entry_without_creating_a_duplicate() {
        let state = json!({
            "workflows": [{ "name": "Legacy", "definition": {}, "createdAt": 1 }],
            "selectedWorkflowId": "",
        });
        let response = apply_workflow_mutation(
            state,
            &json!({
                "action": "upsert",
                "entry": {
                    "id": "workflow_generated",
                    "name": "Migrated",
                    "definition": {},
                    "createdAt": 2,
                    "updatedAt": 10,
                },
                "expectedUpdatedAt": 0,
                "legacyIndex": 0,
            }),
        )
        .unwrap();
        let workflows = response["state"]["workflows"].as_array().unwrap();
        assert_eq!(workflows.len(), 1);
        assert_eq!(workflows[0]["id"], "workflow_generated");
        assert_eq!(workflows[0]["name"], "Migrated");
    }

    #[test]
    fn stale_legacy_index_conflicts_instead_of_creating_a_duplicate() {
        let state = json!({
            "workflows": [{
                "id": "workflow_saved_elsewhere",
                "name": "Remote",
                "definition": {},
                "createdAt": 1,
                "updatedAt": 20,
            }],
            "selectedWorkflowId": "workflow_saved_elsewhere",
        });
        let response = apply_workflow_mutation(
            state.clone(),
            &json!({
                "action": "upsert",
                "entry": {
                    "id": "workflow_generated_locally",
                    "name": "Local",
                    "definition": {},
                    "createdAt": 2,
                    "updatedAt": 10,
                },
                "expectedUpdatedAt": 0,
                "legacyIndex": 0,
            }),
        )
        .unwrap();
        assert!(response.get("conflict").is_some());
        assert_eq!(response["state"], state);
    }

    #[test]
    fn legacy_entry_identity_survives_reordering_before_migration() {
        let target = json!({ "name": "Target legacy", "definition": {}, "updatedAt": 5 });
        let state = json!({
            "workflows": [
                { "name": "Other legacy", "definition": {}, "updatedAt": 7 },
                target.clone(),
            ],
            "selectedWorkflowId": "",
        });
        let response = apply_workflow_mutation(
            state,
            &json!({
                "action": "upsert",
                "entry": {
                    "id": "workflow_target",
                    "name": "Migrated target",
                    "definition": {},
                    "createdAt": 2,
                    "updatedAt": 10,
                },
                "expectedUpdatedAt": 5,
                "legacyIndex": 0,
                "legacyEntry": target,
            }),
        )
        .unwrap();
        let workflows = response["state"]["workflows"].as_array().unwrap();
        assert_eq!(workflows.len(), 2);
        assert!(workflows
            .iter()
            .any(|item| item["id"] == "workflow_target" && item["name"] == "Migrated target"));
        assert!(workflows
            .iter()
            .any(|item| item["name"] == "Other legacy" && item.get("id").is_none()));
    }

    #[test]
    fn legacy_entries_support_select_and_delete_mutations() {
        let first = json!({ "name": "First legacy", "definition": {} });
        let second = json!({ "name": "Second legacy", "definition": {} });
        let state = json!({
            "workflows": [first.clone(), second.clone()],
            "selectedWorkflowId": "",
        });
        let selected = apply_workflow_mutation(
            state,
            &json!({
                "action": "select",
                "workflowId": "workflow_second",
                "legacyIndex": 1,
                "legacyEntry": second,
            }),
        )
        .unwrap();
        assert_eq!(selected["state"]["selectedWorkflowId"], "workflow_second");
        assert_eq!(selected["state"]["workflows"][1]["id"], "workflow_second");

        let deleted = apply_workflow_mutation(
            selected["state"].clone(),
            &json!({
                "action": "delete",
                "workflowId": "workflow_first",
                "legacyIndex": 0,
                "legacyEntry": first,
            }),
        )
        .unwrap();
        let workflows = deleted["state"]["workflows"].as_array().unwrap();
        assert_eq!(workflows.len(), 1);
        assert_eq!(workflows[0]["id"], "workflow_second");
    }

    #[test]
    fn stale_legacy_delete_and_select_report_conflicts() {
        let legacy = json!({ "name": "Legacy", "definition": {} });
        let migrated = json!({
            "id": "workflow_remote",
            "name": "Legacy",
            "definition": {},
            "updatedAt": 20,
        });
        let state = json!({
            "workflows": [migrated],
            "selectedWorkflowId": "workflow_remote",
        });
        for action in ["delete", "select"] {
            let response = apply_workflow_mutation(
                state.clone(),
                &json!({
                    "action": action,
                    "workflowId": "workflow_stale",
                    "legacyIndex": 0,
                    "legacyEntry": legacy,
                }),
            )
            .unwrap();
            assert!(response.get("conflict").is_some(), "{action}");
            assert_eq!(response["state"], state, "{action}");
        }
    }

    #[test]
    fn stale_delete_does_not_remove_a_newer_workflow_revision() {
        let state = json!({
            "workflows": [{
                "id": "workflow",
                "name": "Updated elsewhere",
                "definition": {},
                "createdAt": 1,
                "updatedAt": 20,
            }],
            "selectedWorkflowId": "workflow",
        });
        let response = apply_workflow_mutation(
            state.clone(),
            &json!({
                "action": "delete",
                "workflowId": "workflow",
                "expectedUpdatedAt": 10,
            }),
        )
        .unwrap();
        assert_eq!(response["conflict"]["actualUpdatedAt"], 20);
        assert_eq!(response["state"], state);
    }

    struct ResourceFixture(PathBuf);

    impl ResourceFixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "pymss-resource-layout-{}-{}",
                std::process::id(),
                JSON_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed),
            ));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for ResourceFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn runtime_layouts_keep_resource_up_resources_and_exe_priority() {
        let fixture = ResourceFixture::new();
        let resource = fixture.0.join("Pymss.app/Contents/Resources");
        let exe = fixture.0.join("Pymss.app/Contents/MacOS");
        let candidates = [
            resource.join("python-runtime"),
            resource.join("_up_/python-runtime"),
            resource.join("resources/python-runtime"),
            exe.join("python-runtime"),
        ];
        for candidate in candidates.iter().rev() {
            std::fs::create_dir_all(candidate).unwrap();
            assert_eq!(super::runtime_root_from(Some(&resource), &exe), *candidate);
            let envs = candidate.join("runtime-envs");
            std::fs::create_dir_all(&envs).unwrap();
            assert_eq!(super::bundled_runtime_envs_from(Some(&resource), &exe), Some(envs));
        }
    }

    #[test]
    fn bundled_env_discovery_does_not_assume_the_first_runtime_root_contains_envs() {
        let fixture = ResourceFixture::new();
        let resource = fixture.0.join("resources");
        let exe = fixture.0.join("portable");
        std::fs::create_dir_all(resource.join("python-runtime")).unwrap();
        let envs = exe.join("python-runtime/runtime-envs");
        std::fs::create_dir_all(&envs).unwrap();
        assert_eq!(super::runtime_root_from(Some(&resource), &exe), resource.join("python-runtime"));
        assert_eq!(super::bundled_runtime_envs_from(Some(&resource), &exe), Some(envs));
    }

    #[test]
    fn missing_runtime_root_has_a_fixed_fallback_but_bundled_envs_are_optional() {
        let fixture = ResourceFixture::new();
        let resource = fixture.0.join("resources");
        let exe = fixture.0.join("installer");
        for resource in [Some(resource.as_path()), None] {
            assert_eq!(super::runtime_root_from(resource, &exe), exe.join("python-runtime"));
            assert_eq!(super::bundled_runtime_envs_from(resource, &exe), None);
        }
        assert!(!exe.exists());
    }

    #[test]
    fn runtime_directory_selection_ignores_files_and_accepts_missing_resource_api() {
        let fixture = ResourceFixture::new();
        let resource = fixture.0.join("resources");
        let exe = fixture.0.join("portable");
        std::fs::create_dir_all(&resource).unwrap();
        std::fs::write(resource.join("python-runtime"), b"not a directory").unwrap();
        let envs = exe.join("python-runtime/runtime-envs");
        std::fs::create_dir_all(&envs).unwrap();
        for resource in [Some(resource.as_path()), None, Some(exe.as_path())] {
            assert_eq!(super::runtime_root_from(resource, &exe), exe.join("python-runtime"));
            assert_eq!(super::bundled_runtime_envs_from(resource, &exe), Some(envs.clone()));
        }
    }

    #[test]
    fn development_detection_does_not_treat_packaged_bundle_paths_as_a_checkout() {
        let target = path("workspace/src-tauri/target");
        for relative in ["debug/pymss.exe", "release/pymss.exe", "aarch64-apple-darwin/release/pymss"] {
            assert!(super::is_cargo_profile_executable(&target.join(relative), &target));
        }
        for relative in [
            "release/bundle/Pymss.app/Contents/MacOS/pymss",
            "release/bundle/portable/pymss.exe",
            "debug/deps/pymss.exe",
            "unrelated/pymss.exe",
        ] {
            assert!(!super::is_cargo_profile_executable(&target.join(relative), &target));
        }
    }

    #[test]
    fn separate_state_has_a_store_file() {
        assert_eq!(store_file_name("separate-state").unwrap(), "separate.json");
    }

    #[test]
    fn audio_tools_has_a_store_file() {
        assert_eq!(store_file_name("audio-tools").unwrap(), "audio-tools.json");
    }

    #[test]
    fn update_state_has_a_store_file() {
        assert_eq!(store_file_name("update-state").unwrap(), "update.json");
    }

    #[test]
    fn json_writes_replace_existing_content_without_leaving_temporary_files() {
        let root = std::env::temp_dir().join(format!(
            "pymss-storage-test-{}-{}",
            std::process::id(),
            JSON_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        ));
        let path = root.join("settings.json");
        write_json_file(&path, &json!({ "value": 1 })).unwrap();
        write_json_file(&path, &json!({ "value": 2 })).unwrap();

        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored, json!({ "value": 2 }));
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn editor_upgrade_backup_preserves_original_bytes_and_is_never_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("project.json");
        let backup = root.path().join("project.pre-plugins.json");
        let original = b"{\n  \"id\": \"old\", \"version\": 3, \"tracks\": [{\"effects\": {\"delay\": 0.3}}], \"future\": [1, 2]\n}\n";
        std::fs::write(&path, original).unwrap();
        let mut next: serde_json::Value = serde_json::from_slice(original).unwrap();
        // Autosave/enrichment can run before the user adds their first plugin.
        next["name"] = json!("Renamed");
        super::write_editor_project_file(&path, &next).unwrap();
        assert_eq!(std::fs::read(&backup).unwrap(), original);
        next["version"] = json!(4);
        next["pluginChain"] = json!([{ "id": "slot", "state": "sha256:retained" }]);
        super::write_editor_project_file(&path, &next).unwrap();
        super::write_editor_project_file(&path, &next).unwrap();
        assert_eq!(std::fs::read(&backup).unwrap(), original);
        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved, next);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
    }

    #[test]
    fn editor_upgrade_backup_covers_v3_without_effects_but_not_new_or_existing_v4_projects() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("project.json");
        let backup = root.path().join("project.pre-plugins.json");
        let old = json!({ "id": "project", "version": 3, "tracks": [] });
        super::write_editor_project_file(&path, &old).unwrap();
        super::write_editor_project_file(&path, &old).unwrap();
        assert!(!backup.exists());
        let upgraded = json!({ "id": "project", "version": 4, "masterLimiter": { "enabled": true } });
        super::write_editor_project_file(&path, &upgraded).unwrap();
        let stored: serde_json::Value = serde_json::from_slice(&std::fs::read(&backup).unwrap()).unwrap();
        assert_eq!(stored, old);
        let new_root = tempfile::tempdir().unwrap();
        let new_path = new_root.path().join("project.json");
        super::write_editor_project_file(&new_path, &upgraded).unwrap();
        super::write_editor_project_file(&new_path, &upgraded).unwrap();
        assert!(!new_root.path().join("project.pre-plugins.json").exists());
    }

    #[test]
    fn editor_upgrade_backup_failure_keeps_original_project_until_retry() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("project.json");
        let backup = root.path().join("project.pre-plugins.json");
        let old = b"{\"id\":\"project\",\"version\":3}";
        std::fs::write(&path, old).unwrap();
        std::fs::create_dir(&backup).unwrap();
        let next = json!({ "id": "project", "version": 4 });
        let error = super::write_editor_project_file(&path, &next).unwrap_err();
        assert!(error.to_string().contains("Could not preserve the original editor project"));
        assert_eq!(std::fs::read(&path).unwrap(), old);
        std::fs::remove_dir(&backup).unwrap();
        super::write_editor_project_file(&path, &next).unwrap();
        assert_eq!(std::fs::read(&backup).unwrap(), old);
    }

    #[test]
    fn editor_upgrade_backup_serializes_concurrent_first_saves() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("project.json");
        let old = b"{\"id\":\"project\",\"version\":3}";
        std::fs::write(&path, old).unwrap();
        std::thread::scope(|scope| {
            for revision in 1..=4 {
                let path = &path;
                scope.spawn(move || super::write_editor_project_file(path, &json!({
                    "id": "project", "version": 4, "revision": revision
                })).unwrap());
            }
        });
        assert_eq!(std::fs::read(root.path().join("project.pre-plugins.json")).unwrap(), old);
        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["version"], 4);
        assert!(saved["revision"].as_u64().is_some_and(|revision| (1..=4).contains(&revision)));
    }

    #[cfg(windows)]
    #[test]
    fn json_save_retries_unable_to_remove_replaced_without_touching_the_old_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("project.json");
        let old = br#"{"revision":1}"#;
        std::fs::write(&path, old).unwrap();
        let mut attempts = 0;
        super::write_json_file_with_replace(&path, &json!({ "revision": 2 }), |temp, dest| {
            super::retry_file_replacement(|| {
                attempts += 1;
                assert_eq!(std::fs::read(dest).unwrap(), old);
                let pending: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(temp).unwrap()).unwrap();
                assert_eq!(pending, json!({ "revision": 2 }));
                if attempts < 3 {
                    Err(std::io::Error::from_raw_os_error(1175))
                } else {
                    std::fs::rename(temp, dest)
                }
            })?;
            Ok(())
        })
        .unwrap();
        assert_eq!(attempts, 3);
        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored, json!({ "revision": 2 }));
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn persistent_replacement_failure_is_bounded_and_keeps_the_previous_save() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("project.json");
        let old = br#"{"revision":1}"#;
        std::fs::write(&path, old).unwrap();
        let mut attempts = 0;
        let error = super::write_json_file_with_replace(&path, &json!({ "revision": 2 }), |_, _| {
            super::retry_file_replacement(|| {
                attempts += 1;
                Err(std::io::Error::from_raw_os_error(1175))
            })?;
            Ok(())
        })
        .unwrap_err();
        assert_eq!(attempts, 6);
        assert!(matches!(error,
            crate::error::AppError::Io(error) if error.raw_os_error() == Some(1175)));
        assert_eq!(std::fs::read(&path).unwrap(), old);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn partial_replacements_are_not_retried_and_keep_a_complete_recovery_file() {
        for code in [1176, 1177] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("project.json");
            std::fs::write(&path, br#"{"revision":1}"#).unwrap();
            let mut recovery = PathBuf::new();
            let mut attempts = 0;
            let error = super::write_json_file_with_replace(
                &path,
                &json!({ "revision": 2 }),
                |temp, dest| {
                    recovery = temp.to_path_buf();
                    super::retry_file_replacement(|| {
                        attempts += 1;
                        if code == 1176 {
                            std::fs::remove_file(dest)?;
                        } else {
                            std::fs::rename(dest, root.path().join("original.json"))?;
                        }
                        Err(std::io::Error::from_raw_os_error(code))
                    })?;
                    Ok(())
                },
            )
            .unwrap_err();
            assert_eq!(attempts, 1);
            assert!(!path.exists());
            assert!(error.to_string().contains(&recovery.display().to_string()));
            let pending: serde_json::Value =
                serde_json::from_slice(&std::fs::read(recovery).unwrap()).unwrap();
            assert_eq!(pending, json!({ "revision": 2 }));
        }
    }

    #[cfg(windows)]
    #[test]
    fn non_transient_file_errors_fail_without_retrying() {
        for code in [2, 87, 112] {
            let mut attempts = 0;
            let error = super::retry_file_replacement(|| {
                attempts += 1;
                Err(std::io::Error::from_raw_os_error(code))
            })
            .unwrap_err();
            assert_eq!(attempts, 1);
            assert_eq!(error.raw_os_error(), Some(code));
        }
    }

    #[cfg(windows)]
    #[test]
    fn json_save_succeeds_after_a_windows_reader_releases_its_handle() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("project.json");
        write_json_file(&path, &json!({ "revision": 1 })).unwrap();
        let reader = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&path)
            .unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            drop(reader);
        });
        let result = write_json_file(&path, &json!({ "revision": 2 }));
        release.join().unwrap();
        result.unwrap();
        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored, json!({ "revision": 2 }));
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }


    #[test]
    fn env_root_has_highest_priority() {
        let root = resolve_data_root(
            Some(path("env-data")),
            path("dev-data"),
            Some(path("portable-data")),
            path("legacy-data"),
            true,
        );

        assert_eq!(root, path("env-data"));
    }

    #[test]
    fn development_uses_project_local_data() {
        let root = resolve_data_root(
            None,
            path("dev-data"),
            Some(path("portable-data")),
            path("legacy-data"),
            true,
        );

        assert_eq!(root, path("dev-data"));
    }

    #[test]
    fn release_portable_uses_portable_data() {
        let root = resolve_data_root(
            None,
            path("dev-data"),
            Some(path("portable-data")),
            path("legacy-data"),
            false,
        );

        assert_eq!(root, path("portable-data"));
    }

    #[test]
    fn release_without_portable_marker_uses_packaged_data() {
        let root = resolve_data_root(None, path("dev-data"), None, path("packaged-data"), false);

        assert_eq!(root, path("packaged-data"));
    }

    #[test]
    fn configured_runtime_dir_must_be_absolute() {
        let absolute = std::env::temp_dir().join("soundflow-runtimes");
        let raw = absolute.to_string_lossy().to_string();
        assert_eq!(
            runtime_envs_dir_from_settings(&serde_json::json!({ "runtimeEnvsDir": raw })),
            Some(absolute)
        );
        assert_eq!(
            runtime_envs_dir_from_settings(&serde_json::json!({ "runtimeEnvsDir": "relative" })),
            None
        );
        assert_eq!(
            runtime_envs_dir_from_settings(&serde_json::json!({ "runtimeEnvsDir": "  " })),
            None
        );
        assert_eq!(runtime_envs_dir_from_settings(&serde_json::json!({})), None);
    }

    #[test]
    fn data_root_pointer_requires_an_absolute_path() {
        let absolute = std::env::temp_dir().join("soundflow-data");
        let raw = absolute.to_string_lossy().to_string();
        assert_eq!(
            path_from_data_root_pointer(&serde_json::json!({ "dataRoot": raw })),
            Some(absolute)
        );
        assert_eq!(
            path_from_data_root_pointer(&serde_json::json!({ "dataRoot": "relative" })),
            None
        );
        assert_eq!(path_from_data_root_pointer(&serde_json::json!({})), None);
    }

    #[test]
    fn data_root_entry_move_preserves_nested_files() {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("_debug_output/tmp/data-root-move-tests");
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let case = base.join(format!("{}-{}", std::process::id(), stamp));
        let source = case.join("source");
        let destination = case.join("destination");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::write(source.join("file.txt"), b"root").unwrap();
        std::fs::write(source.join("nested/child.txt"), b"child").unwrap();

        move_data_root_entry(&source, &destination).unwrap();

        assert_eq!(std::fs::read(destination.join("file.txt")).unwrap(), b"root");
        assert_eq!(std::fs::read(destination.join("nested/child.txt")).unwrap(), b"child");
        assert!(!source.exists());
        std::fs::remove_dir_all(case).ok();
    }

    #[test]
    fn data_root_migration_writes_pointer_and_keeps_old_logs() {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("_debug_output/tmp/data-root-migrate-tests");
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let case = base.join(format!("{}-{}", std::process::id(), stamp));
        let current = case.join("current");
        let target = case.join("target");
        std::fs::create_dir_all(current.join("models")).unwrap();
        std::fs::create_dir_all(current.join("settings")).unwrap();
        std::fs::create_dir_all(current.join("logs")).unwrap();
        std::fs::create_dir_all(current.join("temp")).unwrap();
        std::fs::write(current.join("models/model.bin"), b"model").unwrap();
        let old_settings = serde_json::json!({
            "modelDir": current.join("models").to_string_lossy(),
            "outputDir": current.join("outputs").to_string_lossy(),
        });
        write_json_file(&current.join("settings/app.json"), &old_settings).unwrap();
        std::fs::write(current.join("logs/session.log"), b"log").unwrap();
        std::fs::write(current.join("temp/scratch.bin"), b"tmp").unwrap();
        let pointer = case.join("soundflow-root.json");

        let result = migrate_data_root_paths(
            &current,
            &display_root(&target),
            &pointer,
        )
        .unwrap();

        assert!(target.join("models/model.bin").is_file());
        assert!(target.join("settings/app.json").is_file());
        let migrated_settings: Value =
            serde_json::from_slice(&std::fs::read(target.join("settings/app.json")).unwrap()).unwrap();
        let expected_model = target.join("models").to_string_lossy().replace('/', "\\");
        let expected_output = target.join("outputs").to_string_lossy().replace('/', "\\");
        assert_eq!(migrated_settings.get("modelDir").and_then(Value::as_str), Some(expected_model.as_str()));
        assert_eq!(migrated_settings.get("outputDir").and_then(Value::as_str), Some(expected_output.as_str()));
        assert!(!target.join("logs").exists());
        assert!(!target.join("temp").exists());
        assert!(current.join("logs/session.log").is_file());
        assert!(current.join("temp/scratch.bin").is_file());
        let written: Value = serde_json::from_slice(&std::fs::read(&pointer).unwrap()).unwrap();
        assert_eq!(written.get("dataRoot").and_then(Value::as_str), result.get("dataRoot").and_then(Value::as_str));
        std::fs::remove_dir_all(case).ok();
    }

    #[test]
    fn display_root_uses_native_separators() {
        let raw = "E:/base/.soundflow/models/_cache";
        let separator = char::from(92u8).to_string();
        let shown = display_root(&PathBuf::from(raw));
        #[cfg(windows)]
        assert_eq!(shown, raw.replace('/', &separator));
        #[cfg(not(windows))]
        assert_eq!(shown, raw);
    }

    #[test]
    fn empty_cache_root_does_not_count_as_user_models() {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("_debug_output/tmp/data-root-adoption-tests");
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let case = base.join(format!("{}-{}", std::process::id(), stamp));
        let empty = case.join("empty");
        let real = case.join("real");
        std::fs::create_dir_all(empty.join("models/_cache/hf")).unwrap();
        std::fs::create_dir_all(real.join("models/_tool_models")).unwrap();
        assert!(!data_root_has_user_models(&empty));
        assert!(data_root_has_user_models(&real));
        std::fs::remove_dir_all(case).ok();
    }

    #[test]
    fn model_cache_paths_stay_inside_the_model_root() {
        let models = path("data/models");
        let caches = model_cache_paths(&models);
        assert_eq!(caches[0], models.join("_cache/hf"));
        assert_eq!(caches[1], models.join("_cache/torch"));
        assert_eq!(caches[2], models.join("_cache/modelscope"));
    }

}
