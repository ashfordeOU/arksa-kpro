// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! The signed calibration lineage of a mission's simulator.
//!
//! A lineage directory holds:
//!
//! - `entries.jsonl`: one entry (`ks-simcal-lineage/1`) per recalibration, each naming the
//!   SHA-256 of the entry before it (the first names [`GENESIS`]) and carrying its own
//!   SHA-256 over its body; an entry records the mission, telemetry and plugin files by
//!   SHA-256, every plugin's residuals and status, and for a hosted plugin the request and
//!   the verified verdict's outcome;
//! - `requests/<seq>-<plugin>.json` and `verdicts/<seq>-<plugin>.json`: the exact request
//!   sent and the signed verdict received, verbatim;
//! - `signature.json`: the directory signed as a whole with the Arksa signer key (Kshana
//!   Pro's signed dossier), renewed at every append.
//!
//! [`verify_dir`] re-checks the chain, every entry hash, every request and verdict file
//! against its recorded hash, every verdict's signature against the trusted service key(s)
//! and its outcome against the entry, and the directory signature. Editing, removing,
//! reordering or adding an entry or a file is detected.
//!
//! A plugin's status: `calibrated` (hosted, converged), `not-converged` (hosted, the
//! service did not converge: the entry records it and nothing is taken as calibrated),
//! `consistent` (local model within tolerance), `recalibration-needed` (local model drifted
//! and no hosted calibration method serves its model family: the residuals are recorded as
//! an open item), or `no-comparison`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use kshana_pro::remote_verdict::{verify as verify_verdict, SignedVerdict, VerdictTransport};

use super::hosted::{self, Outcome};
use super::mission::{Mission, SourceRef};
use super::plugin::SimPlugin;
use super::residuals::{plugin_residuals, PluginResiduals, Verdict};
use super::{sha256_hex, MODELLED};

/// Identifies a lineage entry.
pub const FORMAT: &str = "ks-simcal-lineage/1";
/// The `prev_entry_sha256` of the first entry.
pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";
/// The entries file.
pub const ENTRIES: &str = "entries.jsonl";

/// A plugin's calibration status in one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Calibrated,
    NotConverged,
    Consistent,
    RecalibrationNeeded,
    NoComparison,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Calibrated => "calibrated",
            Status::NotConverged => "not-converged",
            Status::Consistent => "consistent",
            Status::RecalibrationNeeded => "recalibration-needed",
            Status::NoComparison => "no-comparison",
        }
    }
}

/// The hosted part of a plugin's record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostedRecord {
    pub method: String,
    pub request_file: String,
    pub request_sha256: String,
    pub verdict_file: String,
    pub verdict_file_sha256: String,
    /// The verdict's own hash over its signed body.
    pub verdict_sha256: String,
    pub service: String,
    pub key_id: String,
    pub evidence_class: String,
    pub verdict_utc: String,
    pub outcome: Outcome,
}

/// One plugin in one entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginRecord {
    pub plugin: String,
    pub kind: String,
    pub status: Status,
    pub sources: Vec<SourceRef>,
    pub residuals: PluginResiduals,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hosted: Option<HostedRecord>,
    pub note: String,
}

/// One recalibration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub format: String,
    pub seq: u64,
    pub prev_entry_sha256: String,
    pub utc: String,
    pub mission: String,
    pub mission_sha256: String,
    pub telemetry_sha256: String,
    pub plugins: Vec<PluginRecord>,
    pub evidence_class: String,
    /// SHA-256 over the entry with this field empty.
    pub entry_sha256: String,
}

impl Entry {
    /// The hash of the entry's body (this entry with `entry_sha256` empty).
    pub fn body_sha256(&self) -> String {
        let mut e = self.clone();
        e.entry_sha256.clear();
        sha256_hex(&serde_json::to_vec(&e).unwrap_or_default())
    }
}

