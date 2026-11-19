# Cislunar SSA: catalogue build-up and debris environment (`[ssa]` of `pro-cislunar-observability-study`)

PRIVATE: Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.

Research: vcprep `records/ops/kshana/research-2026-11-20-gaps/10-esa-tenders-forward.md` (ESA 198267
"S3-SD-13 Cislunar catalogue build-up demonstration": initial orbit determination, data association, OD
refinement, orbit taxonomy, visualisation; ESA 193155 "Debris environment modelling in cis-lunar space";
both intended EXPRO+ tenders, 200–500 k€, open to Estonia) and `research-2026-11-19/03-lunar-deep-space.md` §3.
These are the tender scopes only: ESA has no involvement in this work and endorses nothing here.

Evidence class: **MODELLED**. Every result carries the label in `src/cislunar_ssa/mod.rs` (`LABEL`). Synthetic
campaigns are scored against the truth they were generated from, which checks internal consistency, not
performance on real data.

## Gate

No new kind. The work is an optional `[ssa]` section of the existing Arksa-exclusive kind
`pro-cislunar-observability-study` (already in `ARKSA_KINDS` and in Kshana Pro's signed tier manifest at the
`arksa` tier), so it runs only under an Arksa-signed `arksa` entitlement and needs no manifest change. When the
section is present the run returns the SSA result instead of the observability study. There is no command
line entry yet: a command needs a feature id listed in the manifest (see Follow-ups); the signed directory is
written by `cislunar_ssa::write_study`.

## What it does

| Part | Module | What |
|---|---|---|
| Frames and model | `src/cislunar_ssa/mod.rs` | CR3BP state (synodic, nondimensional) propagated with the open engine's adaptive flow (`kshana::cr3bp_family`), backward by the CR3BP time-reversal symmetry; mapped to geocentric ICRF on the instantaneous Earth–Moon line of the open analytical Moon series (`kshana::ephem::moon_icrf`); observers: geocentre, WGS-84 station through the open IAU 2006/2000A CIO chain (`kshana::cio`), or a spacecraft on a CR3BP orbit; RA/Dec residuals with an analytic Jacobian (angle partials × frame × variational STM), cross-checked against central finite differences |
| IOD | `src/cislunar_ssa/iod.rs` | range-hypothesis grid (log-spaced 30,000–1,500,000 km at the first and last observation of a tracklet), a CR3BP Lambert solve per cell (single shooting with the STM), best distinct cells refined by Levenberg–Marquardt; ambiguity flagged when a distinct solution is within Δχ² = 9; formal covariance (HᵀWH)⁻¹ |
| Catalogue build-up | `src/cislunar_ssa/catalogue.rs` | tracklets in time order; association by the Mahalanobis distance rᵀ(HPHᵀ+I)⁻¹r gated at a χ² quantile (0.999, Wilson–Hilferty) and confirmed by a joint refit (reduced χ² ≤ 4); otherwise a new object from IOD; otherwise held and retried after all tracklets; leftovers reported as uncorrelated with the reason; a decision log |
| Taxonomy | `src/cislunar_ssa/taxonomy.rs` | features over a 21-day window and documented rules: `dro`, `dpo`, `nrho` (closest approach ≤ 17,500 km, Zimovan-Spreen et al. 2020), `halo` (L1/L2), `lyapunov-l1/l2`, `elfo` (e ≥ 0.3, ω within 30° of 90°/270°), `llo`, `lunar-orbit`, `earth-orbit`, `escape`, `lunar-impact`, `earth-reentry`, `transfer`; each catalogued label is re-evaluated at the twelve one-sigma points of the covariance (`covariance_agreement`, `stable_under_covariance`) |
| Debris environment | `src/cislunar_ssa/debris.rs` | along a trajectory (preset, state or catalogued object): catalogued objects with STM-propagated covariance, sampled break-up clouds (seeded isotropic Δv, Gaussian kernel, weight = represented / sampled) and background shells (flux n·A·v·Δt); encounters at local minima of separation, linear relative motion to closest approach, 2-D Pc from AK1's `conjunction::pc::pc_2d` for short encounters, and for slow ones (encounter longer than four steps, common for fragments of a cloud on the same orbit) the flux of the Gaussian position density through the hard-body cross-section integrated over the encounter; expected count and P(≥1) = 1 − e^(−N); top encounters |
| TDM | `src/cislunar_ssa/tdm.rs` | CCSDS 503.0-B-2 KVN via the open engine's parser: `ANGLE_TYPE = RADEC`, EME2000/ICRF/GCRF, UTC/TAI/TT; `PARTICIPANT_1` = observer, `PARTICIPANT_2` = track; other angle types refused; a writer for synthetic campaigns |
| Synthetic campaigns | `src/cislunar_ssa/synthetic.rs` | truth from the open engine's corrected families (NRHO 9:2 seed, L2/L1 halo, L1 Lyapunov, DRO) or Moon-centred elements (ELFO after the Moonlight class, LLO); tracks per observer with elevation mask, seeded Gaussian noise; scoring: completeness, duplicates, association purity, position/velocity error, NEES (6 dof) |
| Output | `src/cislunar_ssa/mod.rs` | `study.json` (schema `arksa-cislunar-ssa/1`), `catalogue.csv`, `report.md`, `figure.svg` (rotating frame), `observations.tdm` (synthetic), signed as a directory with Kshana Pro's dossier signature |

