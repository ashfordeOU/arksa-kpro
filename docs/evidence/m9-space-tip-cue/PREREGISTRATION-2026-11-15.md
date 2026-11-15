# M9: space tip-and-cue study (constellation trade with cost): pre-registration

PRIVATE (arksa-kpro). Arksa-exclusive under roadmap 0.42 section 1a (space-mission jobs). Never copy to a
public place.

**Status: drafted and committed on 15 November 2026 by the build session, before any M9 code. Owner sign-off
pending.** Every number below is fixed now. After a run, a bar may be tightened, never loosened; a miss is
recorded in `RESULTS.md` with its numbers and its test asserts the recorded miss.

## What M9 is (roadmap 0.41-0.43, row M9; 0.42 F6)

"Maritime tip-and-cue study: constellation trade (pairs per plane against revisit, reaction and capture rate),
with the design optimiser (B1b cost axis). Pack on top of M1 to M3. Done when: the study reproduces from
scenario and seed." Moved from Kshana Pro F6 to arksa-kpro (owner rule of 14 November 2026: space-mission jobs
are Arksa-exclusive).

The bar is reproducibility, not agreement with any outside figure. The study is MODELLED whatever the outcome:
no measured revisit, reaction or capture data is used, and nothing here is VALIDATED.

## What it builds on (open engine 0.41.0, unchanged)

- M2 (`kshana::tip_and_cue`): Walker star layers at 550 km, sun-synchronous 97.59 deg, J2 secular, RF field of
  regard by elevation mask, optical by off-nadir angle; access intervals; R (revisit), F (full-area refresh) and
  Q (RF-to-optical reaction) with the M2 definitions.
- M3 (pre-registration only; not run): the fix noise `sigma_fix = 1 km` per axis (configuration B), vessel speed
  16 to 22 kn, three fixes before the anchor, footprint radii 2.5, 5 and 10 km.
- M1 is not used numerically: `sigma_fix` stands in for the RF fix quality (M1 bounds are optimistic, see M1
  RESULTS).

## The area and grid (fixed now)

- Points: the M2 Baltic sea points (`kshana::tip_and_cue::baltic_points`, 996 points) thinned to every second
  cell in latitude and in longitude: keep a point when `round((lat - 53.5)/0.25 - 0.5)` and
  `round((lon - 9.5)/0.25 - 0.5)` are both even. Area weighting `cos(lat)` as in M2.
- Epoch 2025-01-01 00:00 UTC (M2's). Span 3 days, step 20 s. F and Q use start times in the first 2 days (the
  engine's rule: span minus one day).
- RF mask 10 deg, optical off-nadir 45 deg, no daylight rule (M2 primary).

## The trade (fixed now)

Each design has an RF layer of `P` planes x `k` satellites per plane (Walker star, phasing 1, first node 0) and
an optical layer of the same `P` and `k`, its nodes offset by half a plane spacing (the M2 convention):

| `P` planes | `k` per plane | total per layer |
|---|---|---|
| 4 | 1, 2, 4 | 4, 8, 16 |
| 8 | 1, 2, 4 | 8, 16, 32 |
| 16 | 1, 2, 4 | 16, 32, 64 |

Nine designs. (8, 2), (16, 2) and (8, 4) are the M2 cases primary-16, primary-32 and V1-32-8x4.

## Cost axis (B1b convention; illustrative numbers, the user's to replace)

`cost = capex + years x opex`, in millions of a currency unit, with `years = 5`:
capex = satellites x (2.0 per RF satellite + 3.0 per optical satellite) + planes x 2 layers x 1.5 per plane
(one launch slot per plane); opex = 0.1 per satellite per year. The numbers are illustrative and labelled so.

## Capture rate (fixed now)

A cue is the start `s` of an RF access interval at a point (as in M2's Q); its wait `Q_c` is the time to the
first optical access at or after `s` (zero if in view). A cue with no optical access before the span ends is a
miss.

Monte Carlo, `N = 4000` cues per design, generator ChaCha8 seeded with the scenario's `seed` (default 9009),
one stream per design (seed + design index):
1. Draw a point with probability proportional to `cos(lat)`, then one of its cues uniformly.
2. Fix spacing `D` = that point's mean start-to-start interval between RF access intervals (at least 60 s).
3. Vessel: speed uniform in [16, 22] kn, heading uniform in [0, 360) deg, straight from `t = -2D` to
   `t = Q_c`; with probability `p_turn = 0.25` one instantaneous turn at a time uniform in `[-2D, Q_c]` by an
   angle uniform in [20, 90] deg, sign uniform.
4. Fixes at `t = -2D, -D, 0`: truth plus independent normal noise `sigma_fix = 1 km` per east and north axis.
5. Prediction: least-squares straight line (constant velocity) through the three fixes, evaluated at `Q_c`.
6. Captured at radius `r` when the prediction error is at most `r`; `r` in {2.5, 5, 10} km, primary 5 km.

The capture rate is the fraction captured. A local flat east-north plane is used (errors of a few km).

## Outputs (reported, not scored)

Per design: R, F, Q (mean, median, 95th percentile, from the engine), cues and unanswered cues, capture at the
three radii, cost; the exact non-dominated front over (cost lower, capture at 5 km higher, Q lower); and the
cheapest design meeting a target (default: capture at 5 km at least 0.90 and Q at most 15 min).

## Bars (what "M9 passes its tests" means)

- **M9-R1 (reproduces from scenario and seed).** Two runs of the same scenario and seed give byte-identical
  result JSON and the same result SHA-256.
- **M9-R2 (seed stability).** With seed 9010 instead of 9009, every design's capture rate at 5 km is within
  `4 x sqrt(2 p (1 - p) / N)` of the first run's (`p` the first run's rate; at least 0.01 when `p` is 0 or 1).
- **M9-R3 (on top of M2, wiring).** For (8, 2), (16, 2) and (8, 4), the pack's R, F and Q equal
  `kshana::tip_and_cue::run` on the same points and grid to a relative 1e-12.
- **M9-R4 (Monte Carlo against closed form).** With `p_turn = 0`, the prediction error per axis is normal with
  variance `sigma_fix^2 (1/3 + (Q_c + D)^2 / (2 D^2))`, so `P(capture) = 1 - exp(-r^2 / (2 s^2))`. For every design
  the Monte Carlo rate at 5 km is within `4 x sqrt(sum P_i (1 - P_i)) / N` (at least 0.005) of the mean of the
  closed-form `P_i` over the same sampled cues.

All four met: M9 passes. R1 failing makes the study void.

## Expectation (written before any run)

Without turns, capture at 5 km should be above 0.95 for every design (a rough count: `D` near 30 min and `Q`
near 20 min give `s` about 1.3 km). Turns drive the misses: a 60 deg turn 10 min before the look moves the
vessel about 6 km off the straight line, so capture at 5 km is expected near 0.85 to 0.95 and to fall as Q
grows. More planes should shorten Q more than more satellites per plane at equal total (M2's V1 said the
opposite for revisit; this is not known for Q). Cost grows with planes faster than with satellites per plane.

## Scope limits

Geometry and timing only (M2's), no detection probability, no tasking or downlink latency, no cloud or
daylight, no real vessel tracks (M3 not run), illustrative costs. Neutral wording: no company or programme
names in code, scenario or result.
