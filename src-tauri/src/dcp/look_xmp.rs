//! Cobalt Look XMP parsing and embedded RGB look-table acquisition (W7).
//!
//! A Cobalt "Look" (part of the "CCD Fever" add-on) is an Adobe Camera Raw
//! `.xmp` preset of `crs:PresetType="Look"`. It sits *on top of* a per-camera
//! base DCP profile and carries its colour grade as an embedded RGB table.
//!
//! Two responsibilities live here:
//!
//! 1. **Metadata** (`parse_look_xmp`) - the browser UI, grouping and pairing
//!    logic (W5/W6) only need the fields, not the table. Parsed with
//!    `quick-xml`, which also sidesteps the §1.1 self-closing-`rdf:li` trap
//!    that a naive regex falls into.
//!
//! 2. **Table acquisition** (Route A, `decode_rgb_table`) - the embedded
//!    `crs:Table_<uuid>` blob is Adobe's `dng_big_table` serialization:
//!    a Z85-like base85 text encoding over a zlib stream wrapping a binary
//!    `dng_rgb_table`. This is a documented Adobe format. The reader here
//!    decodes it into a 3-D RGB LUT; there is deliberately **no writer or
//!    re-encoder** - this is read-only interoperability for tables the user
//!    already owns (spec §6.W7, §8.3).

use std::path::{Path, PathBuf};

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::dcp::DcpError;

/// Identifier for a Cobalt Look: blake3 of the file bytes, stable and
/// deduplicating (same convention as the DCP `ProfileId` in §5.1).
pub type LookId = String;

/// Declared primaries of an RGB look table (`dng_rgb_table` field 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookColorSpace {
    Srgb,
    Adobe,
    ProPhoto,
    DisplayP3,
    Rec2020,
}

impl LookColorSpace {
    fn from_index(i: u32) -> Option<Self> {
        match i {
            0 => Some(Self::Srgb),
            1 => Some(Self::Adobe),
            2 => Some(Self::ProPhoto),
            3 => Some(Self::DisplayP3),
            4 => Some(Self::Rec2020),
            _ => None,
        }
    }
}

/// Declared transfer function of an RGB look table (`dng_rgb_table` field 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookTransfer {
    Linear,
    Srgb,
    Gamma18,
    Gamma22,
    Rec2020,
}

impl LookTransfer {
    fn from_index(i: u32) -> Option<Self> {
        match i {
            0 => Some(Self::Linear),
            1 => Some(Self::Srgb),
            2 => Some(Self::Gamma18),
            3 => Some(Self::Gamma22),
            4 => Some(Self::Rec2020),
            _ => None,
        }
    }
}

/// Gamut-handling policy of an RGB look table (clip vs extend).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookGamut {
    Clip,
    Extend,
}

/// After parsing, check whether a captured `.cube` LUT (Route B) exists for
/// this Look. If the table is currently `Unavailable` (Route A failed or
/// wasn't attempted) and a capture file is on disk, upgrades to `Captured`.
pub fn resolve_captured_table(captured_dir: &std::path::Path, look: &mut CobaltLook) {
    if matches!(look.table, LookTableSource::Unavailable)
        && let Some(cube_path) = crate::dcp::capture::find_captured_lut(captured_dir, &look.uuid)
    {
        log::info!(
            "Cobalt Look `{}` resolved via Route B capture: {}",
            look.uuid,
            cube_path.display()
        );
        look.table = LookTableSource::Captured { cube_path };
    }
}

/// A decoded Cobalt RGB look table (`dng_rgb_table`).
///
/// Samples are stored as `u16` **deltas** from a neutral identity ramp; the
/// raw (delta-encoded) values are kept so nothing is lost, and
/// [`CobaltRgbTable::sample`] applies the ramp to recover the true [0,1]
/// output at a grid point. This is a plain 3-D RGB LUT (not an HSV table);
/// it is applied as stage 7 in the §3.1 pipeline, in the declared colour
/// space (for the supplied looks: ProPhoto primaries, 1.8 gamma).
#[derive(Debug, Clone)]
pub struct CobaltRgbTable {
    /// Grid points per axis. The supplied Cobalt looks all use 32 (32^3).
    pub divisions: u32,
    /// Raw delta-encoded samples, dng order (`r` outermost, `b` innermost),
    /// length == `divisions^3`. The identity ramp is applied in `sample`.
    pub samples: Vec<[u16; 3]>,
    pub primaries: LookColorSpace,
    pub transfer: LookTransfer,
    pub gamut: LookGamut,
    /// ACR's intensity range for this Look (spec §2.4); typically [0.2, 1.5].
    pub min_amount: f64,
    pub max_amount: f64,
}