/// Read the entries of a lineage directory (none when it has no entries file).
pub fn read_entries(dir: &Path) -> Result<Vec<Entry>, String> {
    let p = dir.join(ENTRIES);
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(format!("{}: {e}", p.display())),
    };
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| serde_json::from_str(l).map_err(|e| format!("{ENTRIES} line {}: {e}", i + 1)))
        .collect()
}

/// The result of checking a lineage directory.
#[derive(Debug, Clone, Default)]
pub struct Check {
    pub entries: usize,
    pub head: Option<String>,
    pub signed: bool,
    pub signer: Option<String>,
    pub problems: Vec<String>,
}

impl Check {
    pub fn passed(&self) -> bool {
        self.problems.is_empty()
    }
}

fn file_sha(dir: &Path, rel: &str) -> Result<(Vec<u8>, String), String> {
    let bytes = std::fs::read(dir.join(rel)).map_err(|e| format!("{rel}: {e}"))?;
    let h = sha256_hex(&bytes);
    Ok((bytes, h))
}

fn check_hosted(
    dir: &Path,
    seq: u64,
    r: &PluginRecord,
    h: &HostedRecord,
    keys: &str,
) -> Vec<String> {
    let mut p = Vec::new();
    let at = format!("entry {seq}, plugin '{}'", r.plugin);
    match file_sha(dir, &h.request_file) {
        Ok((_, s)) if s == h.request_sha256 => {}
        Ok(_) => p.push(format!("{at}: {} changed", h.request_file)),
        Err(e) => p.push(format!("{at}: {e}")),
    }
    let bytes = match file_sha(dir, &h.verdict_file) {
        Ok((b, s)) if s == h.verdict_file_sha256 => b,
        Ok(_) => {
            p.push(format!("{at}: {} changed", h.verdict_file));
            return p;
        }
        Err(e) => {
            p.push(format!("{at}: {e}"));
            return p;
        }
    };
    let v: SignedVerdict = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            p.push(format!(
                "{at}: the verdict file is not a signed verdict: {e}"
            ));
            return p;
        }
    };
    if keys.trim().is_empty() {
        p.push(format!(
            "{at}: no trusted service key given; the verdict was not checked"
        ));
    } else {
        for e in verify_verdict(&v, keys) {
            p.push(format!("{at}: verdict: {e}"));
        }
    }
    if v.method != hosted::SELFCAL || h.method != hosted::SELFCAL {
        p.push(format!("{at}: not a {} verdict", hosted::SELFCAL));
    }
    if v.verdict_sha256 != h.verdict_sha256 {
        p.push(format!("{at}: the verdict hash differs from the entry"));
    }
    match hosted::outcome(&v) {
        Ok(o) if o == h.outcome => {}
        Ok(_) => p.push(format!(
            "{at}: the verdict's outcome differs from the entry"
        )),
        Err(e) => p.push(format!("{at}: {e}")),
    }
    let want = if h.outcome.converged {
        Status::Calibrated
    } else {
        Status::NotConverged
    };
    if r.status != want {
        p.push(format!(
            "{at}: status {} does not match the verdict",
            r.status.as_str()
        ));
    }
    p
}

