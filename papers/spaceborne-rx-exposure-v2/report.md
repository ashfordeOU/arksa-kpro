# Per-revolution exposure forecast of four LEO GNSS receivers to the located Sep 2025 northern Baltic emitter-region sessions, with matching receiver test scenarios

**Evidence class: MODELLED.** MODELLED spaceborne GNSS receiver interference exposure. MEASURED: nothing on board; the inputs are published orbital elements (TLEs, propagated with SGP4) and interference maps built from aircraft or ship navigation-accuracy reports, which say where reports were degraded, not where an emitter was or how strong it was. MODELLED: the interferer EIRP (assumed, several values), the receive-antenna gains, free-space propagation, the anti-jam equation and, when the map period differs from the orbit period, the use of the map as a climatology. Not a statement about any receiver's measured performance.

Span 2026-11-15T00:00:00Z to 2026-11-16T00:00:00Z, step 30.0 s, time mode **climatology**: climatology: the areas' time windows do not overlap the span, so every area counts all the time; the result says what the orbit would see if the mapped areas were active, not what happened

## Inputs

- Emitter-region summary adsb 2025-09-14 to 2025-09-19 (ODbL-1.0; Contains aircraft position data from adsb.lol, made available under the Open Database License (ODbL 1.0), https://opendatacommons.org/licenses/odbl/1-0/): 4 located sessions used, 10 not located (not used), SHA-256 d5996b99961614d4153fc1193d6e9a695278d4f4f9ce2045504e3ce07ce1919a

Assumed interferer EIRP per area: 20, 40, 50 dBW. Receiver: {"degraded_margin_db":6.0,"gain_toward_gnss_dbi":0.0,"gain_toward_ground_dbi":-15.0,"q":2.0,"signal_power_dbw":-158.5,"system_noise_temp_k":290.0,"tracking_threshold_dbhz":25.0}. Nominal C/N0 45.5 dB-Hz.

## Results per spacecraft

| Spacecraft | TLE epoch | TLE age (d) | Passes | Exposed (min) | Over areas (min) | Share | Worst margin (dB) per EIRP | Verdict per EIRP |
|---|---|---|---|---|---|---|---|---|
| SWARM A | 2026-11-15T06:23:14Z | 0.2 | 13 | 106.0 | 6.0 | 0.074 | 19.5 / 6.2 / -3.7 | kept / kept / loss-of-tracking-modelled |
| SENTINEL-6A | 2026-11-14T20:24:09Z | 0.6 | 8 | 169.0 | 11.0 | 0.117 | 20.4 / 15.4 / 6.8 | kept / kept / kept |
| GRACE-FO 1 | 2026-11-15T06:03:58Z | 0.2 | 13 | 111.0 | 4.0 | 0.077 | 19.6 / 7.0 / -2.8 | kept / kept / loss-of-tracking-modelled |
| ISS (ZARYA) | 2026-11-15T04:56:49Z | 0.3 | 5 | 35.5 | 0.0 | 0.025 | 19.9 / 9.0 / -0.7 | kept / kept / loss-of-tracking-modelled |

Per-pass detail: `passes.csv`. Full result: `study.json` (result SHA-256 247b8765036fb9c4098f512c43e8792d9d3ea6f2207bc3e4e91e6b112091ee35).

## What this does and does not say

- It says how long the orbit spends with mapped affected areas in radio view and, for each assumed interferer power, what the anti-jam equation gives for the receiver's margin.
- It does not say that any emitter was on air when the spacecraft passed (unless the time mode is coincident, and then only as the map says), how strong it was, or how a real receiver behaved. No on-board measurement is used.
- A map cell or session marks where aircraft or ships reported degraded navigation; its centre is used as the emitter point, an assumption.

## Per-revolution forecast

**MODELLED per-revolution exposure forecast. The areas come from the maps and regions given (where interference was reported or is expected on the ground; no emitter is located), their activity probabilities are inputs, the interferer EIRP is assumed, and the receiver link margin is the stated model. A forecast of what the model gives for the orbit, not a prediction that any interferer will be on air.**

Link model: {"antenna_pattern":[[0.0,3.0],[30.0,2.0],[60.0,-1.0],[80.0,-4.0],[90.0,-7.0],[100.0,-10.0],[120.0,-15.0],[150.0,-20.0],[180.0,-22.0]],"boresight":"the geocentric zenith (a zenith-pointing antenna; no attitude, no body or limb blockage)","other_losses_db":0.0,"polarisation_loss_db":3.0,"reacquisition_s":20.0,"replaces":"receiver.gain_toward_ground_dbi (not used when the forecast is on)"}. Revolutions: a revolution starts at each ascending node (ITRS z crossing upward, interpolated between steps); the first and last revolutions of the span are partial Activity: each area is active for a whole revolution with its activity probability, independently of the others; exact enumeration up to 10 uncertain areas in view per revolution, else a seeded Monte Carlo draw of 1024 samples Outage window: from the first step with a modelled margin under zero to the last one, plus reacquisition_s; runs closer than that are merged.

| Spacecraft | Revolutions | Exposed (min) | Expected exposed (min) | Outage (min) per EIRP | Expected outage (min) per EIRP | Revolutions with outage per EIRP |
|---|---|---|---|---|---|---|
| SWARM A | 17 | 106.0 | 106.0 | 0.0 / 0.0 / 32.2 | 0.0 / 0.0 / 32.2 | 0 / 0 / 5 |
| SENTINEL-6A | 14 | 169.0 | 169.0 | 0.0 / 0.0 / 0.0 | 0.0 / 0.0 / 0.0 | 0 / 0 / 0 |
| GRACE-FO 1 | 16 | 111.0 | 111.0 | 0.0 / 0.0 / 27.2 | 0.0 / 0.0 / 27.2 | 0 / 0 / 5 |
| ISS (ZARYA) | 17 | 35.5 | 35.5 | 0.0 / 0.0 / 7.2 | 0.0 / 0.0 / 7.2 | 0 / 0 / 2 |

EIRP order: 20, 40, 50 dBW. Per revolution: `orbits.csv`. Predicted outage windows per pass: `outages.csv`.

## Matching receiver test scenarios

MODELLED: receiver test scenarios generated from this study; the J/S profiles follow from the assumed EIRP and the link model; the criteria are stated before any run. Not a result of any receiver test.

Format `kshana-testkit-scenario/1`, EIRP 50.0 dBW, passes `outage`: 12 selected, 0 without jamming events over the floor.

| Scenario | Spacecraft | Predicted | Worst margin (dB) | Events | Max J/S (dB) | SHA-256 |
|---|---|---|---|---|---|---|
| `scenarios/rx-exposure-swarm-a-20261115t185500-eirp50.toml` | SWARM A | outage | -3.7 | 7 | 42.0 | 205fa6fdbfcb7c90 |
| `scenarios/rx-exposure-swarm-a-20261115t064500-eirp50.toml` | SWARM A | outage | -3.5 | 8 | 42.0 | 0c310540b4aa223a |
