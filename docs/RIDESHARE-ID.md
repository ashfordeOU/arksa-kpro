# Rideshare object identification (`rideshare-id`, `pro-rideshare-id`)

PRIVATE: Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.

Research: vcprep `records/ops/kshana/research-2026-11-20-gaps/` 06 §1.3 (P23–P30) and G3 ("which object is
mine?"); 06b §0 finding 4 (the five-digit catalogue number ran out in November 2026; objects of the 14 Nov 2026
rideshare were numbered 101388–101400; Space-Track calls Alpha-5 a stop-gap and recommends OMM).

Evidence class: **MODELLED**. The candidate orbits are propagated with SGP4 from the element sets given; the
observations are the operator's own; the probabilities assume Gaussian measurement errors and that the
operator's object is one of the candidates. This is not an identification by any catalogue authority, and no
catalogue authority, launch provider or agency endorses it. The method has been exercised on synthetic
deployments only (below); it has not been run on a real launch's data.

## The problem

After a rideshare, tens of objects are catalogued with nearly identical element sets. An operator that points
its antenna at the wrong one cannot command its satellite; identification can take days to weeks and some
objects are never matched. The operator does have its own evidence: the Doppler curve, ranging or antenna angles
of every pass its ground stations made. This tool turns that evidence into a ranked, signed answer.

## What it does

