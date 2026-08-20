//! Profile registry: discovery, indexing, two-key pairing and per-camera
//! lookup. Workstream W5 of the Cobalt DCP project.
//!
//! The registry holds the parsed metadata for every discovered `.dcp`
//! (via W1's `parser::parse_dcp`) and every known Cobalt Look (via the
//! [`LookEntry`] trait / [`LookMetadata`]). DCPs are indexed by the
//! two-key pair `(unique_camera_model_normalised, profile_name)` per §2.2 —
//! never by filename. Lookups are O(1) against the normalised-camera index.
//!
//! The persistence layer writes `{app_data_dir}/profiles/index.json` holding
//! each DCP's content hash plus file size + mtime so a rescan can skip
//! re-parsing unchanged files.
//!
//! # W7 integration point
//!
//! W7 (branch `cobalt/w7-cobalt-look`, held at decision gate G1) owns
//! `CobaltLook` and `parse_look_xmp`. This module deliberately does NOT
//! define those. It defines the narrow [`LookEntry`] trait — the metadata the
//! pairing logic reads — and a concrete [`LookMetadata`] holder the registry
//! stores. When W7 lands, implement [`LookEntry`] for `CobaltLook` (or adapt)
//! and feed `parse_look_xmp` results into the registry via
//! [`ProfileRegistry::register_look`]. The pairing logic itself is
//! metadata-only, so it is correct and testable now.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::dcp::camera_aliases::normalise_camera_model;
use crate::dcp::model::ProfileId;
use crate::dcp::parser::parse_dcp;

/// Index schema version. Bump to force a full re-parse if the persisted shape
/// changes incompatibly.
const INDEX_VERSION: u32 = 1;

/// The minimum metadata the pairing logic needs from a Cobalt Look.
///
/// `required_base_profile` is the base DCP the Look declares via
/// `crs:CameraProfile` (e.g. `"Cobalt Modular"`); `camera_model_restriction`
/// is `crs:CameraModelRestriction` (empty on all shipped Looks -> `None`).
pub trait LookEntry {
    fn required_base_profile(&self) -> &str;
    fn camera_model_restriction(&self) -> Option<&str>;
    fn uuid(&self) -> &str;
    fn name(&self) -> &str;
    fn group(&self) -> &str;
    fn file_path(&self) -> &Path;
}

/// Concrete metadata holder for a Look, as stored by the registry.
///
/// This is the minimal shape W5's pairing reads. W7's `CobaltLook` carries
/// more (the table source, supports_amount, etc.); when it merges, implement
/// [`LookEntry`] for it or construct a [`LookMetadata`] from it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LookMetadata {
    pub uuid: String,
    pub name: String,
    pub group: String,
    pub required_base_profile: String,
    pub camera_model_restriction: Option<String>,
    pub file_path: PathBuf,
}

impl LookEntry for LookMetadata {
    fn required_base_profile(&self) -> &str {
        &self.required_base_profile
    }
    fn camera_model_restriction(&self) -> Option<&str> {
        self.camera_model_restriction.as_deref()
    }
    fn uuid(&self) -> &str {
        &self.uuid
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn group(&self) -> &str {
        &self.group
    }
    fn file_path(&self) -> &Path {
        &self.file_path
    }
}

/// Where a profile file lives. Managed files are writable and imported into
/// `{app_data_dir}/profiles/`; Adobe auto-discovered files are read-only and
/// best-effort.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileSource {
    /// Imported into `{app_data_dir}/profiles/dcp/`.
    Managed,
    /// Auto-discovered from an Adobe CameraRaw directory (read-only).
    AdobeAutoDiscovered,
}

/// A single indexed DCP profile (metadata only; the full `DcpProfile` is
/// re-parsed lazily when rendering, or produced on demand).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileEntry {
    pub id: ProfileId,
    pub file_path: PathBuf,
    pub profile_name: String,
    pub unique_camera_model: String,
    pub source: ProfileSource,
    pub size: u64,
    pub mtime_ms: u64,
}

impl ProfileEntry {
    pub fn key(&self) -> (String, String) {
        (
            normalise_camera_model(&self.unique_camera_model),
            self.profile_name.clone(),
        )
    }
}

/// Outcome of resolving a Look against the installed base profiles for a
/// camera, per §2.2 / §2.3. Drives the UI's "what you have / what's missing"
/// messaging. D4 in the spec: an unsatisfiable Look must be blocked, never
/// silently substituted.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PairingState {
    Satisfied {
        base: ProfileId,
    },
    MissingBase {
        required_name: String,
        camera: String,
        installed_for_camera: Vec<String>,
    },
    NoProfilesForCamera {
        camera: String,
    },
}

