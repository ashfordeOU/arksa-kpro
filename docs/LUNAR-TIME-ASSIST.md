# Lunar time-scale registry and lunar GNSS assistance (AK6)

Arksa-exclusive. Code: `src/lunar_time_assist/` (`registry.rs`, `assist.rs`, `service.rs`,
`cli.rs`, the built-in registry `registry_v1.toml`); tests: `tests/lunar_time_assist.rs`;
command: `arksa-kpro lunar-time ...`; entitlement: the existing Arksa-exclusive kind
`pro-lunar-time-budget-study` (no new tier-manifest entry). The lunar time budget study
(`src/packs/lunar_time_budget_study_pack.rs`) gains an optional `[time_scale]` section.

**Evidence class: MODELLED** throughout. Nothing here is validated against measured lunar data.
The package format `ks-lunar-gnss-assist/1` is a **proposal, not a standard**.

Research behind it: vcprep `records/ops/kshana/research-2026-11-20-gaps/13-lunar-patents-second-pass.md`
findings 4–6 and gaps N7, N9 (G-B, G-C); `research-2026-11-19/03-lunar-deep-space.md` §2.

## Why

* Intuitive Machines (NASA LCRNS) says in public that "the community has yet to agree on an
  international standard" for lunar time, computes internally in TDB and outputs TL/TCL through
  "a software layer that can be updated as the international standards are defined".
* CGPM 2026 Resolution 6 asks for one lunar reference time scale traceable to UTC but does not
  choose between unscaled TCL and a TL scaled on a lunar equipotential `W0_L`; no conventional
  `W0_L` exists; NASA's LTC strategy is due 31 Dec 2026 with no adoption date.
* JAXA LNSS (ArkEdge) needs a ground-made long-term ephemeris with "at least 24H as effective
  age" for more than 60 GPS and Galileo satellites, because lunar receivers cannot decode most
  GNSS navigation data.

## 1. The provisional time-scale registry

The physics is Kshana Pro's lunar time model (`kshana_pro::lunar_pro::time`, docs/LUNAR-PRO.md
D1 in kshana-pro): TCB/TCG/TT/TDB, the TCL secular rate (LTE440 or first principles), the
periodic terms, the `W0_L` scaling to TL, each with a GUM budget. This crate adds the layer
around it:

* **A registry file** (`registry_v1.toml`, built in) with an id, a version, an issue date, a
  status and a SHA-256. Version 1 is **provisional** and lists:
  * `tcl-iau2024-lte440`, `tcl-iau2024-first-principles` (TCL, two realisations of its rate);
  * `tl-w0-kshana-equatorial-j2`, `tl-w0-turyshev-selenoid`, `tl-w0-turyshev-south-pole`,
    `tl-w0-point-mass` (TL on four candidate surfaces);
  * `ltc-placeholder` (the US policy name, realised as an envelope; below).
  Stated agency preferences are recorded per entry as information, not as a ranking.
* **Rules** (`Registry::validate`): a provisional registry must carry both the TCL and the TL
  families (one option alone would read as the standard), may not mark any definition adopted,
  and must say PROVISIONAL in its notice; an adopted registry must cite its decision; TCL takes
  no `W0_L`, TL needs one; LTC members must exist and not be withdrawn.
* **No default.** Every conversion names its definition; an unknown id is refused with the list.
* **LTC as an envelope.** LTC has no definition. Its offset is the equal-weight midpoint of the
  candidate realisations, with the half-spread as a rectangular "definition undecided" term and
  the largest candidate uncertainty added. The midpoint is not a recommendation.
* **Every output carries** the registry id, version, SHA-256 and status, and (when provisional)
  the notice.
* **Updating.** When CGPM/IAU/IAG decide, publish a new registry file (status `adopted`,
  `decision` citing the resolution); no code change is needed. `lunar-time impact` compares two
  versions at chosen epochs and flags each definition whose parameters changed, that was
  removed, or whose `lunar − UTC` moved by more than a tolerance (default 1 ns): the
  re-qualification evidence an updatable time layer needs. With `--out` and a key the result
  directory is signed (Kshana Pro dossier signature).

Checked by tests: conversions equal Kshana Pro's model; TCL rate within 0.05 µs/day of OSTP's
58.7, TL (default surface) within 0.01 of Ashby and Patla's 56.02, TCL − TL on Turyshev's
selenoid 2.712 µs/day; first-principles TCL rate within 2u of LTE440 (model-against-model
cross-checks, not validation); the LTC envelope arithmetic; the impact flags exactly the
definitions that moved; the rules refuse a single-option provisional registry and an uncited
adoption.

```text
arksa-kpro lunar-time registry
arksa-kpro lunar-time convert --definition tl-w0-turyshev-south-pole --at 2027-01-01T00:00:00 --reference gpst
arksa-kpro lunar-time spread --at 2027-01-01T00:00:00
arksa-kpro lunar-time impact --new registry_v2.toml --at 2027-01-01T00:00:00,2030-01-01T00:00:00 --out impact/ --sign-key-file key.hex
```

