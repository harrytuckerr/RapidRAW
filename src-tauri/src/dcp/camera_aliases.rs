//! Camera-model matching rules and the manufacturer alias table.
//!
//! Normative behaviour from `COBALT_DCP_SPEC.md` §5.2. The rule set is exact:
//! no fuzzy/edit-distance matching is used here (the `fuzzy-matcher` crate is
//! deliberately NOT used — it exists for other features). A wrong match
//! silently produces wrong colour, which is worse than no match, so if no rule
//! hits the profile is simply not offered for that body.
//!
//! The alias table lives in exactly one place so the registry, the pairing
//! logic and the camera-index key all agree.

/// Manufacturer aliases, each mapping to its canonical form.
///
/// Applied case-insensitively to *both* sides of a comparison per §5.2 rule 4.
/// `canonical` is the token a profile's camera string is normalised to.
pub const ALIASES: &[(&str, &str)] = &[
    // Canonical -> canonical entries are included so `canonical_manufacturer`
    // is total over every manufacturer the table knows about.
    ("fujifilm", "fujifilm"),
    ("fuji", "fujifilm"),
    ("nikon", "nikon"),
    ("nikon corporation", "nikon"),
    ("canon", "canon"),
    ("canon inc.", "canon"),
    ("panasonic", "panasonic"),
    ("lumix", "panasonic"),
    ("olympus", "olympus"),
    ("om digital solutions", "olympus"),
    ("pentax", "pentax"),
    ("ricoh imaging", "pentax"),
    ("leica", "leica"),
    ("leica camera ag", "leica"),
    ("sony", "sony"),
    ("hasselblad", "hasselblad"),
    ("phase one", "phase one"),
];

/// Collapse runs of internal whitespace to a single space, and trim.
fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Map a leading manufacturer token to its canonical form, case-insensitively.
///
/// The manufacturer is conventionally the first word (or words) of a camera
/// model string, so we attempt to strip a matching alias from the front and
/// replace it with the canonical token. Multi-word aliases such as
/// "NIKON CORPORATION" and "OM Digital Solutions" are handled because we match
/// the whole prefix, not a single word.
fn canonicalise_manufacturer_prefix(s: &str) -> String {
    let lower = s.to_lowercase();
    // Longest first so "nikon corporation" beats "nikon".
    let mut best: Option<(usize, &str)> = None;
    for (alias, canonical) in ALIASES {
        if lower.starts_with(alias) {
            let rest = &s[alias.len()..];
            if rest.is_empty() || rest.starts_with(' ') {
                let len = alias.len();
                if best.map(|(bl, _)| len > bl).unwrap_or(true) {
                    best = Some((len, canonical));
                }
            }
        }
    }
    match best {
        Some((len, canonical)) => format!("{canonical}{}", &s[len..]),
        None => s.to_string(),
    }
}

/// Normalise a camera-model string to a canonical, indexable key.
///
/// Applies §5.2 rule 2 (trim + collapse whitespace + case-insensitive) and
/// rule 4 (manufacturer aliases) so that distinct-but-equivalent spellings of
/// one body collapse to a single key. The result is the key under which the
/// registry indexes DCPs by camera.
pub fn normalise_camera_model(s: &str) -> String {
    let collapsed = collapse_whitespace(s);
    let canonical = canonicalise_manufacturer_prefix(&collapsed);
    canonical.to_lowercase()
}

