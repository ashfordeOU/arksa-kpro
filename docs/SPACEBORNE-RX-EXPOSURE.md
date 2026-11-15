# Spaceborne GNSS receiver interference assurance (roadmap 0.42 F4)

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

## Gaps

- Public interference maps with cell locations for a recent period (the open observatory's 30 Baltic days) are not
  in git (about 3.4 GB of archive per day); the reader takes them as they are (v1 GeoJSON) and the study can be
  rerun on them. The committed study uses the located emitter-region sessions instead.
- No receive-antenna pattern (a constant back-lobe gain), no attitude, no Earth-limb or body blockage, no
  atmospheric loss; one emitter point per area.
- Element sets are current while the map is from 2025, so the committed study is a climatology; a coincident study
  needs historical element sets for the map days (not public without an account) or a current map.
