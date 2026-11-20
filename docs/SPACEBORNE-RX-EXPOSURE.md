# Spaceborne GNSS receiver interference assurance (roadmap 0.42 F4; v2 AK4)

PRIVATE (arksa-kpro). Arksa-exclusive (space-mission job, roadmap 0.42 section 1a). Public outcome name allowed by
X8: "Spaceborne GNSS receiver interference assurance". Nothing below is for public copy.

Kind `pro-spaceborne-rx-exposure` (also the feature of the command `arksa-kpro rx-exposure`). Code:
`src/packs/spaceborne_rx_exposure_pack.rs`. Tests: `tests/spaceborne_rx_exposure.rs`.

## What it does

For each spacecraft (two-line element sets, propagated with the open engine's SGP4, TEME to ITRS) over a span at a
fixed step, against affected areas from:

- interference maps in the open engine's v1 GeoJSON (`kshana_interference_map`): `degraded` and `anomalous` cells,
  each active on its own UTC day; the open reader (`interference_map::route::load_map`) checks the document;
- emitter-region summaries (`kshana_emitter_region`): sessions with a centroid and a 95 % ellipse, active in their
  session window; sessions without a location are counted and not used;
- areas the user states (circle, optional window).

An area is in radio view when the spacecraft is at or above `min_elevation_deg` seen from the area's emitter point
(its centre, at `emitter_height_m`). Outputs, per spacecraft: passes (maximal runs of steps with an area in view)
with duration, time with the sub-satellite point over an area, highest elevation, shortest range, areas seen, and
for each assumed EIRP the worst modelled effective C/N0 margin over the tracking threshold and the time under zero
and under the degraded margin; totals; a verdict per EIRP (`kept`, `degraded`, `loss-of-tracking-modelled`).

The margin: the interference power from every area in view (EIRP + receiver gain toward the ground − free-space
loss at L1, summed in power), J/S against the received GNSS power, the open engine's anti-jam equation
(`jamming::effective_cn0_dbhz`), minus the threshold. Defaults (all stated in each result): EIRP 0, 20 and 40 dBW;
gain toward the ground −15 dBi (back lobe of a zenith antenna, an assumption); toward GNSS 0 dBi; signal −158.5
dBW; 290 K; threshold 25 dB-Hz; degraded margin 6 dB; Q = 2.

Time mode: `coincident` counts an area only in its window; `climatology` counts every area all the time (the map
used as a picture of where interference has been seen); `auto` picks coincident when any window overlaps the span.

## Evidence class

MODELLED. Measured: nothing on board. The maps say where aircraft or ships reported degraded navigation, not where
an emitter was or how strong; EIRP, gains, free-space propagation and the emitter point are assumptions; SGP4
accuracy falls with element-set age (reported per spacecraft). No result may be called measured, validated,
compliant or certified.

## The signed study report

`arksa-kpro rx-exposure <scenario.toml> --out <dir> [--sign-key-file <file>] [--signer <name>]` writes
`study.json`, `passes.csv` and `report.md`, and with a key signs the directory with Kshana Pro's dossier signature
(`signature.json`, Ed25519 over every file's SHA-256); `arksa-kpro dossier verify <dir> --trust <key>` checks it.
The command is registered once Kshana Pro's signed tier manifest lists the feature (see README, "F4 and M9").

## The committed study (bar: a study on public interference maps and TLEs)

`papers/spaceborne-rx-exposure/` (scenario, whose file names resolve against `tests/fixtures/f4/`, `study.json`, `passes.csv`, `report.md`; unsigned in the repository,
signed on delivery). Inputs, both public:

- `tests/fixtures/f4/leo-celestrak-2026-11-15.tle`: CelesTrak GP element sets of Swarm A, Sentinel-6A, GRACE-FO 1
  and the ISS, fetched 15 November 2026 13:14 UTC (epochs 14 to 15 November 2026);
- `tests/fixtures/f4/emitter-adsb-adsb-lol-2025-09-14_2025-09-19-summary.json`: the open repository's
  emitter-region summary from adsb.lol (ODbL 1.0; open commit 5fda904ec), 14 sessions, 4 located (northern
  and central Finland), 10 not located and not used.

Span 15 November 2026, 24 h, 30 s step; climatology mode (the map is from September 2025). Result
(SHA-256 `af2d379b...`, pinned by the test):

| Spacecraft | Passes | Exposed (min) | Over areas (min) | Worst margin at 0 / 20 / 40 dBW (dB) |
|---|---|---|---|---|
| Swarm A | 13 | 106.0 | 6.0 | 20.4 / 16.4 / −1.5 |
| Sentinel-6A | 8 | 169.0 | 11.0 | 20.5 / 19.8 / 7.7 |
| GRACE-FO 1 | 13 | 111.0 | 4.0 | 20.4 / 17.2 / −0.1 |
| ISS | 5 | 35.5 | 0.0 | 20.5 / 19.7 / 7.5 |

Read: under these assumptions a ground interferer of 40 dBW EIRP at a mapped location could take a low (about 500
km) polar spacecraft's receiver under its tracking threshold for a minute or two on an overhead pass; 0 and 20 dBW
leave more than 16 dB. The higher orbit (Sentinel-6A, 1 336 km) keeps 7.7 dB. These are model statements, not
observations.

## v2 (AK4): per-revolution forecast, link-margin model, outage windows, test-scenario export

Added 20 November 2026 (research 2026-11-20: 05 finding 6, the DTU "deaf zones" felt at about 700 km; 06 G4, an
in-orbit RFI exposure forecast and receiver test for spacecraft operators; 06b, the Humphreys preprint on L1
interference from Molniya-orbit satellites, context only). Code: the same pack; tests:
`tests/spaceborne_exposure_v2.rs` (10), unit tests in the pack (4 new), and one invariant in
`tests/no_overclaims_mission_analysis.rs`. All of it is optional: a scenario without the new sections gives the F4
result unchanged (the committed F4 study's hash is the same).

### Maps of ground interference regions (input; no emitter geolocation)

`region_files = [...]` (or `region_json`): a GeoJSON FeatureCollection with an `arksa_interference_regions` header
(`version` 1, `title`, `source`, `licence`, `evidence`, optional `valid_from_utc`/`valid_to_utc`) and one feature
per region: a `Polygon` (outer ring; holes not used) or a `Point` with `radius_km`. Properties: `id`, `label`,
`activity` (0 to 1, the probability the region is active during a revolution; default 1) and `window_utc` (default
the map's validity). A region is where interference is reported or expected on the ground (a GPSJam-style picture,
an operator's own list, the published "deaf zones"); nothing here estimates where an emitter is. The interferer point
of a region is its reference point (the vertex mean, or the circle's centre), an assumption stated in every result.
Stated `[[area]]` entries take `activity` too. Fixture: `tests/fixtures/f4/synthetic-regions.geojson` (SYNTHETIC).

### `[forecast]`: link-margin model, outage windows, per-revolution forecast

```toml
[forecast]
reacquisition_s = 20.0          # added to the end of every outage window (assumption)
monte_carlo_samples = 1024      # only when a revolution sees more than 10 uncertain areas
[forecast.link]
antenna_pattern = [[0.0, 3.0], [30.0, 2.0], [60.0, -1.0], [80.0, -4.0], [90.0, -7.0],
                   [100.0, -10.0], [120.0, -15.0], [150.0, -20.0], [180.0, -22.0]]   # deg from boresight, dBi
polarisation_loss_db = 3.0      # linear interferer on a RHCP antenna (assumption)
other_losses_db = 0.0
```

- **Receiver link-margin model.** Per area in view: interference = EIRP + G(angle from boresight) − polarisation loss
  − other losses − free-space loss at L1; the boresight is the geocentric zenith (zenith-pointing antenna, no
  attitude). Powers from all areas in view add; J/S against `[receiver]` signal power and gain; the open engine's
  anti-jam equation gives the effective C/N0; the margin is that minus the tracking threshold. The pattern replaces
  `receiver.gain_toward_ground_dbi`. The default pattern is illustrative (a zenith patch with about 25 dB front to
  back), not any product's measured pattern; a mission gives its own.
- **Per pass:** `link_budget` at the worst step (dominant area, range, angle from boresight, gain, losses, aggregate
  coupling, and per EIRP the interference power, J/S, effective C/N0 and margin) and `outage_windows` per EIRP (first
  to last step with a margin under zero, plus `reacquisition_s`; runs closer than that merged), each with its loss
  time, window length, worst margin and the areas in view at the worst step. `outages.csv`.
- **Per revolution** (from ascending node to ascending node, interpolated between steps; the first and last are
  partial): exposure, expected exposure (any area in view active, areas independent), time over areas, worst margin,
  outage time with every area active, and, over the areas' activity, the expected outage time and the probability of
  any outage on the revolution. Each area is on or off for a whole revolution, independently; exact enumeration up
  to 10 uncertain areas in view per revolution, else a seeded Monte Carlo draw (ChaCha8, seed from spacecraft and
  revolution; the same on every run). Totals per spacecraft. `orbits.csv`.

### `[export]`: matching receiver test scenarios (`kshana-testkit-scenario/1`)

Kshana Pro's neutral test-as-code format (kshana-pro #50, runner #51; this crate is pinned to kshana-pro
79293979, which has both). One scenario per selected pass, worst margin first:

```toml
[export]
eirp_dbw = 50.0          # one of assumed_eirp_dbw (default the largest)
include = "outage"       # outage | degraded | all
lead_s = 120.0           # baseline before the pass
tail_s = 300.0           # time for recovery after it
sample_step_s = 5.0      # trajectory samples at least this often
max_interpolation_error_m = 10.0   # samples are bisected (to 50 ms) until linear interpolation is within this
js_step_db = 3.0         # J/S quantisation; one jamming event per run of equal value
js_floor_db = 10.0       # runs under this are left out
bandwidth_hz = 2.046e6   # wideband jamming at L1
detect_bound_s = 10.0
recovery_bound_s = 120.0
availability_bound = 0.95
misleading_h_error_m = 100.0
recover_h_error_m = 50.0
max_scenarios = 50
```

- Scene: from `lead_s` before the pass to `tail_s` after it. Trajectory: the SGP4-propagated sub-satellite point and
  height as time-tagged samples (longitudes kept on the map; two samples 1 ms apart at an antimeridian crossing); the
  largest interpolation error left is stated in the scenario and in the index. The format has no spacecraft
  platform: `aircraft` labels the scenario and the tags say `spacecraft`; a simulator must support orbital dynamics.
- Threats: the modelled J/S of the pass at the exported EIRP as wideband jamming events at L1 (signal-parameter
  level only; no waveform or IQ).
- Criteria, stated before any run: a pass with a modelled outage needs detection (fix withdrawn or flagged, measured
  from the jamming event in which the loss starts), no misleading fix and recovery; a degraded or kept pass needs
  availability, no misleading fix and recovery.
- Written under `scenarios/` in the study directory (covered by the signature); `study.json` lists each scenario's
  id, file and SHA-256 (Kshana Pro's spec identity, carried by every verdict) under `test_scenarios`. Each scenario
  passes Kshana Pro's `Spec::parse`; `kshana-pro testkit validate|compile|run` take them as they are.

### Entitlement

`arksa_kpro::entitlement::ensure_rx_exposure_entitled` (the kind id, Arksa claim): the command and the library entry
points `run_study_entitled` and `run_study_with_scenarios` refuse anything else with the neutral message. The kind
itself runs through Kshana Pro's dispatcher gate as before.

### The committed v2 study

`papers/spaceborne-rx-exposure-v2/` (scenario, `study.json`, `passes.csv`, `orbits.csv`, `outages.csv`, `report.md`,
`scenarios/` with the two worst passes): the same public inputs as the F4 study, assumed EIRP 20, 40 and 50 dBW, the
default link model, export at 50 dBW. Numbers rounded to 9 significant digits before hashing, as in F4. Result
(SHA-256 `247b8765...`, pinned by the test):

| Spacecraft | Revolutions | Worst margin at 20 / 40 / 50 dBW (dB) | Revolutions with a modelled outage at 50 dBW | Outage at 50 dBW (min) |
|---|---|---|---|---|
| Swarm A | 17 | 19.5 / 6.2 / −3.7 | 5 | 32.2 |
| Sentinel-6A | 14 | 20.4 / 15.4 / 6.8 | 0 | 0 |
| GRACE-FO 1 | 16 | 19.6 / 7.0 / −2.8 | 5 | 27.2 |
| ISS | 17 | 19.9 / 9.0 / −0.7 | 2 | 7.2 |

Read: with the stated pattern and a 3 dB polarisation loss, 40 dBW per area leaves every receiver at least 6 dB; 50
dBW takes the lower polar orbits under the threshold for several minutes on passes near the mapped sessions. The
activity of the emitter-region sessions is unknown and left at 1, so the expected values equal the all-active ones.
Model statements, not observations.

### Not built (deliberately)

- No emitter geolocation or direction finding, and no estimate of an emitter's power: regions and EIRP are inputs.
- No anti-jam processing, nulling or CRPA modelling: a single antenna with a fixed pattern.
- No interference from orbit (the Molniya-orbit L1 events in 06b): ground regions only.
- No attitude, body or limb blockage, atmospheric loss or terrain; one interferer point per area.
- The exported scenarios have not been run on a simulator or a receiver here; the simulator profiles in Kshana Pro
  are unconfirmed templates, and whether a given simulator plays a 400 km orbit from time-tagged samples is
  unverified.

## Gaps

- Public interference maps with cell locations for a recent period (the open observatory's 30 Baltic days) are not
  in git (about 3.4 GB of archive per day); the reader takes them as they are (v1 GeoJSON) and the study can be
  rerun on them. The committed study uses the located emitter-region sessions instead.
- No receive-antenna pattern (a constant back-lobe gain), no attitude, no Earth-limb or body blockage, no
  atmospheric loss; one emitter point per area.
- Element sets are current while the map is from 2025, so the committed study is a climatology; a coincident study
  needs historical element sets for the map days (not public without an account) or a current map.