| Part | Module | What |
|---|---|---|
| Catalogue numbers | `src/rideshare/norad.rs` | Alpha-5 encode/decode (`A` = 10 … `Z` = 33, no `I`/`O`; 100000–339999), OMM `NORAD_CAT_ID` up to nine digits, SGP4 elements from OMM, TLE line-1 identity with an Alpha-5 field. **Local stop-gap** (see below) |
| Candidates | `src/rideshare/catalogue.rs` | TLE (2LE/3LE, Alpha-5, checksum recorded, line 1/line 2 numbers must agree), OMM KVN (one or many messages), NDM/XML (`<omm>` bare or inside `<ndm>`), and JSON arrays with KVN keywords as keys (as catalogue services publish; other keys ignored); merge by catalogue number (newest epoch kept, a warning for each duplicate); writers for TLE (Alpha-5, checksums) and OMM KVN |
| Observations | `src/rideshare/observations.rs` | the CSV below; passes split per station at 15-minute gaps unless labelled |
| Identification | `src/rideshare/identify.rs` | per candidate: SGP4 → ITRS (the open engine's TEME→ITRS chain) → range, range rate, Doppler, azimuth, elevation from the WGS-84 station; residuals; per-pass biases; χ² ranking; scaled Δχ²; posterior probability; separation of the two leading candidates; verdict |
| Synthetic deployments | `src/rideshare/synthetic.rs` | N objects released along one orbit with chosen spacing and drift, their TLE and OMM, noisy observations of one of them (unknown oscillator offset included), with the answer |
| Pack | `src/packs/rideshare_id_pack.rs` | `pro-rideshare-id` scenario; `verdict.json`, `candidates.csv`, `residuals.csv`, `report.md`; signed with Kshana Pro's dossier signature |
| Command | `src/rideshare/cli.rs` | `arksa-kpro rideshare-id identify|catalogue|norad|synth` |

## Commands

```
arksa-kpro rideshare-id identify <scenario.toml> --out <dir> [--sign-key-file <file>] [--signer <name>]
arksa-kpro rideshare-id catalogue <file>...                 # list objects: number, TLE field or "OMM only", epoch
arksa-kpro rideshare-id norad encode 270002                 # -> T0002
arksa-kpro rideshare-id norad decode A1388                  # -> 101388
arksa-kpro rideshare-id synth --out <dir> [--spec deployment.toml]
```

`identify` exits 0 for `IDENTIFIED` and 1 for every other verdict, so a script cannot mistake an ambiguous
result for an answer. With a signing key (64 hex characters in a file, or `ARKSA_STUDY_SIGNING_KEY`) the output
directory is signed as a whole (`signature.json`); check it with `arksa-kpro dossier verify`.

## Scenario

```toml
kind = "pro-rideshare-id"
title = "Which object of launch 2026-900 is ours?"
observations_file = "passes.csv"                  # or observations_text
candidate_files = ["launch.tle", "launch.xml"]   # TLE or OMM, detected from the content; or candidate_text

[identify]                 # all optional; defaults shown
doppler_bias_per_pass = true    # estimate the satellite oscillator's unknown offset per pass
range_bias_per_pass = false     # estimate a ranging bias per pass
max_reduced_chi2 = 4.0          # above this even the best candidate does not explain the data
decision_delta_chi2 = 25.0      # margin the runner-up must trail by
min_probability = 0.99          # posterior the best must reach
min_observations = 10
horizon_deg = 0.0               # measurements a candidate puts below this count as "below horizon"
```

## Observation CSV

One measurement per row, a header row naming the columns (any order; an unknown column is refused), `#`
comments.

| column | required | meaning |
|---|---|---|
| `time_utc` | yes | `YYYY-MM-DDThh:mm:ss[.f]Z`, the receive time |
| `station` | yes | station name |
| `lat_deg`, `lon_deg`, `alt_m` | yes | WGS-84 geodetic position (height above the ellipsoid); identical on every row of a station |
| `type` | yes | `doppler_hz`, `range_m`, `range_rate_m_s`, `az_deg`, `el_deg` |
| `value` | yes | the measurement |
| `sigma` | yes | its one-sigma uncertainty, same unit |
| `carrier_hz` | Doppler rows | nominal downlink carrier |
| `pass` | no | pass label; when empty, passes are split per station at gaps over 15 minutes |

Conventions: `doppler_hz` = received − nominal frequency for a one-way downlink, first order
(−`carrier_hz` × range rate / c); `range_m` is the one-way slant range (half a two-way round trip); azimuth from
north through east; elevation above the ellipsoid's local horizontal.

**Sigma must cover more than receiver noise.** Fresh element sets after a rideshare carry along-track errors of
hundreds of metres or more. If the sigmas state only the receiver noise, even the right object can fail the
consistency test (`NO_CONSISTENT_CANDIDATE`); the verdict then says so rather than picking the nearest object.
State sigmas that include the expected element-set prediction error, or use the newest element sets available.

## Method and verdict

For each candidate `k` and measurement `i`: residual `r_ik` = observed − predicted (azimuth wrapped); per-pass
constant biases (Doppler by default) are removed by weighted least squares; `χ²_k = Σ (r_ik/σ_i)²`,
`dof = n − biases`.

* Rank by `χ²`. Variance factor `s² = max(1, χ²_best/dof)`; `Δ_k = (χ²_k − χ²_best)/s²`; posterior under a
  uniform prior `p_k = exp(−Δ_k/2) / Σ_j exp(−Δ_j/2)`; likelihood ratio best:second `10^(Δ_2 / (2 ln 10))`.
* Separation of the two leading candidates: `sqrt(Σ((pred_best − pred_second)/σ)²)` (how many sigma their
  predictions differ over these very measurements; below 5 the data cannot separate them however clean) and
  the distance between their predicted positions (min, median, max over the observation times).

| verdict | when |
|---|---|
| `IDENTIFIED` | best consistent (`χ²/dof ≤ max_reduced_chi2`), `Δ_second ≥ decision_delta_chi2`, `p_best ≥ min_probability` |
| `AMBIGUOUS` | best consistent, but the margin or the probability falls short; `not_excluded` lists every candidate within the margin; nothing is named as the answer |
| `NO_CONSISTENT_CANDIDATE` | even the best candidate does not explain the data (object missing from the set, element sets too old, or sigmas too small) |
| `CONSISTENT_ONLY_CANDIDATE` | one candidate given; consistent, but nothing was discriminated |
| `INSUFFICIENT_DATA` | fewer than `min_observations` measurements |

Not modelled: light time, tropospheric and ionospheric refraction, transmitter frequency drift within a pass,
element-set covariance. At LEO these are small against the along-track differences that separate rideshare
objects, but they are part of why sigma should be generous.

## Outputs

`verdict.json` (schema `arksa-rideshare-id/1`): verdict, identified object (catalogue number, TLE field or
`null` when the number is above 339,999, name, designator, probability), `not_excluded`, the reasons in words,
the separation block, the variance factor and configuration, an observation summary, every candidate's fit
(rank, χ², reduced χ², Δχ², probability, per-type residual RMS and mean in the measurement's unit, the per-pass
biases, measurements below the horizon, element age), the SHA-256 of each input, warnings and
`result_sha256`. Every non-integer number is rounded to 9 significant digits before hashing, so the hash is the
same on macOS and Linux. `candidates.csv` (ranked table), `residuals.csv` (each measurement with the two leading
candidates' predictions and residuals), `report.md`.

## Catalogue numbers above 99,999

* TLE: the five columns hold Alpha-5 up to 339,999 (`A0000` = 100,000, `Z9999` = 339,999); line 1 and line 2
  must name the same object; the checksum counts a letter as 0.
* OMM: `NORAD_CAT_ID` is read as an integer of up to nine digits from the TLE-parameters block or the metadata.
  An object above 339,999 can come only from OMM; it is reported as "OMM only" and carries `tle_field: null`.
* Writers: `format_tle` refuses a number above 339,999 (write OMM); `format_omm_kvn` takes up to nine digits.

**Local helper, to be replaced.** The open engine's `norad` module (kshana main, PR #114, c86040bd) is not in
the published kshana 0.41.0 this crate builds on (0.41.0's `tle::parse_tle_identity` reads five digits only).
`src/rideshare/norad.rs` follows the same rules and is checked against the open engine's conformance vectors
(`tests/fixtures/rideshare/norad-catalogue.json`, copied from kshana `data/conformance/time/norad-catalogue.json`
at c86040bd). The test `the_pinned_engine_still_lacks_alpha5_so_the_local_helper_is_needed` fails on purpose once
a kshana release with Alpha-5 support is pinned: then delete the local module and call `kshana::norad`.

## Gate

The command runs under the mission feature `pro-mission-dossier` (Arksa tier), like `ops`, `simcal` and
`landing-nav`: it needs a token with the `arksa` claim (`ensure_rideshare_id_entitled` in
`src/entitlement.rs`). The scenario kind `pro-rideshare-id` is registered through the hook only once Kshana Pro's
signed tier manifest lists it (`PENDING_MANIFEST_KINDS`, like F4/M9 and `pro-conjunction-validation`); until then
it is built and tested as a pack and refused by the dispatcher like any unknown kind.

## Worked example (synthetic)

`docs/evidence/rideshare-id/` holds a synthetic deployment made by `rideshare-id synth` (defaults: 12 objects
numbered 101388–101399, written in TLE as Alpha-5 `A1388`–`A1399` and in OMM, released 8 km apart along one
sun-synchronous orbit with 0.5 km/rev relative drift; three passes over one station at 59.4° N, 24.7° E;
Doppler at 437.5 MHz with σ = 20 Hz and an unknown 1.25 kHz oscillator offset; the target's catalogue elements
0.3 km off its true position) and its verdict (`verdict/`, unsigned). Result: `IDENTIFIED`, object 101393
(`A1393`), reduced χ² 0.86 over 85 measurements, runner-up 101394 behind by Δχ² ≈ 220, the two leading
candidates' predictions 16 sigma apart and their positions 9–10 km apart. `tests/rideshare_id.rs` re-runs it and
requires the same `result_sha256`.

Tests (`tests/rideshare_id.rs`): the conformance vectors; Alpha-5 over its whole range; six-digit numbers through
TLE and OMM (KVN, XML, JSON) end to end, including numbers above 339,999 in OMM only; 12 objects with Doppler only
(every object found whichever is the target, across noise seeds); 20 objects with angles and ranging from two
stations and an OMM-only target above 339,999; two objects 50 m apart reported `AMBIGUOUS` across noise seeds; a
set without the object reported `NO_CONSISTENT_CANDIDATE`; too little data; the CSV checks; the signed directory
and its tamper check; the command; the entitlement.

## Not built (deliberately)

* No optical (angles from telescopes with star-field astrometry) or radar ingest beyond the generic `az_deg`,
  `el_deg`, `range_m` rows; no CCSDS TDM reader here (the cislunar SSA module has one for angles).
* No orbit determination: candidates are tested as catalogued, not refined. A refined orbit from the operator's
  passes would be a follow-on (and would need a covariance to stay honest).
* No beacon decoding or SatNOGS client; the observations are whatever the operator's stations logged.
* No time-offset estimation per candidate: along-track timing is exactly what separates rideshare objects, so
  absorbing it would erase the evidence.