impl CobaltRgbTable {
    /// Recover the normalised [0,1] output RGB at integer grid index `(ri, gi, bi)`.
    /// See the identity-ramp construction `nop[i] = (i*0xFFFF + (N>>1)) / (N-1)`.
    pub fn sample(&self, ri: u32, gi: u32, bi: u32) -> [f32; 3] {
        let n = self.divisions;
        let idx = (ri * n + gi) * n + bi;
        let [r, g, b] = self.samples[idx as usize];
        let nop = |i: u32| (i * 0xFFFF + (n >> 1)) / (n - 1);
        [
            ((r as u32 + nop(ri)) & 0xFFFF) as f32 / 65535.0,
            ((g as u32 + nop(gi)) & 0xFFFF) as f32 / 65535.0,
            ((b as u32 + nop(bi)) & 0xFFFF) as f32 / 65535.0,
        ]
    }

    /// Normalised [0,1] output at the black corner, which must be exactly
    /// identity (0,0,0) for a valid table.
    pub fn black(&self) -> [f32; 3] {
        self.sample(0, 0, 0)
    }
}

/// Where a Look's table comes from (spec §6.W7 Routes A/B/C).
#[derive(Debug, Clone)]
pub enum LookTableSource {
    /// Route A - decoded natively from the embedded `dng_big_table` blob.
    /// Read-only; never re-encoded or exported.
    Decoded(CobaltRgbTable),
    /// Route B - captured via an ACR round-trip (identity HALD -> ACR ->
    /// `.cube`). Table not yet present in this spike; W6 wires the workflow.
    Captured { cube_path: PathBuf },
    /// Route C - the floor: metadata only, no table. Look still browsers and
    /// pairs, but is shown with a "table unavailable" state.
    Unavailable,
}

/// A parsed Cobalt Look (spec §5.1, plus `short_name`/`sort_name` which W7's
/// acceptance criteria require asserting).
#[derive(Debug, Clone)]
pub struct CobaltLook {
    pub id: LookId,
    pub file_path: PathBuf,
    /// `crs:UUID` - unique per file.
    pub uuid: String,
    /// `crs:Name` - e.g. "Pentax 645D Portrait".
    pub name: String,
    /// `crs:Group` - UI grouping, e.g. "Cobalt CCD fever v3.0".
    pub group: String,
    /// `crs:Cluster`, e.g. "Cobalt-Image".
    pub cluster: String,
    /// `crs:ShortName` - empty on all supplied Looks (§1.1).
    pub short_name: String,
    /// `crs:SortName` - empty on all supplied Looks (§1.1).
    pub sort_name: String,
    /// `crs:CameraProfile` - the base profile the Look requires,
    /// "Cobalt Modular" on all supplied Looks (§2.3).
    pub required_base_profile: String,
    /// `crs:CameraModelRestriction` - empty -> `None` on all supplied Looks,
    /// i.e. no source-camera restriction.
    pub camera_model_restriction: Option<String>,
    pub supports_amount: bool,
    /// `crs:RGBTable` - the id of the embedded table.
    pub table_uuid: String,
    /// The acquired table (Route A decoded / Route B captured / Route C none).
    pub table: LookTableSource,
    pub copyright: Option<String>,
    pub process_version: String,
}

// ---------------------------------------------------------------------------
// Phase 1 - metadata parsing
// ---------------------------------------------------------------------------

/// Parse a Cobalt Look XMP from disk into its metadata + table.
pub fn parse_look_xmp(path: &Path) -> Result<CobaltLook, DcpError> {
    let content = std::fs::read(path).map_err(|e| DcpError::NotACobaltLook(e.to_string()))?;
    let id = blake3::hash(&content).to_hex().to_string();
    parse_look_xmp_bytes(&content, path.to_path_buf(), id)
}

