// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! The dossier's space-safety section, and the `mission decision`, `mission
//! decision-chain`, `mission thresholds` and `mission disposal` commands.
//!
//! A programme may name, under `[space_safety]`, disposal inputs ([`crate::disposal`]),
//! collision-avoidance decision inputs ([`crate::decision::record`]) and one threshold
//! study ([`crate::decision::policy`]):
//!
//! ```toml
//! [space_safety]
//! disposal = ["disposal.toml"]
//! decision = ["decisions/ev-001.toml", "decisions/ev-002.toml"]   # in time order
//!
//! [space_safety.thresholds]
//! events = "events.csv"
//! thresholds = [1e-4, 1e-5, 3e-7]
//! period_days = 180.0          # optional
//! default_post_pc = 0.0        # optional
//! ```
//!
//! Every file is read once, as bytes, checked before anything runs, hashed into the
//! programme's request hash, and copied into `space-safety/` beside its result document.
//! A programme without `[space_safety]` gives a dossier byte-for-byte the same as before.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::decision::{policy, record};
use crate::disposal::{self, rules::RuleText};
use crate::mbse::evidence::{sha256_hex, SourceFile};
use kshana::api::KshanaError;

/// The directory of the dossier the section's files go in.
pub const DIR: &str = "space-safety";
/// The section's honesty label.
pub const LABEL: &str = "MODELLED - collision-avoidance decision records as the operator stated them, a threshold comparison over the listed events and a disposal-compliance screening against quoted rule texts. Not a licence decision and not acceptance by any authority.";

fn err(msg: impl Into<String>) -> KshanaError {
    KshanaError::InvalidInput(msg.into())
}

/// `[space_safety]` in the programme file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpaceSafetySpec {
    #[serde(default)]
    pub disposal: Vec<String>,
    #[serde(default)]
    pub decision: Vec<String>,
    #[serde(default)]
    pub thresholds: Option<ThresholdSpec>,
}

/// `[space_safety.thresholds]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThresholdSpec {
    pub events: String,
    pub thresholds: Vec<f64>,
    #[serde(default)]
    pub period_days: Option<f64>,
    #[serde(default)]
    pub default_post_pc: f64,
}

/// One input file, with its exact bytes.
#[derive(Debug, Clone)]
pub struct InputFile {
    pub file_name: String,
    pub path_display: String,
    pub bytes: Vec<u8>,
    pub sha256: String,
}

impl InputFile {
    fn text(&self) -> Result<&str, KshanaError> {
        std::str::from_utf8(&self.bytes)
            .map_err(|_| err(format!("{} is not UTF-8", self.path_display)))
    }
    fn stem(&self) -> &str {
        self.file_name
            .rsplit_once('.')
            .map(|(s, _)| s)
            .unwrap_or(&self.file_name)
    }
}

/// The loaded `[space_safety]` inputs.
#[derive(Debug, Clone)]
pub struct SpaceSafetyInputs {
    pub disposal: Vec<InputFile>,
    pub decision: Vec<InputFile>,
    pub thresholds: Option<(InputFile, policy::Study)>,
}