/// The in-memory registry contents protected by a mutex.
#[derive(Default)]
struct InnerRegistry {
    /// Normalised camera model -> profile name -> entries with that key.
    ///
    /// Multiple entries under one `(camera, name)` are kept when they have
    /// different content (different source dirs); see §6.W5 req 5.
    dcp_by_camera: HashMap<String, HashMap<String, Vec<ProfileEntry>>>,
    /// Content-hash -> entry, for O(1) `ProfileId` lookup / removal.
    dcp_by_id: HashMap<ProfileId, ProfileEntry>,
    /// All known Looks, in discovery order (W7 feeds these).
    looks: Vec<LookMetadata>,
}

/// Thread-safe, O(1)-lookup registry of discovered DCPs and Looks.
pub struct ProfileRegistry {
    inner: Mutex<InnerRegistry>,
}

impl Default for ProfileRegistry {
    fn default() -> Self {
        ProfileRegistry::new()
    }
}

impl ProfileRegistry {
    pub fn new() -> Self {
        ProfileRegistry {
            inner: Mutex::new(InnerRegistry::default()),
        }
    }

    /// Insert one DCP entry, replacing any entry with the same content hash
    /// and adding to the two-key index (keeping same-key/different-content
    /// entries side by side).
    pub fn insert_dcp(&self, entry: ProfileEntry) {
        let mut inner = self.inner.lock().unwrap();
        // Remove the previous entry with the same id if present, so the key
        // index does not accumulate stale pointers after a file moves.
        let stale_key = inner.dcp_by_id.get(&entry.id).map(|prev| prev.key());
        if let Some((stale_cam, stale_name)) = stale_key
            && let Some(by_name) = inner.dcp_by_camera.get_mut(&stale_cam)
        {
            if let Some(v) = by_name.get_mut(&stale_name) {
                v.retain(|e| e.id != entry.id);
                if v.is_empty() {
                    by_name.remove(&stale_name);
                }
            }
            if by_name.is_empty() {
                inner.dcp_by_camera.remove(&stale_cam);
            }
        }
        inner
            .dcp_by_camera
            .entry(entry.key().0.clone())
            .or_default()
            .entry(entry.key().1.clone())
            .or_default()
            .push(entry.clone());
        inner.dcp_by_id.insert(entry.id, entry);
    }

    /// Remove a DCP by content hash. Returns whether it was present.
    pub fn remove_dcp(&self, id: ProfileId) -> bool {
        let mut inner = self.inner.lock().unwrap();
        match inner.dcp_by_id.remove(&id) {
            Some(prev) => {
                let (cam, name) = prev.key();
                if let Some(by_name) = inner.dcp_by_camera.get_mut(&cam) {
                    if let Some(v) = by_name.get_mut(&name) {
                        v.retain(|e| e.id != id);
                        if v.is_empty() {
                            by_name.remove(&name);
                        }
                    }
                    if by_name.is_empty() {
                        inner.dcp_by_camera.remove(&cam);
                    }
                }
                true
            }
            None => false,
        }
    }

    /// Register a Look's metadata (W7 feeds this; see the module docs).
    pub fn register_look(&self, look: LookMetadata) {
        let mut inner = self.inner.lock().unwrap();
        inner.looks.retain(|l| l.uuid != look.uuid);
        inner.looks.push(look);
    }

    /// All registered Looks.
    pub fn looks(&self) -> Vec<LookMetadata> {
        self.inner.lock().unwrap().looks.clone()
    }

    /// Look up the installed base DCP for a `(camera, profile_name)` pair,
    /// per §2.2. `camera` is matched via §5.2 rules against the DCPs' stored
    /// `unique_camera_model`. O(1) against the normalised-camera index.
    pub fn lookup_base(&self, camera: &str, profile_name: &str) -> Option<ProfileEntry> {
        let inner = self.inner.lock().unwrap();
        let key = normalise_camera_model(camera);
        inner
            .dcp_by_camera
            .get(&key)
            .and_then(|by_name| by_name.get(profile_name))
            .and_then(|v| v.first().cloned())
    }

    /// Resolve a Look against the installed profiles for a camera. This is
    /// the §2.2 / §2.3 pairing rule, implemented exactly:
    ///
    /// - base DCP resolves -> `Satisfied`
    /// - camera has profiles but not the required base -> `MissingBase`
    ///   (with the list the user DOES have for that body)
    /// - camera has no profiles at all -> `NoProfilesForCamera`
    ///
    /// A Look is NEVER reported applicable when its base is missing — no
    /// silent substitution (§2.3, R9).
    pub fn resolve_look(&self, look: &dyn LookEntry, camera: &str) -> PairingState {
        let inner = self.inner.lock().unwrap();
        let key = normalise_camera_model(camera);
        match inner.dcp_by_camera.get(&key) {
            None => PairingState::NoProfilesForCamera {
                camera: camera.to_string(),
            },
            Some(by_name) => {
                let required = look.required_base_profile();
                match by_name.get(required) {
                    Some(entries) => PairingState::Satisfied {
                        base: entries[0].id,
                    },
                    None => {
                        let mut installed: Vec<String> = by_name.keys().cloned().collect();
                        installed.sort();
                        PairingState::MissingBase {
                            required_name: required.to_string(),
                            camera: camera.to_string(),
                            installed_for_camera: installed,
                        }
                    }
                }
            }
        }
    }

