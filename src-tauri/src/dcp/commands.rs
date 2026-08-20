//! Tauri command surface for the profile registry (W5).
//!
//! Ergonomics mirror `import_luts` / `remove_lut` in `lut_processing.rs`: the
//! import validates before accepting (parse first, never copy an invalid file
//! into the managed directory), remove guards against paths outside the
//! managed directory, and Android content URIs flow through the same SAF
//! helpers the LUT importer uses.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::AppState;
use crate::dcp::registry::{
    LookMetadata, PairingState, ProfileEntry, ProfileRegistry, ProfileSource,
    adobe_discovery_roots, discover_profiles, index_path, managed_dcp_dir, managed_looks_dir,
};

fn strip_verbatim(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
}

/// Unique destination within `dir`, appending " (n)" on collision — mirrors
/// `unique_lut_destination`.
fn unique_destination(dir: &Path, stem: &str, extension: &str) -> PathBuf {
    let mut candidate = dir.join(format!("{stem}.{extension}"));
    let mut suffix = 1;
    while candidate.exists() && suffix < 1000 {
        candidate = dir.join(format!("{stem} ({suffix}).{extension}"));
        suffix += 1;
    }
    candidate
}

// ---------------------------------------------------------------------------
// Browser model
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileEntryView {
    pub id: String,
    pub file_path: String,
    pub unique_camera_model: String,
    pub profile_name: String,
    pub source: String,
}

impl From<&ProfileEntry> for ProfileEntryView {
    fn from(e: &ProfileEntry) -> Self {
        ProfileEntryView {
            id: e.id.to_hex(),
            file_path: e.file_path.to_string_lossy().into_owned(),
            unique_camera_model: e.unique_camera_model.clone(),
            profile_name: e.profile_name.clone(),
            source: match e.source {
                ProfileSource::Managed => "managed".to_string(),
                ProfileSource::AdobeAutoDiscovered => "adobe".to_string(),
            },
        }
    }
}