impl SpaceSafetyInputs {
    /// Every input file, in a fixed order, with the kind it is.
    pub fn files(&self) -> Vec<(&'static str, &InputFile)> {
        let mut v: Vec<(&'static str, &InputFile)> = Vec::new();
        v.extend(self.disposal.iter().map(|f| ("disposal", f)));
        v.extend(self.decision.iter().map(|f| ("decision", f)));
        v.extend(self.thresholds.iter().map(|(f, _)| ("thresholds", f)));
        v
    }

    /// Load and check every file the spec names (paths relative to `base`). Nothing is
    /// written.
    pub fn load(base: &Path, spec: SpaceSafetySpec) -> Result<SpaceSafetyInputs, KshanaError> {
        let read = |p: &str, what: &str| -> Result<InputFile, KshanaError> {
            let file_name = Path::new(p)
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
                .ok_or_else(|| err(format!("[space_safety] '{p}' is not a file path")))?;
            let bytes = std::fs::read(base.join(p)).map_err(|e| {
                err(format!(
                    "could not read the [space_safety] {what} file {p}: {e}"
                ))
            })?;
            Ok(InputFile {
                file_name,
                path_display: p.to_string(),
                sha256: sha256_hex(&bytes),
                bytes,
            })
        };
        let disposal = spec
            .disposal
            .iter()
            .map(|p| read(p, "disposal"))
            .collect::<Result<Vec<_>, _>>()?;
        let decision = spec
            .decision
            .iter()
            .map(|p| read(p, "decision"))
            .collect::<Result<Vec<_>, _>>()?;
        let thresholds = match spec.thresholds {
            Some(t) => Some((
                read(&t.events, "threshold events")?,
                policy::Study {
                    thresholds: t.thresholds,
                    period_days: t.period_days,
                    default_post_pc: t.default_post_pc,
                },
            )),
            None => None,
        };
        let inputs = SpaceSafetyInputs {
            disposal,
            decision,
            thresholds,
        };
        if inputs.files().is_empty() {
            return Err(err("[space_safety] names no file"));
        }
        // Each file and each result document is copied flat into space-safety/.
        let mut names: Vec<String> = Vec::new();
        for (kind, f) in inputs.files() {
            for n in [f.file_name.clone(), format!("{}.{kind}.json", f.stem())] {
                if names.iter().any(|x| x.eq_ignore_ascii_case(&n)) {
                    return Err(err(format!(
                        "[space_safety]: two files would be written as {DIR}/{n}; give the inputs distinct names"
                    )));
                }
                names.push(n);
            }
        }
        // Every input must parse before anything runs.
        let _ = assess(&inputs)?;
        Ok(inputs)
    }
}

/// One disposal assessment in the dossier.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DisposalEntry {
    pub input: SourceFile,
    pub result: SourceFile,
    pub assessment_sha256: String,
    pub title: String,
    pub spacecraft: String,
    pub strategy: String,
    pub lifetime_basis: String,
    pub lifetime_nominal_years: Option<f64>,
    pub lifetime_upper_years: Option<f64>,
    pub lifetime_upper_beyond_horizon: bool,
    pub cross_check: Option<String>,
    pub checks: Vec<disposal::Check>,
    pub counts: disposal::Counts,
}

/// One decision record in the dossier.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DecisionEntry {
    pub input: SourceFile,
    pub result: SourceFile,
    pub record_sha256: String,
    pub prev_record_sha256: Option<String>,
    pub event_id: String,
    pub tca_utc: String,
    pub pc: f64,
    pub miss_distance_m: f64,
    pub policy: String,
    pub manoeuvre_pc: f64,
    pub band: String,
    pub chosen_option: String,
    pub chosen_kind: String,
    pub consistency: String,
    pub mitigation: String,
    pub hours_before_tca: f64,
    pub decided_by: String,
    pub rationale: String,
    pub notes: Vec<String>,
}

/// The threshold comparison in the dossier.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ThresholdEntry {
    pub input: SourceFile,
    pub result: SourceFile,
    pub comparison_sha256: String,
    pub events: usize,
    pub risk_without_manoeuvres: f64,
    pub rows: Vec<policy::Row>,
    pub esa_reference_threshold: Option<f64>,
    pub reading: String,
}

/// The dossier's space-safety section.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SpaceSafetySection {
    pub label: String,
    pub disposal: Vec<DisposalEntry>,
    pub decisions: Vec<DecisionEntry>,
    /// The chain check of the decision records in programme order: `None` when no record
    /// names a previous one.
    pub decision_chain: Option<Vec<String>>,
    pub thresholds: Option<ThresholdEntry>,
    /// Every rule text the section refers to.
    pub rules: Vec<RuleText>,
}

/// A computed section and the files that go with it under `space-safety/`.
pub struct Assessed {
    pub section: SpaceSafetySection,
    /// (path relative to the dossier, bytes).
    pub files: Vec<(String, Vec<u8>)>,
}

fn source(path: String, bytes: &[u8]) -> SourceFile {
    SourceFile {
        path,
        sha256: sha256_hex(bytes),
    }
}