    /// All DCP profile names installed for a given camera (per §5.2 matching),
    /// used by the UI's "here's what you DO have" message.
    pub fn profile_names_for_camera(&self, camera: &str) -> Vec<String> {
        let inner = self.inner.lock().unwrap();
        let key = normalise_camera_model(camera);
        let mut names: Vec<String> = inner
            .dcp_by_camera
            .get(&key)
            .map(|by_name| by_name.keys().cloned().collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    /// All DCP entries installed for a camera.
    pub fn profiles_for_camera(&self, camera: &str) -> Vec<ProfileEntry> {
        let inner = self.inner.lock().unwrap();
        let key = normalise_camera_model(camera);
        inner
            .dcp_by_camera
            .get(&key)
            .map(|by_name| by_name.values().flatten().cloned().collect())
            .unwrap_or_default()
    }

    /// Get a single entry by content hash.
    pub fn get(&self, id: ProfileId) -> Option<ProfileEntry> {
        self.inner.lock().unwrap().dcp_by_id.get(&id).cloned()
    }

    /// Get a single entry by content-hash, given as a lowercase hex string.
    pub fn get_by_hex(&self, hex: &str) -> Option<ProfileEntry> {
        let mut bytes = [0u8; 32];
        if hex.len() != 64 {
            return None;
        }
        for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
            let hi = (chunk[0] as char).to_digit(16)?;
            let lo = (chunk[1] as char).to_digit(16)?;
            bytes[i] = ((hi << 4) | lo) as u8;
        }
        self.get(ProfileId(bytes))
    }

    /// All DCP entries.
    pub fn all_profiles(&self) -> Vec<ProfileEntry> {
        let inner = self.inner.lock().unwrap();
        inner.dcp_by_id.values().cloned().collect()
    }

    /// Number of distinct DCP profiles and Looks.
    pub fn len(&self) -> (usize, usize) {
        let inner = self.inner.lock().unwrap();
        (inner.dcp_by_id.len(), inner.looks.len())
    }
}

// ---------------------------------------------------------------------------
// Discovery + persistence
// ---------------------------------------------------------------------------

/// A file that matched the extension filter but has not yet been resolved into
/// profile metadata (used for `.xmp` Looks until W7's parser lands, and for
/// per-file import accounting).
#[derive(Debug, Clone)]
pub struct PendingFile {
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ms: u64,
}

/// The result of a discovery pass.
#[derive(Debug, Default)]
pub struct DiscoveryResult {
    /// Fully parsed and indexed DCP profiles.
    pub dcp_entries: Vec<ProfileEntry>,
    /// `.xmp` files found but not yet resolved into Looks (W7 integration).
    pub pending_xmp: Vec<PendingFile>,
    /// Files that failed to parse and were skipped, with a reason.
    pub skipped: Vec<(PathBuf, String)>,
}

fn mtime_ms(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
}

/// Persisted, fast-invalidation entry: the cached parse result plus the
/// size+mtime that make it valid.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedEntry {
    path: PathBuf,
    profile_name: String,
    unique_camera_model: String,
    id: ProfileId,
    size: u64,
    mtime_ms: u64,
    source: ProfileSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedIndex {
    version: u32,
    entries: Vec<PersistedEntry>,
}

impl ProfileEntry {
    fn to_persisted(&self) -> PersistedEntry {
        PersistedEntry {
            path: self.file_path.clone(),
            profile_name: self.profile_name.clone(),
            unique_camera_model: self.unique_camera_model.clone(),
            id: self.id,
            size: self.size,
            mtime_ms: self.mtime_ms,
            source: self.source,
        }
    }
}

impl From<PersistedEntry> for ProfileEntry {
    fn from(p: PersistedEntry) -> Self {
        ProfileEntry {
            id: p.id,
            file_path: p.path,
            profile_name: p.profile_name,
            unique_camera_model: p.unique_camera_model,
            source: p.source,
            size: p.size,
            mtime_ms: p.mtime_ms,
        }
    }
}

/// Load the persisted index from `index_path`, if present and current.
fn load_persisted(index_path: &Path) -> Option<Vec<PersistedEntry>> {
    let bytes = fs::read(index_path).ok()?;
    let idx: PersistedIndex = serde_json::from_slice(&bytes).ok()?;
    if idx.version != INDEX_VERSION {
        return None;
    }
    Some(idx.entries)
}

fn save_persisted(index_path: &Path, entries: &[PersistedEntry]) -> Result<(), String> {
    if let Some(parent) = index_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let idx = PersistedIndex {
        version: INDEX_VERSION,
        entries: entries.to_vec(),
    };
    let json = serde_json::to_vec_pretty(&idx).map_err(|e| e.to_string())?;
    fs::write(index_path, json).map_err(|e| e.to_string())
}

/// A DCP entry in its persisted (fast-invalidation) form. Exposed so the
/// command layer can rewrite `index.json` after an import/remove without
/// duplicating the serialization shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedDcp {
    pub path: PathBuf,
    pub profile_name: String,
    pub unique_camera_model: String,
    pub id: ProfileId,
    pub size: u64,
    pub mtime_ms: u64,
    pub source: ProfileSource,
}