/// Parse a Cobalt Look XMP from raw bytes. `quick-xml` is used throughout so
/// the self-closing `rdf:li` case (§1.1) is handled by the XML parser rather
/// than a regex that would silently grab the *next* element's text.
pub fn parse_look_xmp_bytes(
    content: &[u8],
    file_path: PathBuf,
    id: LookId,
) -> Result<CobaltLook, DcpError> {
    let mut reader = Reader::from_reader(content);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut attrs: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    // `open` is the stack of currently-open `crs:` elements (for routing Text
    // events to the innermost element). `completed` holds the final text of
    // each `crs:` element once it closes. Self-closing elements (ShortName,
    // SortName) go straight to `completed`; text-bearing elements (Name, Group)
    // move from `open` to `completed` on their End event. Without this split,
    // popping on End would discard the captured text - which is why Name/Group
    // came back empty and only the self-closing fields survived.
    let mut open: Vec<(Vec<u8>, String)> = Vec::new();
    let mut completed: Vec<(Vec<u8>, String)> = Vec::new();
    let mut in_description = false;

    loop {
        let ev = reader
            .read_event_into(&mut buf)
            .map_err(|e| DcpError::NotACobaltLook(format!("XML parse error: {e}")))?;
        match ev {
            Event::Start(e) => {
                // Use the full qualified element name: `rdf:Description`,
                // `crs:Name`, `crs:Group`, ...
                let name = e.name();
                if name.as_ref() == b"rdf:Description" {
                    in_description = true;
                    attrs.clear();
                    open.clear();
                    completed.clear();
                    for a in e.attributes().flatten() {
                        // Attribute keys are matched by local name (no prefix).
                        let key = local_name(a.key.as_ref()).to_vec();
                        let value = a
                            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                            .map_err(|e| DcpError::NotACobaltLook(format!("attr decode: {e}")))?
                            .into_owned()
                            .into_bytes();
                        attrs.push((key, value));
                    }
                } else if in_description && name.as_ref().starts_with(b"crs:") {
                    open.push((name.as_ref().to_vec(), String::new()));
                }
            }
            Event::Empty(e) if in_description => {
                // Self-closing <crs:ShortName/> or <rdf:li .../>.
                let name = e.name();
                if name.as_ref().starts_with(b"crs:") {
                    completed.push((name.as_ref().to_vec(), String::new()));
                }
            }
            Event::Text(e) => {
                if in_description
                    && !open.is_empty()
                    && let Ok(t) = e.decode()
                {
                    let last = open.last_mut().unwrap();
                    if last.1.is_empty() {
                        last.1.push_str(&t);
                    }
                }
            }
            Event::End(e) => {
                let name = e.name();
                if name.as_ref() == b"rdf:Description" {
                    break;
                }
                if in_description && name.as_ref().starts_with(b"crs:") && !open.is_empty() {
                    // Move the completed element from the open stack to
                    // `completed` so its captured text is retained.
                    let entry = open.pop().unwrap();
                    completed.push(entry);
                }
            }
            _ => {}
        }
        buf.clear();
    }

    build_look(&attrs, &completed, file_path, id)
}

fn local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|&b| b == b':').next().unwrap_or(name)
}

fn attr<'a>(attrs: &'a [(Vec<u8>, Vec<u8>)], key: &str) -> Option<&'a [u8]> {
    attrs
        .iter()
        .find(|(k, _)| k == key.as_bytes())
        .map(|(_, v)| v.as_slice())
}

fn nested_value(nested: &[(Vec<u8>, String)], key: &str) -> Option<String> {
    nested
        .iter()
        .find(|(k, _)| k == key.as_bytes())
        .map(|(_, v)| v.clone())
}

fn str_attr(
    attrs: &[(Vec<u8>, Vec<u8>)],
    key: &str,
    field: &'static str,
) -> Result<String, DcpError> {
    attr(attrs, key)
        .map(|v| String::from_utf8_lossy(v).into_owned())
        .ok_or(DcpError::MissingField(field))
}

fn bool_attr(
    attrs: &[(Vec<u8>, Vec<u8>)],
    key: &str,
    field: &'static str,
) -> Result<bool, DcpError> {
    match str_attr(attrs, key, field)?.as_str() {
        "True" | "true" => Ok(true),
        "False" | "false" => Ok(false),
        other => Err(DcpError::InvalidValue {
            field,
            detail: format!("expected True/False, got {other:?}"),
        }),
    }
}