/// Assess every input. Pure: reads no file and writes none.
pub fn assess(inputs: &SpaceSafetyInputs) -> Result<Assessed, KshanaError> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut rule_ids: Vec<String> = Vec::new();
    let mut put = |name: String, bytes: Vec<u8>| -> SourceFile {
        let path = format!("{DIR}/{name}");
        let s = source(path.clone(), &bytes);
        files.push((path, bytes));
        s
    };

    let mut disposal_entries = Vec::new();
    for f in &inputs.disposal {
        let d = disposal::DisposalInput::parse(f.text()?)
            .map_err(|e| err(format!("[space_safety] {}: {e}", f.path_display)))?;
        let a = disposal::assess(&d, &f.sha256);
        let input = put(f.file_name.clone(), f.bytes.clone());
        let result = put(
            format!("{}.disposal.json", f.stem()),
            (a.to_json() + "\n").into_bytes(),
        );
        rule_ids.extend(a.body.rules.iter().map(|r| r.id.to_string()));
        let l = &a.body.lifetime;
        disposal_entries.push(DisposalEntry {
            input,
            result,
            assessment_sha256: a.assessment_sha256.clone(),
            title: a.body.title.clone(),
            spacecraft: a.body.spacecraft.clone(),
            strategy: a.body.strategy.clone(),
            lifetime_basis: l.basis.clone(),
            lifetime_nominal_years: l.nominal_years,
            lifetime_upper_years: l.upper_years,
            lifetime_upper_beyond_horizon: l.upper_beyond_horizon,
            cross_check: l.cross_check.clone(),
            checks: a.body.checks.clone(),
            counts: a.body.counts.clone(),
        });
    }

    let mut decisions = Vec::new();
    let mut chain_docs = Vec::new();
    for f in &inputs.decision {
        let i = record::DecisionInput::parse(f.text()?)
            .map_err(|e| err(format!("[space_safety] {}: {e}", f.path_display)))?;
        let r = record::build(i, &f.sha256)?;
        let json = r.to_json() + "\n";
        chain_docs.push(
            serde_json::from_str::<serde_json::Value>(&json).map_err(|e| err(e.to_string()))?,
        );
        let input = put(f.file_name.clone(), f.bytes.clone());
        let result = put(format!("{}.decision.json", f.stem()), json.into_bytes());
        let b = &r.body;
        rule_ids.extend(b.findings.rule_refs.iter().cloned());
        let chosen = b
            .input
            .options
            .iter()
            .find(|o| o.id == b.input.decision.chosen_option)
            .expect("validated");
        decisions.push(DecisionEntry {
            input,
            result,
            record_sha256: r.record_sha256.clone(),
            prev_record_sha256: b.input.prev_record_sha256.clone(),
            event_id: b.input.event.event_id.clone(),
            tca_utc: b.input.event.tca_utc.clone(),
            pc: b.input.event.pc,
            miss_distance_m: b.input.event.miss_distance_m,
            policy: b.input.policy.name.clone(),
            manoeuvre_pc: b.input.policy.manoeuvre_pc,
            band: b.findings.band.as_str().into(),
            chosen_option: chosen.id.clone(),
            chosen_kind: chosen.kind.as_str().into(),
            consistency: b.findings.consistency.as_str().into(),
            mitigation: b.findings.mitigation.clone(),
            hours_before_tca: b.findings.hours_before_tca,
            decided_by: format!(
                "{} ({})",
                b.input.decision.decided_by, b.input.decision.role
            ),
            rationale: b.input.decision.rationale.clone(),
            notes: b.findings.notes.clone(),
        });
    }
    let decision_chain = if decisions.iter().any(|d| d.prev_record_sha256.is_some()) {
        Some(record::verify_chain(&chain_docs))
    } else {
        None
    };

    let thresholds = match &inputs.thresholds {
        Some((f, study)) => {
            let events = policy::parse_events(f.text()?)
                .map_err(|e| err(format!("[space_safety] {}: {e}", f.path_display)))?;
            let c = policy::compare(&events, study, &f.sha256)
                .map_err(|e| err(format!("[space_safety] thresholds: {e}")))?;
            let input = put(f.file_name.clone(), f.bytes.clone());
            let result = put(
                format!("{}.thresholds.json", f.stem()),
                (c.to_json() + "\n").into_bytes(),
            );
            rule_ids.extend(c.body.rule_refs.iter().cloned());
            Some(ThresholdEntry {
                input,
                result,
                comparison_sha256: c.comparison_sha256.clone(),
                events: c.body.events,
                risk_without_manoeuvres: c.body.risk_without_manoeuvres,
                rows: c.body.rows.clone(),
                esa_reference_threshold: c.body.esa_reference_threshold,
                reading: c.body.reading.clone(),
            })
        }
        None => None,
    };

    let section = SpaceSafetySection {
        label: LABEL.into(),
        disposal: disposal_entries,
        decisions,
        decision_chain,
        thresholds,
        rules: disposal::rules::rules_for(rule_ids.iter().map(String::as_str)),
    };
    Ok(Assessed { section, files })
}