impl PersistedDcp {
    pub fn from_entry(e: &ProfileEntry) -> PersistedDcp {
        PersistedDcp {
            path: e.file_path.clone(),
            profile_name: e.profile_name.clone(),
            unique_camera_model: e.unique_camera_model.clone(),
            id: e.id,
            size: e.size,
            mtime_ms: e.mtime_ms,
            source: e.source,
        }
    }
}

impl From<PersistedDcp> for PersistedEntry {
    fn from(p: PersistedDcp) -> Self {
        PersistedEntry {
            path: p.path,
            profile_name: p.profile_name,
            unique_camera_model: p.unique_camera_model,
            id: p.id,
            size: p.size,
            mtime_ms: p.mtime_ms,
            source: p.source,
        }
    }
}

/// Persist a set of DCP entries to `index.json` (same shape discovery reads
/// back for fast invalidation).
pub fn save_index(index_path: &Path, entries: &[PersistedDcp]) -> Result<(), String> {
    let persisted: Vec<PersistedEntry> = entries.iter().cloned().map(Into::into).collect();
    save_persisted(index_path, &persisted)
}

/// Recursively collect files with a given extension under `root`.
fn collect_ext(root: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    if !root.exists() {
        return;
    }
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.file_type().is_file()
            && entry
                .path()
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case(ext))
        {
            out.push(entry.into_path());
        }
    }
}

/// The RapidRAW-managed profiles directory: `{app_data_dir}/profiles/`.
pub fn managed_profiles_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("profiles")
}

/// The managed `dcp/` subdirectory.
pub fn managed_dcp_dir(app_data_dir: &Path) -> PathBuf {
    managed_profiles_dir(app_data_dir).join("dcp")
}

/// The managed `looks/` subdirectory.
pub fn managed_looks_dir(app_data_dir: &Path) -> PathBuf {
    managed_profiles_dir(app_data_dir).join("looks")
}

/// Path to the persisted index file.
pub fn index_path(app_data_dir: &Path) -> PathBuf {
    managed_profiles_dir(app_data_dir).join("index.json")
}

/// The Adobe auto-discover roots for the current platform (read-only,
/// best-effort). Returns an empty vec on Linux (none standard) and is compiled
/// out on Android (W8).
pub fn adobe_discovery_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let mut roots = Vec::new();
        if let Some(home) = std::env::var_os("HOME") {
            let base = PathBuf::from(home).join("Library/Application Support/Adobe/CameraRaw");
            roots.push(base.join("Settings"));
            roots.push(base.join("CameraProfiles"));
        }
        roots
    }
    #[cfg(target_os = "windows")]
    {
        let mut roots = Vec::new();
        if let Some(pd) = std::env::var_os("PROGRAMDATA") {
            roots.push(PathBuf::from(pd).join("Adobe/CameraRaw/Settings"));
        }
        if let Some(ad) = std::env::var_os("APPDATA") {
            roots.push(PathBuf::from(ad).join("Adobe/CameraRaw/Settings"));
        }
        roots
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        Vec::new()
    }
}