/// DCPs for one body grouped by profile name (same name, different content
/// kept side by side and disambiguated by source dir).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraProfileGroup {
    pub profile_name: String,
    pub entries: Vec<ProfileEntryView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LookBrowserEntry {
    pub uuid: String,
    pub name: String,
    pub group: String,
    pub required_base_profile: String,
    pub file_path: String,
    pub pairing: PairingState,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileBrowserModel {
    /// The resolved camera body for the image, if it could be determined.
    pub camera: Option<String>,
    /// DCP camera profiles matching that body (§2.5, "camera profiles" group).
    pub camera_profiles: Vec<CameraProfileGroup>,
    /// All known Looks, each with its pairing state for this camera.
    pub looks: Vec<LookBrowserEntry>,
}

/// Resolve a `(make, model)` pair into the camera string used for §5.2
/// matching. `None` when neither is available.
fn compose_camera(make: &str, model: &str) -> Option<String> {
    let make = make.trim();
    let model = model.trim();
    match (make.is_empty(), model.is_empty()) {
        (false, false) => Some(format!("{make} {model}")),
        (false, true) => Some(make.to_string()),
        (true, false) => Some(model.to_string()),
        (true, true) => None,
    }
}

/// Best-effort camera extraction from a file's EXIF metadata.
///
/// This is W5's lightweight path for `list_profiles_for_image`. W4 refines it
/// to use `rawler`'s `clean_model` directly from the already-loaded `RawImage`
/// (§4.1); the registry's matching is camera-string-agnostic, so the two agree.
fn camera_for_path(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let exif = crate::exif_processing::read_exif_data_from_bytes(&path.to_string_lossy(), &bytes);
    let make = exif.get("Make").map(|s| s.as_str()).unwrap_or("");
    let model = exif.get("Model").map(|s| s.as_str()).unwrap_or("");
    compose_camera(make, model)
}

fn build_browser_model(registry: &ProfileRegistry, camera: Option<&str>) -> ProfileBrowserModel {
    let mut camera_profiles: Vec<CameraProfileGroup> = Vec::new();
    if let Some(cam) = camera {
        let entries = registry.profiles_for_camera(cam);
        // Group by profile name, preserving order; same-name entries kept.
        let mut groups: Vec<(String, Vec<ProfileEntryView>)> = Vec::new();
        for entry in entries {
            let view = ProfileEntryView::from(&entry);
            if let Some(g) = groups.iter_mut().find(|(n, _)| *n == view.profile_name) {
                g.1.push(view);
            } else {
                groups.push((view.profile_name.clone(), vec![view]));
            }
        }
        groups.sort_by_key(|(name, _)| name.to_lowercase());
        camera_profiles = groups
            .into_iter()
            .map(|(profile_name, entries)| CameraProfileGroup {
                profile_name,
                entries,
            })
            .collect();
    }

    let looks = registry
        .looks()
        .into_iter()
        .map(|l: LookMetadata| LookBrowserEntry {
            uuid: l.uuid.clone(),
            name: l.name.clone(),
            group: l.group.clone(),
            required_base_profile: l.required_base_profile.clone(),
            file_path: l.file_path.to_string_lossy().into_owned(),
            pairing: registry.resolve_look(&l, camera.unwrap_or("")),
        })
        .collect();

    ProfileBrowserModel {
        camera: camera.map(|s| s.to_string()),
        camera_profiles,
        looks,
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Return the profile browser model for an image: the DCPs for its camera body
/// and every Look with its pairing state for that body (§2.5).
#[tauri::command]
pub fn list_profiles_for_image(
    app_handle: AppHandle,
    path: String,
    state: State<'_, AppState>,
) -> Result<ProfileBrowserModel, String> {
    let registry = state.profile_registry.clone();
    let camera = camera_for_path(Path::new(&path));
    let _ = app_handle;
    Ok(build_browser_model(&registry, camera.as_deref()))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportFailure {
    pub path: String,
    pub reason: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub imported: usize,
    pub failed: Vec<ImportFailure>,
}

/// Minimal XMP well-formedness check. Full Look validation (PresetType ==
/// "Look", required fields) is W7's job; here we only refuse to copy files
/// that are not even well-formed XML.
fn xmp_is_well_formed(bytes: &[u8]) -> bool {
    let mut reader = quick_xml::Reader::from_reader(bytes);
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Eof) => return true,
            Ok(_) => continue,
            Err(_) => return false,
        }
    }
}

/// Import `.dcp` and `.xmp` files into the managed profiles directory.
///
/// DCPs are parsed (via W1) before acceptance — an unparseable file is
/// rejected with a specific reason and never copied (§6.W5 req 4). XMP files
/// are checked for well-formedness and copied; resolving them into registered
/// Looks happens when W7 lands (see the `registry` module docs).
#[tauri::command]
pub fn import_profiles(
    app_handle: AppHandle,
    source_paths: Vec<String>,
    state: State<'_, AppState>,
) -> Result<ImportResult, String> {
    let data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;
    let registry = state.profile_registry.clone();
    let dcp_dir = managed_dcp_dir(&data_dir);
    let looks_dir = managed_looks_dir(&data_dir);
    fs::create_dir_all(&dcp_dir).map_err(|e| e.to_string())?;
    fs::create_dir_all(&looks_dir).map_err(|e| e.to_string())?;

    let mut result = ImportResult {
        imported: 0,
        failed: Vec::new(),
    };

    for source in source_paths {
        match import_one(&app_handle, &dcp_dir, &looks_dir, &registry, &source) {
            Ok(true) => result.imported += 1,
            Ok(false) => {
                // Skipped (e.g. unsupported extension) — not an error.
            }
            Err(reason) => result.failed.push(ImportFailure {
                path: source.clone(),
                reason,
            }),
        }
    }

    persist_index_after_change(&registry, &data_dir);

    Ok(result)
}

fn import_one(
    app_handle: &AppHandle,
    dcp_dir: &Path,
    looks_dir: &Path,
    registry: &ProfileRegistry,
    source: &str,
) -> Result<bool, String> {
    // --- Android SAF content URIs mirror the LUT importer ---
    #[cfg(target_os = "android")]
    if crate::android_integration::is_android_content_uri(source) {
        let bytes = crate::android_integration::read_android_content_uri(source)
            .map_err(|e| format!("failed to read content URI: {e}"))?;
        let name = crate::android_integration::resolve_android_content_uri_name(source)
            .map_err(|e| format!("failed to resolve content URI name: {e}"))?;
        return import_bytes(
            app_handle,
            dcp_dir,
            looks_dir,
            registry,
            &PathBuf::from(name),
            bytes,
            Some(source),
        );
    }

    let source_path = Path::new(source);
    let ext = source_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if !matches!(ext.as_str(), "dcp" | "xmp") {
        return Ok(false);
    }

    let bytes = fs::read(source_path).map_err(|e| format!("cannot read '{}': {e}", source))?;
    import_bytes(
        app_handle,
        dcp_dir,
        looks_dir,
        registry,
        source_path,
        bytes,
        None,
    )
}

fn import_bytes(
    _app_handle: &AppHandle,
    dcp_dir: &Path,
    looks_dir: &Path,
    registry: &ProfileRegistry,
    source_path: &Path,
    bytes: Vec<u8>,
    _display_source: Option<&str>,
) -> Result<bool, String> {
    let ext = source_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let stem = source_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("profile");

    if ext == "dcp" {
        // Validate by parsing BEFORE accepting (req 4).
        let entry = parse_dcp_bytes(bytes.as_slice(), source_path)
            .map_err(|e| format!("invalid DCP '{}': {e}", source_path.display()))?;
        let destination = unique_destination(dcp_dir, stem, "dcp");
        fs::write(&destination, &bytes)
            .map_err(|e| format!("failed to copy '{}': {e}", source_path.display()))?;
        let mut entry = entry;
        entry.file_path = destination;
        registry.insert_dcp(entry);
        Ok(true)
    } else if ext == "xmp" {
        // Minimal well-formedness check; full Look validation is W7.
        if !xmp_is_well_formed(&bytes) {
            return Err(format!(
                "invalid XMP '{}': not well-formed XML",
                source_path.display()
            ));
        }
        let destination = unique_destination(looks_dir, stem, "xmp");
        fs::write(&destination, &bytes)
            .map_err(|e| format!("failed to copy '{}': {e}", source_path.display()))?;
        // W7 integration: resolve the copied XMP into a LookMetadata and call
        // registry.register_look once parse_look_xmp lands.
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Parse DCP bytes into a `ProfileEntry` (used by import validation). Uses W1's
/// in-memory path via a temp-like in-memory parse. We write to a temp file to
/// reuse `parse_dcp`, since import validation happens before the final copy.
fn parse_dcp_bytes(bytes: &[u8], display: &Path) -> Result<ProfileEntry, String> {
    // The parser reads from a path (and mmaps large files). For validation we
    // stage into a temp file, parse, then discard. This guarantees we only
    // ever copy a file that parses cleanly.
    let tmp = tempfile::Builder::new()
        .prefix("rapidraw-dcp-validate-")
        .suffix(".dcp")
        .tempfile()
        .map_err(|e| format!("temp file error: {e}"))?;
    fs::write(tmp.path(), bytes).map_err(|e| e.to_string())?;
    let profiles = crate::dcp::parser::parse_dcp(tmp.path()).map_err(|e| format!("{}", e))?;
    let primary = profiles
        .into_iter()
        .next()
        .ok_or_else(|| "contained no profiles".to_string())?;
    drop(tmp);
    Ok(ProfileEntry {
        id: primary.id,
        file_path: display.to_path_buf(),
        profile_name: primary.profile_name,
        unique_camera_model: primary.unique_camera_model,
        source: ProfileSource::Managed,
        size: bytes.len() as u64,
        mtime_ms: 0,
    })
}

/// Remove a profile by its content-hash id. Only managed (writable) profiles
/// can be removed; Adobe auto-discovered files are read-only.
#[tauri::command]
pub fn remove_profile(
    app_handle: AppHandle,
    id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;
    let registry = state.profile_registry.clone();

    let entry = registry
        .get_by_hex(&id)
        .ok_or_else(|| "profile not found".to_string())?;

    match entry.source {
        ProfileSource::AdobeAutoDiscovered => {
            return Err("Cannot delete an Adobe auto-discovered profile".to_string());
        }
        ProfileSource::Managed => {}
    }

    let managed = strip_verbatim(&managed_dcp_dir(&data_dir));
    let target = strip_verbatim(&entry.file_path);
    if !target.starts_with(&managed) {
        return Err(
            "Access denied: Cannot remove files outside the managed profiles directory".to_string(),
        );
    }

    if target.exists() {
        fs::remove_file(&target).map_err(|e| e.to_string())?;
    }
    registry.remove_dcp(entry.id);
    persist_index_after_change(&registry, &data_dir);
    Ok(())
}

/// Trigger a background rescan of all discovery roots, emitting a Tauri event
/// on completion. Never blocks the caller.
#[tauri::command]
pub fn rescan_profiles(app_handle: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let registry = state.profile_registry.clone();
    let data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;
    spawn_rescan(app_handle, registry, data_dir);
    Ok(())
}

/// Spawn a background discovery pass and emit `profiles-rescanned` when done.
fn spawn_rescan(app_handle: AppHandle, registry: Arc<ProfileRegistry>, data_dir: PathBuf) {
    std::thread::spawn(move || {
        let result = discover_profiles(&data_dir, &registry);
        match result {
            Ok(discovery) => {
                let payload = serde_json::json!({
                    "dcpCount": discovery.dcp_entries.len(),
                    "lookCount": discovery.pending_xmp.len(),
                    "skipped": discovery.skipped.len(),
                });
                let _ = app_handle.emit("profiles-rescanned", payload);
            }
            Err(err) => {
                let _ = app_handle.emit("profiles-rescan-error", err);
            }
        }
    });
}

/// Persist the current registry to `index.json`. Best-effort.
fn persist_index_after_change(registry: &ProfileRegistry, data_dir: &Path) {
    let entries = registry.all_profiles();
    let idx_path = index_path(data_dir);
    let persisted: Vec<crate::dcp::registry::PersistedDcp> = entries
        .iter()
        .map(crate::dcp::registry::PersistedDcp::from_entry)
        .collect();
    if let Err(err) = crate::dcp::registry::save_index(&idx_path, &persisted) {
        log::warn!("failed to persist profile index: {err}");
    }
}

/// The profile registry handle exposed to commands.
pub fn registry_from_state(state: &State<'_, AppState>) -> Arc<ProfileRegistry> {
    state.profile_registry.clone()
}

/// Kick off the background rescan from app startup (see `lib.rs` setup).
pub fn start_background_rescan(app_handle: AppHandle) {
    let data_dir = match app_handle.path().app_data_dir() {
        Ok(d) => d,
        Err(e) => {
            log::error!("profile rescan: cannot resolve app data dir: {e}");
            return;
        }
    };
    let registry = app_handle.state::<AppState>().profile_registry.clone();
    spawn_rescan(app_handle, registry, data_dir);
}

/// Verify the Adobe discovery roots resolve on this platform (used by a
/// startup sanity log; must never fail the app).
pub fn log_discovery_roots() {
    let roots = adobe_discovery_roots();
    if roots.is_empty() {
        log::debug!("profile discovery: no Adobe roots on this platform");
    } else {
        for root in roots {
            log::debug!("profile discovery root: {}", root.display());
        }
    }
}