Worked scenario: `scenarios/cislunar-ssa/demo.toml`.

## Model fidelity (what is NOT modelled)

Circular restricted three-body dynamics only: no solar gravity, solar radiation pressure, gravity harmonics or
lunar eccentricity in the dynamics (the ICRF mapping does follow the real Earth–Moon distance and direction).
No light time, aberration or refraction; UT1 = UTC, no polar motion; station observations need only clear an
elevation mask (no daylight, weather, brightness or phase-angle limits). Formal covariances assume the stated
angle noise. Taxonomy thresholds are modelling choices, and inclinations are from the Earth–Moon orbital plane
(the lunar equator is about 6.7° away). Debris populations and densities are inputs, not a validated cislunar
environment model.

## Tests

`tests/cislunar_ssa.rs` (12) and unit tests in the modules (5): taxonomy labels for corrected NRHO, L2/L1
halo, L1 Lyapunov, DRO, ELFO and LLO members and for Earth orbit, escape and impact; analytic vs
finite-difference Jacobian; single-tracklet IOD consistency (reduced χ², NEES); a two-day, two-station,
three-object campaign (association purity 1, no duplicates, NEES inside the 0.999 gate, families correct and
stable under the covariance for the multi-day objects; the one-tracklet halo object is labelled with a
covariance agreement below 1); TDM round trip into the same catalogue; signed directory verifies and detects
tampering; MODELLED label and no endorsement claim; debris risk linear in the represented population and the
background flux formula; the slow-encounter flux integral equals the 2-D short-encounter integral on a fast encounter; the worked scenario parses; the Arksa gate (standalone Pro token refused, Arksa token runs); determinism.

## Follow-ups

- A command line (`arksa-kpro cislunar-ssa <scenario> --out <dir> --sign-key-file f`) needs a feature id in
  Kshana Pro's signed tier manifest (`ARKSA_ENTRIES`), for example `pro-cislunar-ssa` (type `feature`), added in
  kshana-pro first; this PR does not touch kshana-pro.
- Joint IOD of two short tracklets (track-to-track association) for objects never seen long enough alone.
- Ephemeris-model dynamics (Sun, eccentric Moon, SRP) for real-data campaigns; light time and aberration.
- Validation on real angles (an observation-data partner, as the 198267 scope expects) before any claim beyond
  MODELLED.