fn build_look(
    attrs: &[(Vec<u8>, Vec<u8>)],
    nested: &[(Vec<u8>, String)],
    file_path: PathBuf,
    id: LookId,
) -> Result<CobaltLook, DcpError> {
    // Reject non-Looks up front so we never misparse a preset as a Look.
    let preset_type = str_attr(attrs, "PresetType", "PresetType")?;
    if preset_type != "Look" {
        return Err(DcpError::NotACobaltLook(format!(
            "crs:PresetType = {preset_type:?}, expected \"Look\""
        )));
    }

    // camera_model_restriction: empty attribute -> None.
    let camera_model_restriction = match attr(attrs, "CameraModelRestriction") {
        Some(v) if !v.is_empty() => Some(String::from_utf8_lossy(v).into_owned()),
        _ => None,
    };

    let copyright = attr(attrs, "Copyright")
        .map(|v| String::from_utf8_lossy(v).into_owned())
        .filter(|s| !s.is_empty());

    let table_uuid = str_attr(attrs, "RGBTable", "RGBTable")?;
    let table_blob = attr(attrs, &format!("Table_{table_uuid}")).map(|v| v.to_vec());

    let table = match table_blob {
        Some(blob) => match decode_rgb_table(&blob) {
            Ok(t) => LookTableSource::Decoded(t),
            Err(e) => {
                // A malformed table must not take the whole Look down: keep the
                // metadata and surface it as unavailable (Route C floor).
                log::warn!("Cobalt look `{}` table decode failed: {e}", table_uuid);
                LookTableSource::Unavailable
            }
        },
        None => LookTableSource::Unavailable,
    };

    Ok(CobaltLook {
        id,
        file_path,
        uuid: str_attr(attrs, "UUID", "UUID")?,
        name: nested_value(nested, "crs:Name").unwrap_or_default(),
        group: nested_value(nested, "crs:Group").unwrap_or_default(),
        short_name: nested_value(nested, "crs:ShortName").unwrap_or_default(),
        sort_name: nested_value(nested, "crs:SortName").unwrap_or_default(),
        cluster: str_attr(attrs, "Cluster", "Cluster")?,
        required_base_profile: str_attr(attrs, "CameraProfile", "CameraProfile")?,
        camera_model_restriction,
        supports_amount: bool_attr(attrs, "SupportsAmount", "SupportsAmount")?,
        table_uuid,
        table,
        copyright,
        process_version: str_attr(attrs, "ProcessVersion", "ProcessVersion")?,
    })
}

// ---------------------------------------------------------------------------
// Phase 2, Route A - decode the embedded dng_big_table / dng_rgb_table
// ---------------------------------------------------------------------------

/// Adobe's base85 decode table (`dng_big_table::DecodeFromString`), indexed by
/// `ord(char) - 32`. `0xFF` marks characters that are skipped (the eight
/// XML-unsafe chars that never appear in the blob). This is a Z85-like
/// alphabet - the value is *not* the character's ordinal position.
const KDECODE: [u8; 96] = [
    0xFF, 0x44, 0xFF, 0x54, 0x53, 0x52, 0xFF, 0x49, 0x4B, 0x4C, 0x46, 0x41, 0xFF, 0x3F, 0x3E, 0x45,
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x40, 0xFF, 0xFF, 0x42, 0xFF, 0x47,
    0x51, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x2B, 0x2C, 0x2D, 0x2E, 0x2F, 0x30, 0x31, 0x32,
    0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x3B, 0x3C, 0x3D, 0x4D, 0xFF, 0x4E, 0x43, 0xFF,
    0x48, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18,
    0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F, 0x20, 0x21, 0x22, 0x23, 0x4F, 0x4A, 0x50, 0xFF, 0xFF,
];

/// Decode Adobe base85 text into the compressed binary block.
fn decode_base85(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut phase = 0u8;
    let mut value: u64 = 0;
    for &ch in text {
        if !(32..=127).contains(&ch) {
            continue;
        }
        let d = KDECODE[(ch - 32) as usize] as u32;
        if d > 85 {
            continue;
        }
        phase += 1;
        match phase {
            1 => value = d as u64,
            2 => value += (d as u64) * 85,
            3 => value += (d as u64) * 85 * 85,
            4 => value += (d as u64) * 85 * 85 * 85,
            _ => {
                value += (d as u64) * 85 * 85 * 85 * 85;
                out.extend_from_slice(&(value as u32).to_le_bytes());
                phase = 0;
            }
        }
    }
    if phase > 1 {
        let bytes = (value as u32).to_le_bytes();
        out.extend_from_slice(&bytes[..(phase - 1) as usize]);
    }
    out
}