// ------------------------------------------------------------------ commands

/// Usage of the space-safety commands: the `mission` usage, which lists them.
pub use super::cli::USAGE;

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Positional arguments after the command, skipping flags and their values.
fn positionals(args: &[String]) -> Vec<String> {
    let takes_value = [
        "--out",
        "--license",
        "--sign-key-file",
        "--signer",
        "--thresholds",
        "--period-days",
        "--default-post-pc",
        "--utc",
    ];
    let mut out = Vec::new();
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if takes_value.contains(&a.as_str()) {
            i += 2;
            continue;
        }
        if !a.starts_with("--") {
            out.push(a.clone());
        }
        i += 1;
    }
    out
}

/// The signing seed (from `--sign-key-file` or `ARKSA_STUDY_SIGNING_KEY`), if any.
fn signing_seed(args: &[String]) -> Result<Option<[u8; 32]>, KshanaError> {
    let key = match flag(args, "--sign-key-file") {
        Some(f) => Some(std::fs::read_to_string(&f).map_err(|e| err(format!("{f}: {e}")))?),
        None => std::env::var("ARKSA_STUDY_SIGNING_KEY").ok(),
    };
    key.filter(|k| !k.trim().is_empty())
        .map(|k| kshana_pro::license::parse_key_hex(k.trim()))
        .transpose()
}

fn number(args: &[String], name: &str) -> Result<Option<f64>, KshanaError> {
    flag(args, name)
        .map(|v| {
            v.parse::<f64>()
                .map_err(|_| err(format!("{name} '{v}' is not a number")))
        })
        .transpose()
}

/// Run `mission decision|decision-chain|thresholds|disposal` (`args[0]` is the
/// command); `now_unix` stamps a signature. `None` when `args[0]` is none of them.
pub fn run(args: &[String], now_unix: i64) -> Option<Result<u8, KshanaError>> {
    let cmd = args.first()?.as_str();
    if !matches!(
        cmd,
        "decision" | "decision-chain" | "thresholds" | "disposal"
    ) {
        return None;
    }
    Some(run_inner(cmd, args, now_unix))
}

