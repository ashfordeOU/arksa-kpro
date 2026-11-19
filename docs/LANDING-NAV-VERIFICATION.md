# Landing-navigation fault-handling verification

Arksa-exclusive (feature `pro-mission-dossier`, which needs a token with the `arksa` claim).
Every output is labelled **MODELLED**. Nothing here is flight qualification, a certification or
an acceptance by NASA, ESA or any authority, and nothing here claims customers or validation.

Abbreviations: CLPS Commercial Lunar Payload Services; FDIR fault detection, isolation and
recovery; HIL hardware in the loop; LRF laser rangefinder; SIL software in the loop.

## Why

Research note 13 (finding 1, gap G-A): four of the last five commercial lunar landing attempts
had trouble traced to altitude sensing or its handling. ispace Mission 1 (April 2023): the
onboard computer incorrectly presumed the radar altimeter was faulty. IM-1 (February 2024): the
laser rangefinders' safety switch was never enabled. IM-2 (March 2025): the altimeter failed
during landing. ispace Mission 2 (June 2025): ispace attributes the loss to a laser range
finder anomaly. Each provider tests its own FDIR; what is missing is a neutral, pre-registered
campaign with evidence a reviewer can re-run. Sources: `arksa-kpro landing-nav library`.

## Scope: verification only

The harness never estimates a lander's state. It has **no terrain-relative or map-matching
navigation, no guidance and no sensor fusion** (export control; BUILD_RULES). It corrupts a
known, synthetic truth, hands the corrupted sensor streams to the provider, and compares the
provider's logged filter outputs with the truth. Terrain appears only as a scalar height under
the sensor over time (for example a crater), which changes what a healthy altimeter reads.
The descent is one-dimensional (vertical).

## Workflow

```bash
arksa-kpro landing-nav library [--json] --dev
arksa-kpro landing-nav scenarios scenarios/landing-nav/campaign.toml --out prereg/ [--sign-key-file key.hex] --dev
#   ... the provider runs its filter (SIL, HIL or replay) on prereg/streams/<id>.csv
#   and logs its outputs to logs/<id>.csv ...
arksa-kpro landing-nav verify scenarios/landing-nav/campaign.toml --logs logs/ --out verdict/ \
    --prereg prereg/preregistration.json [--sign-key-file key.hex] --dev
arksa-kpro dossier verify verdict/ --trust <signer public key>
```

`scenarios` writes `campaign.toml`, `preregistration.json` (campaign SHA-256, pass bars, stream
and truth hashes), `streams/` and `truth/`; the operator may withhold `truth/`. `verify` refuses
a campaign whose hash differs from the pre-registration (bars or scenarios changed after
registration), regenerates the truth (deterministic, seeded), checks every log and writes
`verdict.json`, `verdict.md` and copies of the inputs. A signing key (64 hex characters, or
`ARKSA_STUDY_SIGNING_KEY`) signs either directory as a whole (Kshana Pro dossier signature,
Ed25519). `verify` exits 0 only when every scenario passes. Without `--prereg` the verdict
records that the bars were not shown to be fixed in advance.

## Campaign file (`arksa-landing-nav-campaign/1`)

See `scenarios/landing-nav/campaign.toml` (synthetic and illustrative) and
`src/landing/campaign.rs`.

* `[system_under_test]`: name, version, `test_setup` (`SIL`, `HIL` or `replay`, declared by the
  provider and not checked), optional provider.
* `[descent]`: constant-deceleration braking from `start_alt_m`/`start_vz_mps` to
  `terminal_alt_m`/`terminal_vz_mps`, then constant-rate descent to touchdown; `step_s`.
* `[[sensor]]`: `id`, `kind` (`radar_altimeter` or `laser_rangefinder`), noise floor and
  range-proportional noise, range limits (no return outside them), `rate_hz`.
* `[bars]`: the pre-registered pass bars (below).
* `[[scenario]]`: `id`, `title`, optional `incident`, `[[scenario.terrain]]` steps and
  `[[scenario.fault]]` faults (`sensor`, `type`, `t_start_s`, `t_end_s`, `value`,
  `probability`, and optional `expect` = `detect`|`either` and `recover` overrides).

## Fault library

| Type | What it is | Default |
|---|---|---|
| `dropout` | no measurement over the window | detect, recover |
| `never_enabled` | the sensor never reports (not armed) | detect |
| `bias` | constant offset `value` m, reported valid | detect |
| `drift` | offset growing at `value` m/s, reported valid | detect |
| `stuck` | reading freezes, still flagged valid | detect |
| `saturation` | readings clipped at a ceiling `value` m, flagged valid | detect |
| `spikes` | outliers of +/- `value` m with `probability` per sample | either, recover |
| `noise_burst` | extra noise `value` m (dust or plume returns) | either, recover |

Terrain steps are not faults: the sensor stays truthful and must not be rejected.

## Filter-log interface

One CSV per scenario, `<scenario id>.csv`; `#` lines are comments; other columns are ignored and
listed in the verdict.

| Column | Meaning |
|---|---|
| `t_s` | seconds from the start of the stream, strictly increasing |
| `alt_est_m`, `alt_sigma_m` | altitude above the landing-site datum and its stated 1 sigma (> 0) |
| `vz_est_mps` | vertical speed, negative downwards |
| `vz_sigma_mps` | optional; enables the vertical-speed consistency check |
| `<id>_health` | per sensor: `ok`, `fault` or `off` |
| `<id>_used` | per sensor: `1` if its latest measurement is used, else `0` |

A sensor is *flagged* at an epoch when its health is not `ok` or it is not used.

## Checks and verdicts

| Check | Pass when |
|---|---|
| coverage | the log spans start to touchdown with no gap over `max_gap_s` (else INCOMPLETE) |
| detection | a `detect` fault is flagged within `detect_latency_s` of onset; for bias, drift, stuck and saturation the onset is when the error first reaches `min_detectable_error_m` while the sensor is in range |
| faulty_reaccepted | after detection, the faulty sensor is used again for at most `detect_latency_s` while its error is still detectable |
| recovery | after a fault that must recover, the sensor is used again within `recovery_s` |
| false_rejection | a truthful sensor (in range now and `recovery_s` ago, no fault, past any recovery window) is flagged for at most `false_reject_max_s` in total |
| configuration | a sensor that should report is not `off` for the whole descent (skipped where `never_enabled` is injected) |
| consistency | at least `consistency_min_fraction` of epochs have abs(altitude error) <= `sigma_k` x stated sigma |
| low_alt_error, low_alt_vz_error | below `low_alt_m`, the largest errors are within the bars |

A scenario FAILS on any failed check, is INCOMPLETE when its log is missing, malformed or does
not cover the descent, and PASSES otherwise. The campaign verdict is FAIL if any scenario
fails, else INCOMPLETE if any is incomplete, else PASS. `verdict_sha256` is the SHA-256 of the
canonical JSON of the body.

## Limits

* MODELLED: synthetic one-dimensional descent and generic sensor models, not validated against
  any real sensor or flight data. The incident summaries are of public reporting, not of
  accident investigation reports.
* The verdict covers the filter's behaviour as logged; whether the logs come from the flight
  software is the provider's declaration (`test_setup`).
* Not built here (follow-ups): a dust-plume degradation model validated against data (N3),
  landing-site acceptance coupling dispersion with slope (N5), post-landing reconstruction
  (N6), a mission-dossier section for the verdict.