/// Decompress the block: first 4 bytes (LE) = uncompressed size; the rest is
/// a zlib stream (RFC 1950). Bounds-checked: never trust the header blindly.
fn decompress_block(compressed: &[u8]) -> Result<Vec<u8>, DcpError> {
    if compressed.len() < 5 {
        return Err(DcpError::TableDecode(
            "compressed block shorter than 5 bytes".into(),
        ));
    }
    let expected = u32::from_le_bytes(compressed[..4].try_into().unwrap()) as usize;
    // Guard against absurd allocations from a corrupt size header.
    if expected > 1 << 28 {
        return Err(DcpError::TableDecode(format!(
            "declared uncompressed size {expected} exceeds 256 MiB"
        )));
    }
    use std::io::Read;
    let mut out = Vec::with_capacity(expected);
    let mut dec = flate2::bufread::ZlibDecoder::new(compressed[4..].as_ref());
    dec.read_to_end(&mut out)
        .map_err(|e| DcpError::TableDecode(format!("zlib inflate: {e}")))?;
    if out.len() != expected {
        return Err(DcpError::TableDecode(format!(
            "size mismatch: header says {expected}, inflated {}",
            out.len()
        )));
    }
    Ok(out)
}

/// Decode the embedded `dng_rgb_table` from its base85 blob (Route A).
///
/// Blob layout: base85 -> [u32 LE uncompressed_size][zlib data]; then a
/// `dng_rgb_table`:
/// ```text
///   u32 type       = 1 (RGBTable)
///   u32 version    = 1
///   u32 dimensions = 3
///   u32 divisions  = N
///   N^3 x (u16 r, u16 g, u16 b)  - deltas from an identity ramp
///   u32 primaries, u32 gamma, u32 gamut
///   f64 min_amount, f64 max_amount
/// ```
pub fn decode_rgb_table(blob: &[u8]) -> Result<CobaltRgbTable, DcpError> {
    let compressed = decode_base85(blob);
    let raw = decompress_block(&compressed)?;
    parse_rgb_table(&raw)
}

