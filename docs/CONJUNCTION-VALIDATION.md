# Conjunction-assessment validation (`pro-conjunction-validation`)

PRIVATE: Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.

Research: vcprep `records/ops/kshana/research-2026-11-20-gaps/` 06 §0.1–0.4, G1 (neutral validation
scorecards for CA services), G2 (covariance-realism evidence), G16 (CDM/OEM ingest); 06b findings 1–3
(the OSC CASS data-quality-monitor slot, the CC0 TraCSS verification dataset, the EU SST–TraCSS study).

Evidence class: **MODELLED**. Agreement figures are exact comparisons of the named files (SHA-256 in every
result) against the reference set the user names. A reference set is not ground truth: the TraCSS answer key
is the output of a third-party tool (Aerospace's CSieve), and its publisher calls the dataset a diagnostic for
self-evaluation, "not ... a tool for formal system certification or validation". Nothing here validates or
certifies a provider, and no agency (NASA, ESA, the US Office of Space Commerce or any other) endorses it.

## What it does

| Part | Module | What |
|---|---|---|
| CDM ingest | `src/conjunction/cdm.rs` | CCSDS 508.0-B-1 CDM, KVN and NDM/XML, several messages per file; state km→m, RTN covariance (21 terms or position-only); `HBR` read when a provider adds it; a KVN writer (`write_kvn`) |
| Results-table ingest | `src/conjunction/answer_key.rs` | the TraCSS verification dataset's results-table columns (user's guide, Table 5), also accepted as a provider's output format; streaming reader with a pair filter |
| OEM / OMM | `src/conjunction/ephemeris.rs` | OEM states through the open engine's reader, plus every covariance matrix of every segment (the open reader skips them); OMM (KVN/XML) to an object class and SFSH regime |
| Pc | `src/conjunction/pc.rs` | 2D short-encounter Pc (disc integral in principal axes, tail-safe), encounter-plane Mahalanobis distance; used to recompute and explain, never to grade |
| Checks | `src/conjunction/scorecard.rs` | per event: TCA in window, no self-on-self, inside the screening volume (sphere or SFSH box), miss = \|relative position\|, relative position and speed consistent with the states, TCA is a closest approach (range rate as a time offset within the velocity-class objective), covariances PSD, Pc in [0,1], OD data age |
| Scorecard | `src/conjunction/scorecard.rs` | provider vs reference: dedupe of A–B/B–A reports, pair + nearest-TCA matching, recall/precision with boundary cases reported apart, TCA objectives by relative-velocity class (VLRV < 10 m/s ~10 s, LRV < 50 m/s ~0.01 s, NLRV ~0.001 s), miss and relative-position agreement (1 m), covariance Bhattacharyya distance (0.1), Pc ratio for information only, recall by object class |
| Realism | `src/conjunction/realism.rs` | Mahalanobis d² against χ²₃ per object class and prediction-age bin: σ scale factor, two-sided χ² test on Σd², KS distance and p-value, 95 % containment, per-axis normalised variance; from predicted+reference OEMs or a residual table |
| Comparison | `src/conjunction/compare.rs` | the same event from two providers: matching within 6 % of the orbital period, ΔTCA, Δmiss, ΔPc, classification under each provider's policy and conflicts, one-input swaps (HBR, covariances, states) on the recomputed Pc with the interaction remainder, provenance notes (ephemeris source, covariance method, OD age, HBR missing) |
| Pack and command | `src/packs/conjunction_validation_pack.rs`, `src/commands.rs` | one TOML scenario runs any of the sections; `arksa-kpro conjunction <scenario.toml> --out <dir> [--sign-key-file f]` writes `study.json`, `events.csv`, `report.md` and signs the directory (Kshana Pro dossier signature); `arksa-kpro conjunction ingest <files>` prints normalised events |

Default objectives are cited in every result: Auman et al., *Validation methodology for TraCSS conjunction
assessment*, AMOS 2025; the dataset's user's guide (March 2026); Borowitz et al., AMOS 2026 (6 % period
matching). The NASA CA Handbook Vol. 1 (Aug 2026) points to validation procedures in its Appendix J (Volume 2);
Volume 2 was not reviewed for this build, so the checks follow the AMOS methodology and the dataset guide, not
the appendix text.

## Gate

Kind and command feature `pro-conjunction-validation`, `arksa` tier, registered through the hook like F4/M9
(`PENDING_MANIFEST_KINDS`, `PENDING_MANIFEST_FEATURES`). Until Kshana Pro's signed tier manifest lists its two
hashes (kind and feature: SHA-256 of `pro-conjunction-validation`), it is built and tested as a library and
pack but is not reachable through the dispatcher or the command line, and is refused like any unknown kind.

## The public dataset

The OSC "Dataset for Conjunction Assessment Verification" (CC0-1.0, 18 March 2026) has a 21.7 GB OCM ephemeris
archive, two results tables (spherical 10 km with 0.5 m HBR, ~200 MB gz; SFSH volumes with discrete HBR,
~60–144 MB gz), a volume mapping file and a user's guide. The data files are released through a Google Drive
access request on the OSC page, so they cannot be fetched without an account and are not committed here.

To run the opt-in test on a local copy:

```bash
gunzip IVV_Releasable_Dataset_Spherical_DefaultHBR.csv.gz
ARKSA_TRACSS_KEY=$PWD/IVV_Releasable_Dataset_Spherical_DefaultHBR.csv \
  [ARKSA_TRACSS_TEST=provider_results.csv] cargo test --test conjunction_validation tracss -- --nocapture
```

Unverified assumptions about the results table (stated in every result that reads one): the `C1_*`/`C2_*`
covariance columns are the upper triangle row by row, in km-based units (`covariance_unit = "m2"` overrides).
The first run on the real file should confirm both.

## Not built (deliberately)

- A conjunction screener: this grades and explains others' outputs; it does not produce CDMs from ephemerides
  (the 21 GB OCM input is not read).
- A hosted held-out truth set (research G1 puts that in meru, behind the network boundary).
- Pc methods beyond 2D (3D / Monte Carlo for low relative velocity): VLRV/LRV events are scored on TCA and
  geometry only, and the recomputed Pc is flagged as rectilinear.
- Decision records and disposal compliance (brief AK2).