/// Decide whether a DCP's `UniqueCameraModel` matches a query camera string,
/// per §5.2 rules 1-4, stopping at the first hit.
///
/// - Rule 1: exact, case-sensitive.
/// - Rule 2: exact, case-insensitive, after collapsing whitespace/trimming.
/// - Rule 3: case-insensitive against `"{make} {model}"` — equivalent to rule 2
///   when the query is already a composed `make model` string, which is how the
///   pipeline supplies it, so it is covered by the normalised comparison.
/// - Rule 4: case-insensitive with manufacturer aliases applied to both sides.
pub fn camera_model_matches(dcp_model: &str, query: &str) -> bool {
    if dcp_model == query {
        return true; // rule 1
    }
    // rules 2-4: alias-normalised, whitespace-normalised, case-insensitive
    normalise_camera_model(dcp_model) == normalise_camera_model(query)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- §5.2 rule 1: exact, case-sensitive ---

    #[test]
    fn rule1_exact_case_sensitive_match() {
        assert!(camera_model_matches("Fujifilm X-Pro2", "Fujifilm X-Pro2"));
    }

    #[test]
    fn rule1_case_sensitive_mismatch_still_matches_via_rule2() {
        // Different case is NOT rule 1, but rule 2 (case-insensitive) catches it.
        assert!(camera_model_matches("Fujifilm X-Pro2", "FUJIFILM X-PRO2"));
    }

    // --- §5.2 rule 2: case-insensitive, whitespace-collapsed ---

    #[test]
    fn rule2_case_insensitive_match() {
        assert!(camera_model_matches("Fujifilm X-Pro2", "fujifilm x-pro2"));
    }

    #[test]
    fn rule2_collapses_internal_whitespace() {
        assert!(camera_model_matches("Fujifilm   X-Pro2", "Fujifilm X-Pro2"));
    }

    #[test]
    fn rule2_trims_surrounding_whitespace() {
        assert!(camera_model_matches(" Fujifilm X-Pro2 ", "Fujifilm X-Pro2"));
    }

    // --- §5.2 rule 3: composed "{make} {model}" ---

    #[test]
    fn rule3_make_and_model_composition() {
        // DCP stores the full unique camera model; the query is make + model.
        assert!(camera_model_matches("Fujifilm X-Pro2", "Fujifilm X-Pro2"));
    }

    // --- §5.2 rule 4: manufacturer aliases, both sides ---

    #[test]
    fn alias_fujifilm_fuji() {
        assert!(camera_model_matches("Fujifilm X-Pro2", "Fuji X-Pro2"));
        assert!(camera_model_matches("Fuji X-Pro2", "Fujifilm X-Pro2"));
    }

    #[test]
    fn alias_fujifilm_fuji_uppercase() {
        assert!(camera_model_matches("Fujifilm X-Pro2", "FUJI X-Pro2"));
    }

    #[test]
    fn alias_nikon_corporation() {
        assert!(camera_model_matches("Nikon D850", "NIKON CORPORATION D850"));
        assert!(camera_model_matches("NIKON CORPORATION D850", "Nikon D850"));
    }

    #[test]
    fn alias_canon_inc() {
        assert!(camera_model_matches("Canon EOS R5", "Canon Inc. EOS R5"));
    }

    #[test]
    fn alias_panasonic_lumix() {
        assert!(camera_model_matches("Panasonic DC-S5", "Lumix DC-S5"));
        assert!(camera_model_matches("Lumix DC-S5", "Panasonic DC-S5"));
    }

    #[test]
    fn alias_olympus_om_digital() {
        assert!(camera_model_matches(
            "Olympus OM-1",
            "OM Digital Solutions OM-1"
        ));
    }

    #[test]
    fn alias_pentax_ricoh() {
        assert!(camera_model_matches("Pentax K-1", "RICOH IMAGING K-1"));
        assert!(camera_model_matches("PENTAX K-1", "Pentax K-1"));
    }

    #[test]
    fn alias_leica_camera_ag() {
        assert!(camera_model_matches("Leica M11", "Leica Camera AG M11"));
    }

    #[test]
    fn alias_sony_uppercase() {
        assert!(camera_model_matches("Sony A7R IV", "SONY A7R IV"));
    }

    #[test]
    fn alias_hasselblad() {
        assert!(camera_model_matches("Hasselblad X2D", "Hasselblad X2D"));
    }

    #[test]
    fn alias_phase_one() {
        assert!(camera_model_matches("Phase One IQ4", "Phase One IQ4"));
    }

    // --- negative: distinct bodies must NOT match ---

    #[test]
    fn different_camera_does_not_match() {
        assert!(!camera_model_matches("Fujifilm X-Pro2", "Sony A7R IV"));
        assert!(!camera_model_matches("Nikon D850", "Canon EOS R5"));
    }

    #[test]
    fn unknown_manufacturer_not_alias_matched() {
        // A manufacturer the alias table does not know must be matched literally.
        assert!(!camera_model_matches("Hasselblad X2D", "Fuji X2D"));
    }

    // --- normalise_camera_model key stability ---

    #[test]
    fn normalise_aliases_to_shared_key() {
        let a = normalise_camera_model("Fujifilm X-Pro2");
        let b = normalise_camera_model("FUJI X-Pro2");
        let c = normalise_camera_model("  Fujifilm  X-Pro2  ");
        assert_eq!(a, "fujifilm x-pro2");
        assert_eq!(a, b);
        assert_eq!(a, c);
    }

    #[test]
    fn normalise_om_digital_prefix() {
        assert_eq!(
            normalise_camera_model("OM Digital Solutions OM-1"),
            "olympus om-1"
        );
    }

    #[test]
    fn normalise_ricoh_imaging_prefix() {
        assert_eq!(normalise_camera_model("RICOH IMAGING K-1"), "pentax k-1");
    }
}