fn run_inner(cmd: &str, args: &[String], now_unix: i64) -> Result<u8, KshanaError> {
    let pos = positionals(args);
    let signer = || -> Result<Option<([u8; 32], String, String)>, KshanaError> {
        Ok(signing_seed(args)?.map(|s| {
            (
                s,
                flag(args, "--signer").unwrap_or_else(|| "Arksa".into()),
                crate::decision::time::fmt_utc(now_unix),
            )
        }))
    };
    let report_signed = |p: Option<std::path::PathBuf>| match p {
        Some(p) => println!("signed: {}", p.display()),
        None => println!("not signed: no signing key given"),
    };
    match cmd {
        "decision" => {
            let path = pos
                .first()
                .ok_or_else(|| err(format!("decision needs an input file\n\n{USAGE}")))?;
            let out = flag(args, "--out").ok_or_else(|| err("decision needs --out <dir>"))?;
            let (r, bytes) = crate::decision::record_file(Path::new(path))?;
            let s = signer()?;
            let signed = crate::decision::write_record(
                &r,
                &bytes,
                Path::new(&out),
                s.as_ref().map(|(k, n, u)| (k, n.as_str(), u.as_str())),
            )?;
            let f = &r.body.findings;
            println!(
                "{}: {}; chose '{}' ({}), which {}; {} review notes",
                r.body.input.event.event_id,
                f.band.as_str(),
                r.body.input.decision.chosen_option,
                f.chosen_kind.as_str(),
                f.consistency.as_str(),
                f.notes.len()
            );
            println!("record_sha256 {}", r.record_sha256);
            report_signed(signed);
            Ok(0)
        }
        "decision-chain" => {
            if pos.is_empty() {
                return Err(err(format!("decision-chain needs records\n\n{USAGE}")));
            }
            let mut docs = Vec::new();
            for p in &pos {
                let mut path = std::path::PathBuf::from(p);
                if path.is_dir() {
                    path = path.join("decision.json");
                }
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| err(format!("{}: {e}", path.display())))?;
                docs.push(
                    serde_json::from_str::<serde_json::Value>(&text)
                        .map_err(|e| err(format!("{}: {e}", path.display())))?,
                );
            }
            let problems = record::verify_chain(&docs);
            if problems.is_empty() {
                println!(
                    "OK: {} records, each hashes correctly and names the one before it",
                    docs.len()
                );
                Ok(0)
            } else {
                for p in &problems {
                    println!("FAIL: {p}");
                }
                Ok(1)
            }
        }
        "thresholds" => {
            let path = pos
                .first()
                .ok_or_else(|| err(format!("thresholds needs an events file\n\n{USAGE}")))?;
            let list = flag(args, "--thresholds")
                .ok_or_else(|| err("thresholds needs --thresholds <list>"))?;
            let bytes = std::fs::read(path).map_err(|e| err(format!("{path}: {e}")))?;
            let text = String::from_utf8(bytes.clone())
                .map_err(|_| err(format!("{path} is not UTF-8")))?;
            let events = policy::parse_events(&text)?;
            let c = policy::compare(
                &events,
                &policy::Study {
                    thresholds: policy::parse_thresholds(&list)?,
                    period_days: number(args, "--period-days")?,
                    default_post_pc: number(args, "--default-post-pc")?.unwrap_or(0.0),
                },
                &sha256_hex(&bytes),
            )?;
            print!("{}", c.to_csv());
            println!("{}", c.body.reading);
            if let Some(out) = flag(args, "--out") {
                std::fs::write(&out, c.to_json() + "\n").map_err(|e| err(format!("{out}: {e}")))?;
                println!("[wrote {out}]");
            }
            Ok(0)
        }
        "disposal" => {
            let path = pos
                .first()
                .ok_or_else(|| err(format!("disposal needs an input file\n\n{USAGE}")))?;
            let out = flag(args, "--out").ok_or_else(|| err("disposal needs --out <dir>"))?;
            let bytes = std::fs::read(path).map_err(|e| err(format!("{path}: {e}")))?;
            let a = disposal::assess_file(Path::new(path))?;
            let s = signer()?;
            let signed = disposal::write(
                &a,
                &bytes,
                Path::new(&out),
                s.as_ref().map(|(k, n, u)| (k, n.as_str(), u.as_str())),
            )?;
            let c = &a.body.counts;
            println!(
                "{}: {} compliant, {} marginal, {} not compliant, {} not assessed, {} disclosures; lifetime basis {}",
                a.body.spacecraft, c.compliant, c.marginal, c.not_compliant, c.not_assessed, c.info, a.body.lifetime.basis
            );
            println!("assessment_sha256 {}", a.assessment_sha256);
            report_signed(signed);
            Ok(0)
        }
        _ => unreachable!(),
    }
}

// ------------------------------------------------------------------ rendering