/// Discover and index DCPs from the managed directory and Adobe roots, using
/// the persisted index for fast invalidation (skip re-parsing unchanged
/// files). XMP Looks are enumerated as `pending_xmp` — resolving them into
/// [`LookMetadata`] is W7's job (see module docs).
///
/// `app_data_dir` supplies the managed root and the index location.
pub fn discover_profiles(
    app_data_dir: &Path,
    registry: &ProfileRegistry,
) -> Result<DiscoveryResult, String> {
    let idx_path = index_path(app_data_dir);
    let cache = load_persisted(&idx_path).unwrap_or_default();
    let cache_by_path: HashMap<PathBuf, PersistedEntry> =
        cache.into_iter().map(|e| (e.path.clone(), e)).collect();

    let managed_dcp = managed_dcp_dir(app_data_dir);

    let mut dcp_files: Vec<PathBuf> = Vec::new();
    collect_ext(&managed_dcp, "dcp", &mut dcp_files);
    let mut xmp_files: Vec<PathBuf> = Vec::new();
    collect_ext(&managed_looks_dir(app_data_dir), "xmp", &mut xmp_files);

    // Adobe auto-discovery is best-effort and desktop-only; it must degrade
    // silently if the directories are absent (they usually are).
    for root in adobe_discovery_roots() {
        collect_ext(&root, "dcp", &mut dcp_files);
        collect_ext(&root, "xmp", &mut xmp_files);
    }

    dcp_files.sort();
    dcp_files.dedup();
    xmp_files.sort();
    xmp_files.dedup();

    let mut result = DiscoveryResult::default();

    let mut parsed: Vec<PersistedEntry> = Vec::with_capacity(dcp_files.len());
    for path in &dcp_files {
        let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let mtime = mtime_ms(path).unwrap_or(0);
        let source = if path.starts_with(&managed_dcp) {
            ProfileSource::Managed
        } else {
            ProfileSource::AdobeAutoDiscovered
        };

        // Fast path: unchanged file already indexed.
        if let Some(cached) = cache_by_path.get(path)
            && cached.size == size
            && cached.mtime_ms == mtime
        {
            let entry = ProfileEntry::from(cached.clone());
            registry.insert_dcp(entry.clone());
            result.dcp_entries.push(entry);
            parsed.push(cached.clone());
            continue;
        }

        // Slow path: parse and index.
        match parse_dcp(path) {
            Ok(profiles) => {
                // Use the primary profile for the index entry; sub-profiles
                // (ExtraCameraProfiles) share the same content hash and are
                // handled by the render path when W2 needs them.
                if let Some(primary) = profiles.first() {
                    let entry = ProfileEntry {
                        id: primary.id,
                        file_path: path.clone(),
                        profile_name: primary.profile_name.clone(),
                        unique_camera_model: primary.unique_camera_model.clone(),
                        source,
                        size,
                        mtime_ms: mtime,
                    };
                    let persisted = entry.to_persisted();
                    registry.insert_dcp(entry.clone());
                    result.dcp_entries.push(entry);
                    parsed.push(persisted);
                }
            }
            Err(err) => {
                log::warn!("skipping unparseable DCP '{}': {err}", path.display());
                result.skipped.push((path.clone(), err.to_string()));
            }
        }
    }

    for path in &xmp_files {
        let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let mtime = mtime_ms(path).unwrap_or(0);
        result.pending_xmp.push(PendingFile {
            path: path.clone(),
            size,
            mtime_ms: mtime,
        });
    }

    save_persisted(&idx_path, &parsed).ok();

    Ok(result)
}