// The final `off += 8` (after max_amount) is legitimately never re-read; the
// forward-only cursor makes that last write unused, which is harmless.
#[allow(unused_assignments)]
fn parse_rgb_table(raw: &[u8]) -> Result<CobaltRgbTable, DcpError> {
    let mut off = 0usize;
    macro_rules! u32 {
        () => {{
            if off + 4 > raw.len() {
                return Err(DcpError::TableDecode("truncated u32".into()));
            }
            let v = u32::from_le_bytes(raw[off..off + 4].try_into().unwrap());
            off += 4;
            v
        }};
    }
    macro_rules! u16 {
        () => {{
            if off + 2 > raw.len() {
                return Err(DcpError::TableDecode("truncated u16".into()));
            }
            let v = u16::from_le_bytes(raw[off..off + 2].try_into().unwrap());
            off += 2;
            v
        }};
    }
    macro_rules! f64 {
        () => {{
            if off + 8 > raw.len() {
                return Err(DcpError::TableDecode("truncated f64".into()));
            }
            let v = f64::from_le_bytes(raw[off..off + 8].try_into().unwrap());
            off += 8;
            v
        }};
    }

    if u32!() != 1 {
        return Err(DcpError::TableDecode("not an RGB table (type != 1)".into()));
    }
    if u32!() != 1 {
        return Err(DcpError::TableDecode("unknown RGB table version".into()));
    }
    let dimensions = u32!();
    let divisions = u32!();
    if dimensions != 3 {
        return Err(DcpError::TableDecode(format!(
            "only 3-D RGB tables supported (got {dimensions}D)"
        )));
    }
    let n = divisions;
    if n == 0 || n > 256 {
        return Err(DcpError::TableDecode(format!(
            "implausible table divisions {n}"
        )));
    }
    let count = (n as usize).pow(3);
    if off + count * 6 > raw.len() {
        return Err(DcpError::TableDecode(
            "sample block overruns payload".into(),
        ));
    }

    let mut samples = Vec::with_capacity(count);
    for _ in 0..count {
        samples.push([u16!(), u16!(), u16!()]);
    }

    let primaries = LookColorSpace::from_index(u32!())
        .ok_or_else(|| DcpError::TableDecode("unknown primaries index".into()))?;
    let transfer = LookTransfer::from_index(u32!())
        .ok_or_else(|| DcpError::TableDecode("unknown transfer index".into()))?;
    let gamut = match u32!() {
        0 => LookGamut::Clip,
        1 => LookGamut::Extend,
        _ => return Err(DcpError::TableDecode("unknown gamut index".into())),
    };
    let min_amount = f64!();
    let max_amount = f64!();

    Ok(CobaltRgbTable {
        divisions: n,
        samples,
        primaries,
        transfer,
        gamut,
        min_amount,
        max_amount,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The §1.1 self-closing-`rdf:li` trap: an empty `ShortName`/`SortName`
    /// must parse to `""`, NOT to the *next* element's text (the Group value).
    /// This test fails under a naive regex that skips the self-closing tag.
    #[test]
    fn self_closing_rdf_li_yields_empty_short_and_sort_name() {
        let xmp = br#"<?xpacket?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF>
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:PresetType="Look"
   crs:Cluster="Cobalt-Image"
   crs:UUID="369CEFB650E07246A4C925B6449A0E01"
   crs:SupportsAmount="True"
   crs:CameraModelRestriction=""
   crs:CameraProfile="Cobalt Modular"
   crs:RGBTable="B83D0CA5772B34B5B26CE93ABEA395F6"
   crs:ProcessVersion="15.4">
   <crs:Name><rdf:Alt><rdf:li xml:lang="x-default">Pentax 645D Portrait</rdf:li></rdf:Alt></crs:Name>
   <crs:ShortName><rdf:Alt><rdf:li xml:lang="x-default"/></rdf:Alt></crs:ShortName>
   <crs:SortName><rdf:Alt><rdf:li xml:lang="x-default"/></rdf:Alt></crs:SortName>
   <crs:Group><rdf:Alt><rdf:li xml:lang="x-default">Cobalt CCD fever v3.0</rdf:li></rdf:Alt></crs:Group>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        let look = parse_look_xmp_bytes(xmp, PathBuf::from("t.xmp"), "id".into()).unwrap();
        assert_eq!(look.name, "Pentax 645D Portrait");
        assert_eq!(look.group, "Cobalt CCD fever v3.0");
        assert_eq!(look.short_name, "");
        assert_eq!(look.sort_name, "");
    }

    /// Round-trip a tiny synthetic RGB table through the decoder to prove the
    /// base85 + zlib + delta-ramp path is self-consistent.
    #[test]
    fn decode_rgb_table_round_trip() {
        use flate2::Compression;
        use flate2::write::ZlibEncoder;
        use std::io::Write;

        let n: u32 = 4;
        let mut raw = Vec::new();
        raw.extend_from_slice(&1u32.to_le_bytes()); // type
        raw.extend_from_slice(&1u32.to_le_bytes()); // version
        raw.extend_from_slice(&3u32.to_le_bytes()); // dimensions
        raw.extend_from_slice(&n.to_le_bytes()); // divisions
        let nop = |i: u32| (i * 0xFFFF + (n >> 1)) / (n - 1);
        for ri in 0..n {
            for gi in 0..n {
                for bi in 0..n {
                    // deltas of zero -> identity table
                    raw.extend_from_slice(&0u16.to_le_bytes());
                    raw.extend_from_slice(&0u16.to_le_bytes());
                    raw.extend_from_slice(&0u16.to_le_bytes());
                    let _ = (nop(ri), nop(gi), nop(bi));
                }
            }
        }
        raw.extend_from_slice(&2u32.to_le_bytes()); // primaries = ProPhoto
        raw.extend_from_slice(&2u32.to_le_bytes()); // gamma = 1.8
        raw.extend_from_slice(&0u32.to_le_bytes()); // gamut = clip
        raw.extend_from_slice(&0.2f64.to_le_bytes());
        raw.extend_from_slice(&1.5f64.to_le_bytes());

        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&raw).unwrap();
        let z = enc.finish().unwrap();
        let mut block = Vec::new();
        block.extend_from_slice(&(raw.len() as u32).to_le_bytes());
        block.extend_from_slice(&z);

        // base85-encode the block using a tiny encoder for this test.
        let encoded = base85_encode_for_test(&block);
        let table = decode_rgb_table(encoded.as_bytes()).unwrap();
        assert_eq!(table.divisions, 4);
        assert_eq!(table.samples.len(), 64);
        assert_eq!(table.primaries, LookColorSpace::ProPhoto);
        assert_eq!(table.transfer, LookTransfer::Gamma18);
        // identity table: every sample equals the ramp point -> black == 0
        assert_eq!(table.black(), [0.0, 0.0, 0.0]);
        // at (3,3,3) identity -> (1,1,1)
        let w = table.sample(3, 3, 3);
        for v in w {
            assert!((v - 1.0).abs() < 1e-4, "white corner {w:?} not identity");
        }
    }

    /// Minimal base85 encoder matching `decode_base85`: full 4-byte groups
    /// emit 5 chars; a trailing partial group of `rem` bytes emits `rem + 1`
    /// chars (the decoder emits `phase - 1` trailing bytes). This mirrors
    /// Adobe's `dng_big_table` encoding so the round trip is exact.
    fn base85_encode_for_test(data: &[u8]) -> String {
        // Build the reverse map from KDECODE.
        let mut rev = [0u8; 256];
        for (i, &d) in KDECODE.iter().enumerate() {
            if d <= 85 {
                rev[d as usize] = (i + 32) as u8;
            }
        }
        let mut out = String::new();
        let mut idx = 0;
        while idx + 4 <= data.len() {
            let v = u32::from_le_bytes(data[idx..idx + 4].try_into().unwrap());
            idx += 4;
            out.push_str(&emit_chars(v, 5, &rev));
        }
        let rem = data.len() - idx;
        if rem > 0 {
            let mut last = [0u8; 4];
            last[..rem].copy_from_slice(&data[idx..]);
            let v = u32::from_le_bytes(last);
            out.push_str(&emit_chars(v, rem + 1, &rev));
        }
        out
    }

    fn emit_chars(v: u32, nchars: usize, rev: &[u8; 256]) -> String {
        // Least-significant base85 digit first, matching `decode_base85`
        // (phase 1 = 85^0 = least significant). The earlier MSB-first version
        // disagreed with the decoder and corrupted the zlib stream.
        let mut x = v as u64;
        let mut s = String::new();
        for _ in 0..nchars {
            let d = (x % 85) as u8;
            x /= 85;
            s.push(rev[d as usize] as char);
        }
        s
    }

    /// Asset-dependent acceptance tests: run with `RAPIDRAW_TEST_ASSETS=1`.
    #[test]
    #[ignore]
    fn all_supplied_looks_parse_and_pass_acceptance() {
        if std::env::var("RAPIDRAW_TEST_ASSETS").as_deref() != Ok("1") {
            eprintln!("SKIPPED: RAPIDRAW_TEST_ASSETS not set — cannot run Look acceptance tests");
            return;
        }
        let dir = std::env::var("RAPIDRAW_LOOKS_DIR")
            .unwrap_or_else(|_| "/Users/harrisontucker/Downloads/Cobalt_CCD_Fever_3".to_string());
        if !std::path::Path::new(&dir).is_dir() {
            eprintln!("SKIPPED: Looks directory not found at {dir}");
            return;
        }
        let mut found = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("xmp") {
                continue;
            }
            let look = parse_look_xmp(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            found += 1;
            assert_eq!(
                look.required_base_profile,
                "Cobalt Modular",
                "{}",
                path.display()
            );
            assert_eq!(look.camera_model_restriction, None, "{}", path.display());
            assert_eq!(look.short_name, "", "{}", path.display());
            assert_eq!(look.sort_name, "", "{}", path.display());
            // Every supplied Look decodes its embedded RGB table (Route A).
            match &look.table {
                LookTableSource::Decoded(t) => {
                    assert_eq!(t.divisions, 32, "{}", path.display());
                    assert_eq!(t.samples.len(), 32usize.pow(3), "{}", path.display());
                    assert_eq!(t.black(), [0.0, 0.0, 0.0], "{}", path.display());
                }
                _ => panic!("{}: table did not decode", path.display()),
            }
        }
        assert_eq!(found, 17, "expected 17 supplied Looks, found {found}");
    }
}
