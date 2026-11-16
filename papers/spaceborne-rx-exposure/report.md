# Exposure of four LEO GNSS receivers to the located Sep 2025 northern Baltic emitter-region sessions

**Evidence class: MODELLED.** MODELLED spaceborne GNSS receiver interference exposure. MEASURED: nothing on board; the inputs are published orbital elements (TLEs, propagated with SGP4) and interference maps built from aircraft or ship navigation-accuracy reports, which say where reports were degraded, not where an emitter was or how strong it was. MODELLED: the interferer EIRP (assumed, several values), the receive-antenna gains, free-space propagation, the anti-jam equation and, when the map period differs from the orbit period, the use of the map as a climatology. Not a statement about any receiver's measured performance.

Span 2026-11-15T00:00:00Z to 2026-11-16T00:00:00Z, step 30.0 s, time mode **climatology**: climatology: the areas' time windows do not overlap the span, so every area counts all the time; the result says what the orbit would see if the mapped areas were active, not what happened

## Inputs

- Emitter-region summary adsb 2025-09-14 to 2025-09-19 (ODbL-1.0; Contains aircraft position data from adsb.lol, made available under the Open Database License (ODbL 1.0), https://opendatacommons.org/licenses/odbl/1-0/): 4 located sessions used, 10 not located (not used), SHA-256 d5996b99961614d4153fc1193d6e9a695278d4f4f9ce2045504e3ce07ce1919a

Assumed interferer EIRP per area: 0, 20, 40 dBW. Receiver: {"degraded_margin_db":6.0,"gain_toward_gnss_dbi":0.0,"gain_toward_ground_dbi":-15.0,"q":2.0,"signal_power_dbw":-158.5,"system_noise_temp_k":290.0,"tracking_threshold_dbhz":25.0}. Nominal C/N0 45.5 dB-Hz.

## Results per spacecraft

| Spacecraft | TLE epoch | TLE age (d) | Passes | Exposed (min) | Over areas (min) | Share | Worst margin (dB) per EIRP | Verdict per EIRP |
|---|---|---|---|---|---|---|---|---|
| SWARM A | 2026-11-15T06:23:14Z | 0.2 | 13 | 106.0 | 6.0 | 0.074 | 20.4 / 16.4 / -1.5 | kept / kept / loss-of-tracking-modelled |
| SENTINEL-6A | 2026-11-14T20:24:09Z | 0.6 | 8 | 169.0 | 11.0 | 0.117 | 20.5 / 19.8 / 7.7 | kept / kept / kept |
| GRACE-FO 1 | 2026-11-15T06:03:58Z | 0.2 | 13 | 111.0 | 4.0 | 0.077 | 20.4 / 17.2 / -0.1 | kept / kept / loss-of-tracking-modelled |
| ISS (ZARYA) | 2026-11-15T04:56:49Z | 0.3 | 5 | 35.5 | 0.0 | 0.025 | 20.5 / 19.7 / 7.5 | kept / kept / kept |

Per-pass detail: `passes.csv`. Full result: `study.json` (result SHA-256 3ed4722782ebbf7971c062449e9793b48cb9895ee10d1ecf77f0b06edfff6b5c).

## What this does and does not say

- It says how long the orbit spends with mapped affected areas in radio view and, for each assumed interferer power, what the anti-jam equation gives for the receiver's margin.
- It does not say that any emitter was on air when the spacecraft passed (unless the time mode is coincident, and then only as the map says), how strong it was, or how a real receiver behaved. No on-board measurement is used.
- A map cell or session marks where aircraft or ships reported degraded navigation; its centre is used as the emitter point, an assumption.