## 2. The lunar GNSS assistance package (`ks-lunar-gnss-assist/1`, proposal)

* **Content:** a GPST epoch grid (`start`, `step_s`, `n_epochs`), per-satellite ECEF positions
  (integer mm) and clocks (integer ps) — the SP3 resolution — a validity window inside the data
  (no extrapolation), the interpolation rule (11-point Lagrange with per-node Earth-rotation
  correction, linear clocks: the open engine's SP3 interpolation), and per satellite an
  **integrity bound**: a zero-mean Gaussian overbound of the conservative range error
  `|Δr| + c|Δt|` with `σ(age) = σ0 + g1·age + g2·age²` (age in hours from the start of validity)
  and an integrity risk `P` per independent sample (`K = Φ⁻¹(1 − P/2)`).
* **Lunar time reference:** the registry id, version, SHA-256, status and definition id the
  user maps GNSS time with, `lunar − GPST` at the start of validity with its uncertainty, and
  the secular rate.
* **Signature:** Ed25519 over the canonical JSON without the signature block; signing refuses a
  package whose bytes would not survive a JSON round trip.
* **`assist pack`** packs a predicted SP3 (uniform grid; satellites with a missing epoch or a
  bad sentinel are dropped and listed) with a bound file (`[bound]` plus per-satellite
  overrides).
* **`assist check`** checks requirements; defaults follow the published LNSS statement:
  validity ≥ 24 h, ≥ 60 satellites, GPS + Galileo, node spacing ≤ 900 s, signed; optional
  protection level `K·σ` at the end of validity and a required lunar time reference. Exit code
  1 when any item fails.
* **`assist score`** scores a package against a reference product (for example a later precise
  SP3) at the reference epochs inside validity: error-versus-age curve (RMS, max, mean σ,
  RMS/σ, exceedances per bin), per-satellite summary, and an integrity statement with
  one-sided Clopper–Pearson bounds over samples spaced by `decorrelation_s` (default 1 h):
  `BOUND-HELD (demonstrated)`, `NOT-DEMONSTRATED` (with the number of clean independent samples
  that would be needed), or `BOUND-VIOLATED`. Option `clock_datum = "remove-epoch-mean"` for
  products with different clock references (it then hides a common clock error and says so).
  The directory (`score.json`, `age_curve.csv`, `report.md`) is signed when a key is given.

Checked by tests (synthetic 62-satellite GPS + Galileo constellation through the SP3 text
format): a package of the truth scores below 1 cm; injected along-track and clock errors are
recovered to 1 cm at 24 h; an honest bound is demonstrated, an undersized one violated, and a
1e-7 allocation reported as not demonstrable from a day of data; tampering, a wrong key, too
few satellites, a missing constellation and an over-large protection level are each caught;
packing refuses to extrapolate; the signed score directory verifies.

## 3. The hosted generator (not in this crate)

Producing the long-validity prediction and deriving its overbound is the high-value method.
Under the X3 rule it belongs in the hosted verdict service and is reached over the network.
`service::request_package` posts an `AssistRequest` (satellites, validity start and length,
step, largest integrity risk, the registry definition wanted) to `/verdict/lunar-assist`,
verifies the signed verdict envelope against the trusted service keys (Kshana Pro's
`remote_verdict::verify`), and accepts the package only if it answers the request.
`arksa-kpro lunar-time assist request --service <url> --request <json> --trusted-key <hex>`.

**Status: the service method does not exist yet.** The client is tested against a local stand-in
that signs with a test key. Building the method in the hosted service is a follow-up.

## 4. The study's `[time_scale]` section

```toml
[time_scale]
epochs_utc = ["2027-01-01T00:00:00", "2030-01-01T00:00:00"]
definitions = []   # empty: every live definition, side by side
```

The `pro-lunar-time-budget-study` result then carries a `time_scale` block (registry reference,
notice, `no_default_definition: true`, per-definition `lunar − UTC` and rate with uncertainty,
the candidate spread per epoch) and the PDF a short section. Without the section the study's
output and hash are unchanged.

## Honest limits

* Everything is MODELLED; the rate cross-checks are model against model.
* The TCL origin convention and the analytic periodic series (≈0.2 µs) are Kshana Pro's, with
  the limits stated in kshana-pro docs/LUNAR-PRO.md.
* The registry's entries record positions as published in the cited sources (secondary for the
  CGPM adoption text); none is presented as the standard.
* The scorecard's binomial statement treats samples spaced by `decorrelation_s` on one satellite
  as independent; errors shared across satellites make it optimistic. The reference product is
  not ground truth.
* Time systems and frames are declared, not converted: both products must be GPST and ECEF in
  the same frame.
* No pending leap second handling beyond Kshana Pro's leap-second table.
