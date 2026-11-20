# Synthetic deployment of 12 objects

Evidence class: **MODELLED**. MODELLED rideshare object identification: the candidate orbits are propagated with SGP4 from the element sets given; the observations are the operator's own; the probabilities assume Gaussian measurement errors (scaled by the best fit's variance factor) and that the operator's object is one of the candidates. Not an identification by any catalogue authority. MEASURED: only the observations supplied.

## Verdict: IDENTIFIED

- best: 101393 (TLE A1393) with reduced chi-square 0.858 over 85 measurements; probability 1.000000 under a uniform prior over 12 candidates
- runner-up: 101394 (TLE A1394) worse by delta chi-square 219.739 (scaled by the variance factor 1.000), likelihood ratio 10^47.7
- the two leading candidates' predictions differ by 16.2 sigma over these measurements; their positions are 9.032 to 10.038 km apart
- margin 219.739 >= 25 and probability 1.000000 >= 0.99: identified

## Observations

85 measurements, 3 passes, 2026-11-15T09:12:00.000Z to 2026-11-15T12:22:30.000Z (3.1750 h).

## Best against second best

| quantity | value |
|---|---|
| best | 101393 |
| second | 101394 |
| delta_chi2 | 219.7386 |
| log10_likelihood_ratio | 47.7156 |
| predicted_difference_sigma | 16.1782 |
| distance_km_min | 9.0319 |
| distance_km_median | 9.5300 |
| distance_km_max | 10.0375 |

## Candidates (ranked)

| rank | catalogue number | TLE field | source | reduced chi2 | delta chi2 | probability | below horizon | status |
|---|---|---|---|---|---|---|---|---|
| 1 | 101393 | A1393 | tle | 0.8585 | 0.0000 | 1.0000 | 0 | fitted |
| 2 | 101394 | A1394 | tle | 3.5382 | 219.7386 | 1.925e-48 | 0 | fitted |
| 3 | 101392 | A1392 | tle | 4.5639 | 303.8456 | 1.049e-66 | 0 | fitted |
| 4 | 101395 | A1395 | tle | 12.6054 | 963.2446 | 6.825e-210 | 0 | fitted |
| 5 | 101391 | A1391 | tle | 14.6336 | 1129.5619 | 5.233e-246 | 0 | fitted |
| 6 | 101396 | A1396 | tle | 28.0603 | 2230.5479 | 0.0000 | 0 | fitted |
| 7 | 101390 | A1390 | tle | 31.0875 | 2478.7790 | 0.0000 | 0 | fitted |
| 8 | 101397 | A1397 | tle | 49.9015 | 4021.5273 | 0.0000 | 0 | fitted |
| 9 | 101389 | A1389 | tle | 53.9091 | 4350.1494 | 0.0000 | 0 | fitted |
| 10 | 101398 | A1398 | tle | 78.0856 | 6332.6222 | 0.0000 | 0 | fitted |
| 11 | 101388 | A1388 | tle | 83.0890 | 6742.8980 | 0.0000 | 0 | fitted |
| 12 | 101399 | A1399 | tle | 112.6795 | 9169.3216 | 0.0000 | 0 | fitted |

Method: SGP4 (open engine 0.41.0) per candidate, TEME to ITRS, station geometry on WGS-84; residual = observed - predicted; per-pass constant biases by weighted least squares where configured; chi-square ranking; scaled delta chi-square and posterior probability under a uniform prior. Not modelled: light time, tropospheric and ionospheric refraction, transmitter frequency drift within a pass, element-set covariance.

No catalogue authority, launch provider or agency endorses this result.

Result SHA-256 (numbers rounded to 9 significant digits): `8589a93e61dbd99dd9a0c8bd4b57b4d59af4eee3564cc54b35e8dd1b70324879`