/// Check a lineage directory (module docs). `trusted_verdict_keys`: the service public
/// key(s), comma separated. `trust`: the Arksa signer key(s) the directory signature must be
/// made with (empty: the signature is checked but its key is not pinned).
pub fn verify_dir(
    dir: &Path,
    trusted_verdict_keys: &str,
    trust: &[[u8; 32]],
) -> Result<Check, String> {
    let entries = read_entries(dir)?;
    let mut c = Check {
        entries: entries.len(),
        head: entries.last().map(|e| e.entry_sha256.clone()),
        ..Default::default()
    };
    if entries.is_empty() {
        c.problems
            .push(format!("{}: no lineage entries", dir.display()));
    }
    let mut prev = GENESIS.to_string();
    for (i, e) in entries.iter().enumerate() {
        let seq = i as u64 + 1;
        if e.format != FORMAT {
            c.problems
                .push(format!("entry {seq}: format '{}'", e.format));
        }
        if e.seq != seq {
            c.problems
                .push(format!("entry {seq}: records sequence number {}", e.seq));
        }
        if e.prev_entry_sha256 != prev {
            c.problems
                .push(format!("entry {seq}: does not name the entry before it"));
        }
        if e.body_sha256() != e.entry_sha256 {
            c.problems
                .push(format!("entry {seq}: its hash does not match its body"));
        }
        for r in &e.plugins {
            match (&r.hosted, r.status) {
                (Some(h), _) => {
                    c.problems
                        .extend(check_hosted(dir, e.seq, r, h, trusted_verdict_keys))
                }
                (None, Status::Calibrated | Status::NotConverged) => c.problems.push(format!(
                    "entry {seq}, plugin '{}': status {} without a hosted verdict",
                    r.plugin,
                    r.status.as_str()
                )),
                _ => {}
            }
        }
        prev = e.entry_sha256.clone();
    }
    if dir
        .join(kshana_pro::signed_dossier::SIGNATURE_FILE)
        .exists()
    {
        let v = kshana_pro::signed_dossier::verify(dir, trust).map_err(|e| e.to_string())?;
        c.signed = true;
        c.signer = Some(format!("{} (key {})", v.signer, v.key_id));
        c.problems
            .extend(v.problems.iter().map(|p| format!("signature: {p}")));
    } else if !trust.is_empty() {
        c.problems
            .push("the lineage is not signed, but a signer key was given to check".into());
    }
    Ok(c)
}