/// One block of the section as the HTML and PDF faces lay it out.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading(String),
    SubHeading(String),
    Para(String),
    Small(String),
    /// Column titles with their PDF widths (mm) and whether set in the fixed-width face.
    Table {
        cols: Vec<(&'static str, f32, bool)>,
        rows: Vec<Vec<String>>,
    },
}

fn p_text(p: f64) -> String {
    if p == 0.0 {
        "0".into()
    } else if p >= 0.01 {
        format!("{p:.3}")
    } else {
        format!("{p:.2e}")
    }
}

fn years(y: Option<f64>, beyond: bool) -> String {
    match y {
        Some(y) if y >= crate::disposal::lifetime::MAX_YEARS => {
            format!("> {} y", crate::disposal::lifetime::MAX_YEARS)
        }
        Some(y) => format!("{y:.2} y"),
        None if beyond => format!("> {} y", crate::disposal::lifetime::MAX_YEARS),
        None => "-".into(),
    }
}

impl SpaceSafetySection {
    /// The section as blocks, in reading order.
    pub fn blocks(&self) -> Vec<Block> {
        let mut b = vec![
            Block::Heading(
                "Space safety: collision-avoidance decisions and disposal compliance".into(),
            ),
            Block::Para(self.label.clone()),
        ];
        if !self.decisions.is_empty() {
            b.push(Block::SubHeading(format!(
                "Collision-avoidance decision records: {}",
                self.decisions.len()
            )));
            b.push(Block::Table {
                cols: vec![
                    ("Event", 24.0, false),
                    ("TCA (UTC)", 30.0, false),
                    ("Pc / miss", 22.0, false),
                    ("Policy band", 30.0, false),
                    ("Chosen", 30.0, false),
                    ("Against policy", 34.0, false),
                ],
                rows: self
                    .decisions
                    .iter()
                    .map(|d| {
                        vec![
                            d.event_id.clone(),
                            d.tca_utc.clone(),
                            format!("{} / {:.0} m", p_text(d.pc), d.miss_distance_m),
                            format!(
                                "{} ({}, manoeuvre at {})",
                                d.band,
                                d.policy,
                                p_text(d.manoeuvre_pc)
                            ),
                            format!(
                                "{} ({}), {:.1} h before TCA",
                                d.chosen_option, d.chosen_kind, d.hours_before_tca
                            ),
                            d.consistency.clone(),
                        ]
                    })
                    .collect(),
            });
            for d in &self.decisions {
                b.push(Block::Small(format!(
                    "{}: decided by {}. Rationale: {} Mitigation: {}.{} Record SHA-256 {}.",
                    d.event_id,
                    d.decided_by,
                    d.rationale,
                    d.mitigation,
                    if d.notes.is_empty() {
                        String::new()
                    } else {
                        format!(" Review notes: {}", d.notes.join(" "))
                    },
                    d.record_sha256
                )));
            }
            match &self.decision_chain {
                Some(p) if p.is_empty() => b.push(Block::Small(
                    "The records form an intact chain: each hashes correctly and names the one before it.".into(),
                )),
                Some(p) => b.push(Block::Para(format!("CHAIN BROKEN: {}", p.join(" ")))),
                None => b.push(Block::Small("The records are not chained (none names a previous record).".into())),
            }
        }
        if let Some(t) = &self.thresholds {
            b.push(Block::SubHeading(format!(
                "Threshold policy comparison over {} conjunction events",
                t.events
            )));
            b.push(Block::Table {
                cols: vec![
                    ("Pc threshold", 30.0, false),
                    ("Manoeuvres", 26.0, false),
                    ("Per year", 24.0, false),
                    ("Risk accepted", 32.0, false),
                    ("Reduction", 26.0, false),
                    ("90% and <= 1e-4", 32.0, false),
                ],
                rows: t
                    .rows
                    .iter()
                    .map(|r| {
                        vec![
                            format!("{:.1e}", r.threshold),
                            r.manoeuvres.to_string(),
                            r.manoeuvres_per_year
                                .map(|x| format!("{x:.1}"))
                                .unwrap_or_else(|| "-".into()),
                            format!("{:.3e}", r.risk_accepted),
                            format!("{:.1}%", r.reduction * 100.0),
                            if r.meets_esa_reduction && r.within_esa_upper_bound {
                                "yes".into()
                            } else {
                                "no".into()
                            },
                        ]
                    })
                    .collect(),
            });
            b.push(Block::Small(format!(
                "{} Risk without manoeuvres {:.3e} (sum of the listed probabilities). The 90% column is set beside ESA ESSB-ST-U-007 5.3.3.3 b; untracked debris is not counted here, so the reduction is an upper bound. Comparison SHA-256 {}.",
                t.reading, t.risk_without_manoeuvres, t.comparison_sha256
            )));
        }
        for d in &self.disposal {
            b.push(Block::SubHeading(format!(
                "Disposal compliance: {} ({})",
                d.spacecraft, d.strategy
            )));
            b.push(Block::Para(format!(
                "Orbital lifetime ({}): nominal {}, conservative bound {}. {} compliant, {} marginal, {} not compliant, {} not assessed, {} disclosures.",
                d.lifetime_basis,
                years(d.lifetime_nominal_years, false),
                years(d.lifetime_upper_years, d.lifetime_upper_beyond_horizon),
                d.counts.compliant,
                d.counts.marginal,
                d.counts.not_compliant,
                d.counts.not_assessed,
                d.counts.info
            )));
            if let Some(x) = &d.cross_check {
                b.push(Block::Para(format!("Cross-check: {x}")));
            }
            b.push(Block::Table {
                cols: vec![
                    ("Clause", 48.0, false),
                    ("Verdict", 26.0, false),
                    ("Basis", 26.0, false),
                    ("Value", 36.0, false),
                    ("Limit", 34.0, false),
                ],
                rows: d
                    .checks
                    .iter()
                    .map(|c| {
                        vec![
                            c.citation.clone(),
                            c.verdict.as_str().into(),
                            c.basis.clone(),
                            c.value.clone(),
                            c.limit.clone(),
                        ]
                    })
                    .collect(),
            });
            b.push(Block::Small(format!(
                "Assessment SHA-256 {}; the full assessment, with each check's detail, is {}.",
                d.assessment_sha256, d.result.path
            )));
        }
        b.push(Block::SubHeading("Rule texts referred to".into()));
        for r in &self.rules {
            b.push(Block::Small(match r.status {
                crate::disposal::rules::RuleStatus::Verified => format!(
                    "{} - \"{}\" Source: {}, {} (read {}).{}",
                    r.citation,
                    r.quote,
                    r.source_title,
                    r.source_url,
                    r.retrieved,
                    if r.note.is_empty() {
                        String::new()
                    } else {
                        format!(" {}", r.note)
                    }
                ),
                crate::disposal::rules::RuleStatus::ToConfirm => {
                    format!("{} - TO CONFIRM. {}", r.citation, r.note)
                }
            }));
        }
        b
    }

    /// The section as HTML (escaped), with the anchor `space-safety`.
    pub fn to_html(&self) -> String {
        let esc = |s: &str| {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
        };
        let mut h = String::new();
        for blk in self.blocks() {
            match blk {
                Block::Heading(t) => {
                    h += &format!("<h2 id=\"space-safety\" class=\"break\">{}</h2>", esc(&t))
                }
                Block::SubHeading(t) => h += &format!("<h3>{}</h3>", esc(&t)),
                Block::Para(t) => h += &format!("<p>{}</p>", esc(&t)),
                Block::Small(t) => h += &format!("<p class=\"small\">{}</p>", esc(&t)),
                Block::Table { cols, rows } => {
                    h += "<div class=\"scroll\"><table><thead><tr>";
                    for (c, _, _) in &cols {
                        h += &format!("<th>{}</th>", esc(c));
                    }
                    h += "</tr></thead><tbody>";
                    for r in rows {
                        h += "<tr>";
                        for c in r {
                            h += &format!("<td>{}</td>", esc(&c));
                        }
                        h += "</tr>";
                    }
                    h += "</tbody></table></div>";
                }
            }
        }
        h
    }
}