/// Parse a single `.dcp` file into a `ProfileEntry` (used by import, which
/// must validate before accepting — never copy an invalid file into the
/// managed directory, §6.W5 req 4).
pub fn parse_dcp_entry(path: &Path, source: ProfileSource) -> Result<ProfileEntry, String> {
    let size = fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| format!("cannot stat '{}': {e}", path.display()))?;
    let mtime = mtime_ms(path).unwrap_or(0);
    let profiles = parse_dcp(path).map_err(|e| e.to_string())?;
    let primary = profiles
        .into_iter()
        .next()
        .ok_or_else(|| format!("'{}' contained no profiles", path.display()))?;
    Ok(ProfileEntry {
        id: primary.id,
        file_path: path.to_path_buf(),
        profile_name: primary.profile_name,
        unique_camera_model: primary.unique_camera_model,
        source,
        size,
        mtime_ms: mtime,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_dcp_entry(camera: &str, name: &str) -> ProfileEntry {
        // Distinct id per (camera, name) for stable tests.
        let h = blake3::hash(format!("{camera}\u{1}{name}").as_bytes());
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(h.as_bytes());
        ProfileEntry {
            id: ProfileId(bytes),
            file_path: PathBuf::from(format!("/tmp/{name}.dcp")),
            profile_name: name.to_string(),
            unique_camera_model: camera.to_string(),
            source: ProfileSource::Managed,
            size: 1,
            mtime_ms: 1,
        }
    }

    fn synthetic_look(
        required: &str,
        camera_restriction: Option<&str>,
        name: &str,
    ) -> LookMetadata {
        LookMetadata {
            uuid: format!("uuid-{name}"),
            name: name.to_string(),
            group: "Cobalt CCD fever v3.0".to_string(),
            required_base_profile: required.to_string(),
            camera_model_restriction: camera_restriction.map(|s| s.to_string()),
            file_path: PathBuf::from(format!("/tmp/{name}.xmp")),
        }
    }

    // --- §2.3 the known live mismatch ---

    #[test]
    fn known_live_mismatch_is_missing_base() {
        let reg = ProfileRegistry::new();
        reg.insert_dcp(synthetic_dcp_entry("Fujifilm X-Pro2", "Cobalt Flat"));

        let look = synthetic_look("Cobalt Modular", None, "Pentax 645D Portrait");
        let state = reg.resolve_look(&look, "Fujifilm X-Pro2");

        assert_eq!(
            state,
            PairingState::MissingBase {
                required_name: "Cobalt Modular".to_string(),
                camera: "Fujifilm X-Pro2".to_string(),
                installed_for_camera: vec!["Cobalt Flat".to_string()],
            }
        );
    }

    // --- §2.2 satisfied ---

    #[test]
    fn satisfied_when_base_present() {
        let reg = ProfileRegistry::new();
        let flat = synthetic_dcp_entry("Fujifilm X-Pro2", "Cobalt Modular");
        reg.insert_dcp(flat.clone());

        let look = synthetic_look("Cobalt Modular", None, "Pentax 645D Portrait");
        assert_eq!(
            reg.resolve_look(&look, "Fujifilm X-Pro2"),
            PairingState::Satisfied { base: flat.id }
        );
    }

    #[test]
    fn satisfied_matches_by_normalised_camera_via_alias() {
        let reg = ProfileRegistry::new();
        let modular = synthetic_dcp_entry("Fujifilm X-Pro2", "Cobalt Modular");
        reg.insert_dcp(modular.clone());

        let look = synthetic_look("Cobalt Modular", None, "Pentax 645D Portrait");
        // Camera supplied as the manufacturer alias "Fuji".
        assert_eq!(
            reg.resolve_look(&look, "Fuji X-Pro2"),
            PairingState::Satisfied { base: modular.id }
        );
    }

    // --- no profiles for camera ---

    #[test]
    fn no_profiles_for_camera() {
        let reg = ProfileRegistry::new();
        reg.insert_dcp(synthetic_dcp_entry("Nikon D850", "Cobalt Flat"));

        let look = synthetic_look("Cobalt Modular", None, "Pentax 645D Portrait");
        assert_eq!(
            reg.resolve_look(&look, "Sony A7R IV"),
            PairingState::NoProfilesForCamera {
                camera: "Sony A7R IV".to_string(),
            }
        );
    }

    // --- §2.3 negative: never substitute a missing base ---

    #[test]
    fn negative_no_silent_substitution() {
        let reg = ProfileRegistry::new();
        // A "Cobalt Flat" base exists for this camera, but the Look needs
        // "Cobalt Modular".
        reg.insert_dcp(synthetic_dcp_entry("Fujifilm X-Pro2", "Cobalt Flat"));

        let look = synthetic_look("Cobalt Modular", None, "Pentax 645D Portrait");
        let state = reg.resolve_look(&look, "Fujifilm X-Pro2");

        // MUST NOT be Satisfied — never over a different base (R9).
        assert!(
            !matches!(state, PairingState::Satisfied { .. }),
            "Look must never be satisfied over a mismatched base"
        );
        assert_eq!(
            state,
            PairingState::MissingBase {
                required_name: "Cobalt Modular".to_string(),
                camera: "Fujifilm X-Pro2".to_string(),
                installed_for_camera: vec!["Cobalt Flat".to_string()],
            }
        );
    }

    // --- two-key lookup: same name, different camera is a different profile ---

    #[test]
    fn two_key_lookup_same_name_different_camera() {
        let reg = ProfileRegistry::new();
        let xpro2 = synthetic_dcp_entry("Fujifilm X-Pro2", "Cobalt Flat");
        let pentax = synthetic_dcp_entry("Pentax 645D", "Cobalt Flat");
        reg.insert_dcp(xpro2.clone());
        reg.insert_dcp(pentax.clone());

        assert_eq!(
            reg.lookup_base("Fujifilm X-Pro2", "Cobalt Flat")
                .map(|e| e.id),
            Some(xpro2.id)
        );
        assert_eq!(
            reg.lookup_base("Pentax 645D", "Cobalt Flat").map(|e| e.id),
            Some(pentax.id)
        );
        assert_eq!(reg.lookup_base("Pentax 645D", "Cobalt Modular"), None);
    }

    // --- duplicate handling (§6.W5 req 5) ---

    #[test]
    fn same_content_hash_dedupes() {
        let reg = ProfileRegistry::new();
        let a = synthetic_dcp_entry("Fujifilm X-Pro2", "Cobalt Flat");
        let mut b = a.clone();
        // Same id (same content) but a different path.
        b.file_path = PathBuf::from("/tmp/other/Cobalt Flat.dcp");
        reg.insert_dcp(a);
        reg.insert_dcp(b);

        assert_eq!(reg.all_profiles().len(), 1);
    }

    #[test]
    fn same_camera_name_different_content_kept_both() {
        let reg = ProfileRegistry::new();
        let mut a = synthetic_dcp_entry("Fujifilm X-Pro2", "Cobalt Flat");
        a.file_path = PathBuf::from("/tmp/dirA/Cobalt Flat.dcp");
        let mut b = synthetic_dcp_entry("Fujifilm X-Pro2", "Cobalt Flat");
        b.file_path = PathBuf::from("/tmp/dirB/Cobalt Flat.dcp");
        // Force different ids (different content).
        let h = blake3::hash("different-content".as_bytes());
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(h.as_bytes());
        b.id = ProfileId(bytes);

        reg.insert_dcp(a.clone());
        reg.insert_dcp(b.clone());

        assert_eq!(reg.all_profiles().len(), 2);
        // Both appear under the same (camera, name) key.
        let profiles = reg.profiles_for_camera("Fujifilm X-Pro2");
        assert_eq!(profiles.len(), 2);
    }

    // --- remove ---

    #[test]
    fn remove_dcp() {
        let reg = ProfileRegistry::new();
        let entry = synthetic_dcp_entry("Fujifilm X-Pro2", "Cobalt Flat");
        reg.insert_dcp(entry.clone());
        assert!(reg.remove_dcp(entry.id));
        assert!(!reg.remove_dcp(entry.id));
        assert!(reg.profiles_for_camera("Fujifilm X-Pro2").is_empty());
    }

    // --- O(1) lookup & 500-profile performance (§6.W5 acceptance) ---

    #[test]
    fn five_hundred_profiles_indexes_under_2s_and_looks_up_o1() {
        let start = std::time::Instant::now();
        let reg = ProfileRegistry::new();
        for i in 0..500 {
            let camera = format!("Camera Model {}", i % 25);
            let name = format!("Profile {}", i);
            reg.insert_dcp(synthetic_dcp_entry(&camera, &name));
        }
        let index_elapsed = start.elapsed();

        // Indexing 500 metadata entries must be well under 2s.
        assert!(
            index_elapsed.as_secs_f64() < 2.0,
            "indexing 500 profiles took {:?}",
            index_elapsed
        );

        // Lookup is O(1) — a direct HashMap hit regardless of registry size.
        let lookup_start = std::time::Instant::now();
        let found = reg.lookup_base("Camera Model 7", "Profile 107");
        let lookup_elapsed = lookup_start.elapsed();

        assert!(found.is_some(), "expected profile 107 for camera model 7");
        assert!(
            lookup_elapsed.as_secs_f64() < 1.0,
            "single lookup took {:?}",
            lookup_elapsed
        );

        let (n_dcp, n_looks) = reg.len();
        assert_eq!(n_dcp, 500);
        assert_eq!(n_looks, 0);
    }

    // --- LookEntry via LookMetadata ---

    #[test]
    fn look_metadata_implements_look_entry() {
        let look = synthetic_look("Cobalt Modular", None, "Pentax 645D Portrait");
        let e: &dyn LookEntry = &look;
        assert_eq!(e.required_base_profile(), "Cobalt Modular");
        assert_eq!(e.camera_model_restriction(), None);
        assert_eq!(e.name(), "Pentax 645D Portrait");
        assert_eq!(e.group(), "Cobalt CCD fever v3.0");
        assert_eq!(e.uuid(), "uuid-Pentax 645D Portrait");
    }

    // ---- discovery + persistence (req 1: index + index.json with mtime/size) ---

    /// Minimal valid little-endian DCP writer for a `(camera, name)` profile.
    /// Mirrors W1's synthetic builder so discovery can be integration-tested
    /// without vendor assets.
    fn write_synthetic_dcp(path: &Path, camera: &str, name: &str) {
        #[derive(Clone)]
        struct Tag {
            tag: u16,
            typ: u16,
            data: Vec<u8>,
        }
        let ascii = |s: &str| -> Vec<u8> {
            let mut v = s.as_bytes().to_vec();
            v.push(0);
            v
        };
        let srat = |v: &[(i32, i32)]| -> Vec<u8> {
            v.iter()
                .flat_map(|(n, d)| {
                    let mut b = n.to_le_bytes().to_vec();
                    b.extend_from_slice(&d.to_le_bytes());
                    b
                })
                .collect()
        };
        let tags = [
            Tag {
                tag: 50708,
                typ: 2,
                data: ascii(camera),
            },
            Tag {
                tag: 50936,
                typ: 2,
                data: ascii(name),
            },
            Tag {
                tag: 50721,
                typ: 10,
                data: srat(&[
                    (1, 1),
                    (0, 1),
                    (0, 1),
                    (0, 1),
                    (1, 1),
                    (0, 1),
                    (0, 1),
                    (0, 1),
                    (1, 1),
                ]),
            },
            Tag {
                tag: 50778,
                typ: 3,
                data: 17u16.to_le_bytes().to_vec(),
            },
            Tag {
                tag: 50779,
                typ: 3,
                data: 21u16.to_le_bytes().to_vec(),
            },
            Tag {
                tag: 50941,
                typ: 4,
                data: 2u32.to_le_bytes().to_vec(),
            },
        ];

        let mut ifd = Vec::new();
        ifd.extend_from_slice(&(tags.len() as u16).to_le_bytes());
        let entry_start = ifd.len();
        ifd.resize(entry_start + tags.len() * 12, 0);
        ifd.extend_from_slice(&0u32.to_le_bytes());
        let mut pool = Vec::new();
        for (i, t) in tags.iter().enumerate() {
            let count = match t.typ {
                2 => t.data.len() as u32,
                3 => (t.data.len() / 2) as u32,
                4 => (t.data.len() / 4) as u32,
                10 => (t.data.len() / 8) as u32,
                _ => 0,
            };
            let base = entry_start + i * 12;
            ifd[base..base + 2].copy_from_slice(&t.tag.to_le_bytes());
            ifd[base + 2..base + 4].copy_from_slice(&t.typ.to_le_bytes());
            ifd[base + 4..base + 8].copy_from_slice(&count.to_le_bytes());
            if t.data.len() <= 4 {
                ifd[base + 8..base + 8 + t.data.len()].copy_from_slice(&t.data);
            } else {
                let off = 8 + ifd.len() + pool.len();
                ifd[base + 8..base + 12].copy_from_slice(&(off as u32).to_le_bytes());
                pool.extend_from_slice(&t.data);
            }
        }
        ifd.extend_from_slice(&pool);

        let mut out = Vec::new();
        out.extend_from_slice(b"II");
        out.extend_from_slice(&0x4352u16.to_le_bytes());
        out.extend_from_slice(&8u32.to_le_bytes());
        out.extend_from_slice(&ifd);
        std::fs::write(path, &out).unwrap();
    }

    #[test]
    fn discovery_indexes_dcp_and_persists_index_json() {
        let tmp = tempfile::tempdir().unwrap();
        let app_data = tmp.path();
        let dcp_dir = managed_dcp_dir(app_data);
        std::fs::create_dir_all(&dcp_dir).unwrap();
        write_synthetic_dcp(
            &dcp_dir.join("Cobalt Flat.dcp"),
            "Fujifilm X-Pro2",
            "Cobalt Flat",
        );

        let reg = ProfileRegistry::new();
        let result = discover_profiles(app_data, &reg).unwrap();

        assert_eq!(result.dcp_entries.len(), 1);
        let entry = &result.dcp_entries[0];
        assert_eq!(entry.profile_name, "Cobalt Flat");
        assert_eq!(entry.unique_camera_model, "Fujifilm X-Pro2");
        assert_eq!(entry.source, ProfileSource::Managed);
        assert!(result.skipped.is_empty());

        // index.json was written.
        let idx_path = index_path(app_data);
        assert!(idx_path.exists(), "index.json should be persisted");
        let json = std::fs::read_to_string(&idx_path).unwrap();
        assert!(json.contains("Cobalt Flat"));
        assert!(json.contains("Fujifilm X-Pro2"));

        // A second pass (mtime+size unchanged) still indexes it.
        let reg2 = ProfileRegistry::new();
        let result2 = discover_profiles(app_data, &reg2).unwrap();
        assert_eq!(result2.dcp_entries.len(), 1);
        assert_eq!(result2.dcp_entries[0].id, entry.id);
    }

    #[test]
    fn discovery_reparses_after_content_change() {
        let tmp = tempfile::tempdir().unwrap();
        let app_data = tmp.path();
        let dcp_dir = managed_dcp_dir(app_data);
        std::fs::create_dir_all(&dcp_dir).unwrap();
        let file = dcp_dir.join("Cobalt Flat.dcp");

        write_synthetic_dcp(&file, "Fujifilm X-Pro2", "Cobalt Flat");
        let reg = ProfileRegistry::new();
        let r1 = discover_profiles(app_data, &reg).unwrap();
        assert_eq!(r1.dcp_entries.len(), 1);
        let first_id = r1.dcp_entries[0].id;

        // Change the profile name inside the same file -> different content.
        write_synthetic_dcp(&file, "Fujifilm X-Pro2", "Cobalt Modular");
        let reg2 = ProfileRegistry::new();
        let r2 = discover_profiles(app_data, &reg2).unwrap();
        assert_eq!(r2.dcp_entries.len(), 1);
        assert_eq!(r2.dcp_entries[0].profile_name, "Cobalt Modular");
        assert_ne!(
            r2.dcp_entries[0].id, first_id,
            "content changed -> id must change"
        );
    }
}
