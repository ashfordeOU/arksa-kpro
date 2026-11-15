# M9: space tip-and-cue study: results

PRIVATE (arksa-kpro). **MODELLED whatever the outcome; never VALIDATED.**

Pre-registration: `PREREGISTRATION-2026-11-15.md` (commit 7fdf612, before any M9 code; owner sign-off pending).
Run once with the pre-registered scenario (every field at its default, seed 9009) on 15 November 2026, open engine
0.41.0, Kshana Pro f191606a. Nothing was changed after the run. Output: `papers/space-tip-cue/trade.csv` and
`result.json` (result SHA-256 `db79af2b8932b1bcbad6dab8c669572ac650a18e7b32924405f755bb43b3e951`), pinned by
`tests/space_tip_cue_study.rs`.

## Verdict: M9 passes (R1 to R4 all met)

| Bar | Result |
|---|---|
| **M9-R1** reproduces from scenario and seed | **MET**: two runs give byte-identical JSON and CSV |
| **M9-R2** seed stability (9009 against 9010, capture at 5 km) | **MET**: largest difference 0.0140 (4x2), tolerance 0.0360; every design inside |
| **M9-R3** R, F, Q equal the open M2 planner on the same points and grid (8x2, 16x2, 8x4) | **MET**: equal to 1e-12 relative, unanswered cues equal |
| **M9-R4** Monte Carlo against closed form with no turns | **MET**: largest difference 0.0069 (4x2), tolerance 0.0152; every design inside |

R4 detail (capture at 5 km, `p_turn = 0`): 4x1 0.9523 / 0.9500; 4x2 0.9110 / 0.9179; 4x4 0.8662 / 0.8650; 8x1
0.8420 / 0.8444; 8x2 0.9790 / 0.9794; 8x4 0.9710 / 0.9692; 16x1 0.9945 / 0.9903; 16x2 0.9922 / 0.9929; 16x4
0.9902 / 0.9876 (Monte Carlo / closed form).

## The trade (reported, not scored)

247 sea points, 3 days at 20 s from 2025-01-01, F and Q over the first 2 days; times in minutes; costs illustrative
(M, `capex + 5 x opex`, both layers).

| Design (planes x per plane) | Sats per layer | Cost | R mean | F mean | Q mean | Q median | Q p95 | Capture 2.5 / 5 / 10 km | Front |
|---|---|---|---|---|---|---|---|---|---|
| 4x1 | 4 | 36 | 61.92 | 52.21 | 74.20 | 42.33 | 193.67 | 0.587 / 0.795 / 0.878 | yes |
| 4x2 | 8 | 60 | 28.39 | 25.58 | 46.18 | 29.33 | 145.00 | 0.574 / 0.796 / 0.901 | yes |
| 4x4 | 16 | 108 | 16.08 | 12.43 | 41.66 | 27.33 | 122.00 | 0.539 / 0.780 / 0.903 | yes |
| 8x1 | 8 | 72 | 28.55 | 31.51 | 75.28 | 14.00 | 335.33 | 0.572 / 0.754 / 0.880 | no |
| 8x2 | 16 | 120 | 24.74 | 14.84 | 22.64 | 8.33 | 97.33 | 0.691 / 0.873 / 0.950 | yes |
| 8x4 | 32 | 216 | 13.77 | 6.31 | 14.41 | 5.00 | 87.33 | 0.759 / 0.933 / 0.977 | yes |
| 16x1 | 16 | 144 | 32.69 | 26.88 | 25.84 | 12.67 | 78.67 | 0.695 / 0.854 / 0.939 | no |
| 16x2 | 32 | 240 | 23.04 | 12.96 | 13.09 | 7.00 | 59.67 | 0.753 / 0.915 / 0.979 | yes |
| 16x4 | 64 | 432 | 12.73 | 5.69 | 7.97 | 4.00 | 43.00 | 0.817 / 0.962 / 0.984 | yes |

Non-dominated over (cost, capture at 5 km, Q mean): 4x1, 4x2, 4x4, 8x2, 8x4, 16x2, 16x4. Cheapest design meeting
the default target (capture at 5 km at least 0.90 and Q at most 15 min): **8x4** (32 per layer, cost 216).

## Against the written expectation

- "Without turns, capture at 5 km above 0.95 for every design": **wrong** for five of nine designs (closed form
  0.84 to 0.92 for 4x2, 4x4 and 8x1; the long Q tails with short fix spacing make the line extrapolation
  uncertain). It held for 4x1, 8x2, 8x4 and the 16-plane designs.
- "With turns, capture at 5 km near 0.85 to 0.95 and falling as Q grows": held for designs with Q under about
  25 min (0.85 to 0.96); the 4-plane and 8x1 designs are lower (0.75 to 0.80).
- "More planes shorten Q more than more satellites per plane at equal total": held at 16 satellites (16x1 Q 25.8
  min against 4x4 41.7) and at 32 (16x2 13.1 against 8x4 14.4, a small margin), while more satellites per plane
  shortens revisit more (8x4 R 13.8 against 16x2 23.0), as M2's V1 showed.

## What this says and does not say

It says the trade reproduces exactly from its scenario and seed, that its timing figures are the open planner's,
and that its capture Monte Carlo agrees with the closed form where one exists. It does not say any constellation
achieves these figures in orbit, that the vessel model is right (M3, the real-track test, has not run), or that the
costs are anyone's. MODELLED.