/// How a recalibration is run and recorded.
pub struct RecalOptions<'a> {
    pub transport: &'a dyn VerdictTransport,
    /// Licence token for the verdict service.
    pub bearer: &'a str,
    /// The service public key(s), comma separated.
    pub trusted_verdict_keys: &'a str,
    /// The entry's time stamp (the library reads no clock).
    pub utc: &'a str,
    /// Sign the directory with this Arksa seed, as this signer.
    pub signer: Option<(&'a [u8; 32], &'a str)>,
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Recalibrate a mission's simulator and append the entry to the lineage in `dir`.
///
/// Every hosted plugin's request is sent and its answer checked before anything is written:
/// a refused or unreachable verdict leaves the lineage as it was. An existing lineage must
/// check (chain, hashes and, when signed, its signature) before an entry is added.
pub fn recalibrate_mission(
    m: &Mission,
    plugins: &[Box<dyn SimPlugin>],
    dir: &Path,
    o: &RecalOptions,
) -> Result<Entry, String> {
    let existing = read_entries(dir)?;
    if !existing.is_empty() {
        let c = verify_dir(dir, o.trusted_verdict_keys, &[])?;
        if !c.passed() {
            return Err(format!(
                "the lineage in {} does not check, nothing was added: {}",
                dir.display(),
                c.problems.join("; ")
            ));
        }
    }
    let seq = existing.len() as u64 + 1;
    let mut records = Vec::new();
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for p in plugins {
        let (res, _) = plugin_residuals(m, p.as_ref());
        let sources = p.sources();
        let (status, hosted, note) = if p.hosted().is_some() {
            let r =
                hosted::recalibrate(o.transport, o.bearer, m, p.as_ref(), o.trusted_verdict_keys)?;
            let req_file = format!("requests/{seq:04}-{}.json", p.id());
            let ver_file = format!("verdicts/{seq:04}-{}.json", p.id());
            let ver_bytes = serde_json::to_vec_pretty(&r.verdict).map_err(|e| e.to_string())?;
            let status = if r.outcome.converged {
                Status::Calibrated
            } else {
                Status::NotConverged
            };
            let note = if r.outcome.converged {
                format!(
                    "Recalibrated by the hosted {} method; the calibrated parameters are {}.",
                    hosted::SELFCAL,
                    r.verdict.evidence_class
                )
            } else {
                "The hosted calibration did not converge; nothing is taken as calibrated.".into()
            };
            let rec = HostedRecord {
                method: hosted::SELFCAL.into(),
                request_file: req_file.clone(),
                request_sha256: r.request_sha256.clone(),
                verdict_file: ver_file.clone(),
                verdict_file_sha256: sha256_hex(&ver_bytes),
                verdict_sha256: r.verdict.verdict_sha256.clone(),
                service: r.verdict.service.clone(),
                key_id: r.verdict.key_id.clone(),
                evidence_class: r.verdict.evidence_class.clone(),
                verdict_utc: r.verdict.utc.clone(),
                outcome: r.outcome,
            };
            files.push((req_file, r.request_body.into_bytes()));
            files.push((ver_file, ver_bytes));
            (status, Some(rec), note)
        } else {
            match res.verdict {
                Verdict::Drifted => (
                    Status::RecalibrationNeeded,
                    None,
                    format!(
                        "Drifted beyond tolerance. No hosted calibration method serves kind '{}'; the residuals are recorded as an open item.",
                        p.kind()
                    ),
                ),
                Verdict::NoComparison => (
                    Status::NoComparison,
                    None,
                    "No telemetry sample fell inside the simulated span.".into(),
                ),
                _ => (
                    Status::Consistent,
                    None,
                    "Within tolerance; no recalibration needed.".into(),
                ),
            }
        };
        records.push(PluginRecord {
            plugin: p.id().to_string(),
            kind: p.kind().to_string(),
            status,
            sources,
            residuals: res,
            hosted,
            note,
        });
    }
    let mut entry = Entry {
        format: FORMAT.into(),
        seq,
        prev_entry_sha256: existing
            .last()
            .map(|e| e.entry_sha256.clone())
            .unwrap_or_else(|| GENESIS.into()),
        utc: o.utc.into(),
        mission: m.file.mission.clone(),
        mission_sha256: m.mission_sha256.clone(),
        telemetry_sha256: m.telemetry_sha256.clone(),
        plugins: records,
        evidence_class: MODELLED.into(),
        entry_sha256: String::new(),
    };
    entry.entry_sha256 = entry.body_sha256();

    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (rel, bytes) in &files {
        write(&dir.join(rel), bytes)?;
    }
    let mut text = std::fs::read_to_string(dir.join(ENTRIES)).unwrap_or_default();
    text.push_str(&serde_json::to_string(&entry).map_err(|e| e.to_string())?);
    text.push('\n');
    write(&dir.join(ENTRIES), text.as_bytes())?;
    if let Some((seed, name)) = o.signer {
        kshana_pro::signed_dossier::sign(dir, seed, name, o.utc).map_err(|e| e.to_string())?;
    }
    Ok(entry)
}

/// A short Markdown summary of an entry.
pub fn summary_md(e: &Entry) -> String {
    let mut s = format!(
        "# Calibration lineage entry {} — {}\n\nUTC {} · entry {} · previous {}\n\nEvidence class: {} (simulator predictions and calibrated parameters).\n\n| plugin | kind | status | note |\n|---|---|---|---|\n",
        e.seq, e.mission, e.utc, e.entry_sha256, e.prev_entry_sha256, e.evidence_class
    );
    for r in &e.plugins {
        s.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            r.plugin,
            r.kind,
            r.status.as_str(),
            r.note
        ));
    }
    for r in &e.plugins {
        if let Some(h) = &r.hosted {
            s.push_str(&format!(
                "\n## {} — hosted {}\n\nVerdict {} from {} (key {}, {}), converged: {}",
                r.plugin,
                h.method,
                h.verdict_sha256,
                h.service,
                h.key_id,
                h.evidence_class,
                h.outcome.converged
            ));
            if let (Some(a), Some(b)) = (h.outcome.fidelity_before, h.outcome.fidelity_after) {
                s.push_str(&format!(", fidelity {a} → {b}"));
            }
            s.push_str("\n\n| parameter | value | standard uncertainty |\n|---|---|---|\n");
            for p in &h.outcome.parameters {
                s.push_str(&format!("| {} | {} | {} |\n", p.name, p.value, p.std_err));
            }
        }
    }
    s
}
