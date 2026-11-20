# Simulator recalibration from telemetry (`arksa-kpro simcal`, AK8)

PRIVATE: Arksa-exclusive. Runs only on Arksa servers or in a contracted Sovereign install.

## Why

ESA buys a new end-to-end performance simulator for almost every mission. It bought four
in 2025–26, each above €500k. Two tender texts describe the problem: one calls the reference
simulation framework's development process "outdated/heavy/rigid", and another says simulators
"are not fully connected to flight telemetry", so "the lessons learned are not transferred to
the models for the next spacecraft" (internal research 11, finding 1). This track is a
mission-agnostic simulator core. It compares any mission's simulator with its telemetry,
recalibrates the models the hosted service can calibrate, and keeps a signed record of every
calibration.

## What it does

```
arksa-kpro simcal plugins
arksa-kpro simcal residuals <mission.toml> [--out <dir>]
arksa-kpro simcal recalibrate <mission.toml> --lineage <dir> [--service <https://...>] \
    [--trusted-key <service public key>] [--verdict-license <token>] [--tenant <tag>] \
    [--verdict-ca <file>] [--sign-key-file <file>] [--signer <name>] [--utc <iso>]
arksa-kpro simcal verify <lineage-dir> [--trusted-key <hex>] [--trust <signer public key>]
```

Gate: the mission feature (`pro-mission-dossier`) behind the Arksa entitlement, which is the
same gate as `arksa-kpro mission`. No new pack kind or feature id is needed, so the Kshana Pro
tier manifest is unchanged.

### Mission file (`ks-simcal-mission/1`)

A mission names one telemetry file and its simulator **plugins**. The full syntax is in
`src/simcal/mission.rs`; a worked toy mission is in `tests/fixtures/simcal/toy-1/`.

| plugin kind | simulator | predictions |
|---|---|---|
| `table` | any external simulator: its output as a CSV `t_s,observable,value` | linear interpolation inside the simulated span |
| `engine` | a scenario of any open kind (or a Pro kind the licence carries), run on the embedded engine | a series (`series`, `x`, `y`) or a number (`path`) in the result |
| `hosted` | a model that runs in the hosted verdict service (`method = "selfcal"`) | formed by the service |

To add a model family, implement `simcal::plugin::SimPlugin` and add one line to
`simcal::plugin::KINDS`.

Telemetry is a CSV with the columns `t_s,observable,value,sigma`. Each observable is named
`<plugin>.<name>`. `sigma` is the standard uncertainty of a sample; it is required for a hosted
observable.

### Residuals

For each observable, the residual is telemetry minus the simulated value, computed for every
sample inside the simulated span. The report gives:

- the bias, with its type A standard uncertainty `s/√n`;
- the RMS and the largest |r|;
- how many samples fall outside ±tolerance;
- the reduced chi-square, when every sample has a sigma;
- a verdict: `drifted` if |bias| or RMS is above the tolerance, otherwise `consistent`. It is
  `no-comparison` when no sample falls inside the span, and `hosted` when the hosted service
  forms the residuals.

`--out` writes `residuals.json` (`ks-simcal-residuals/1`) and `residuals.csv`.

### Recalibration (network call, X3)

For each `hosted` plugin, the client sends `scenario_toml` (the plugin's scenario, verbatim),
`config` (its `[plugin.calibration]` table), `sites` (its `[[plugin.site]]` tables) and one
observation for each telemetry sample. An observation is the observable's `key` table plus
`value_ns` and `sigma_ns`. The request goes to the hosted self-calibration verdict through
Kshana Pro's verdict client (`kshana_pro::remote_verdict::request`, method `selfcal`, licence
token on stdin to curl). The answer is accepted only if all of these hold:

1. it is a signed verdict for `selfcal`;
2. its Ed25519 signature verifies against a trusted service key;
3. its outcome has `converged`, `posterior` (`{name, value, std_err}`) and `lineage.head`.

Where the service runs (Arksa's internal instance in the cloud, a contracted instance beside a
Sovereign install), the service credential, the trusted key and the tenant/mission tag sent on
every call are configured as described in `docs/VERDICT-SERVICE.md` (environment
`ARKSA_VERDICT_URL`, `ARKSA_VERDICT_CREDENTIAL_FILE`, `ARKSA_VERDICT_KEY`, `ARKSA_TENANT`; the
mission file's optional `tenant`, else its `mission` id).

The calibration method is not in this crate. Nothing here links the service or the gated method
code; this crate holds only the request contract and the checks on the answer.

Plugin status in the lineage:

| status | meaning |
|---|---|
| `calibrated` | hosted, and the service converged; the calibrated parameters (MODELLED) are recorded |
| `not-converged` | hosted, but the service did not converge; nothing is taken as calibrated |
| `consistent` | a local model within tolerance |
| `recalibration-needed` | a local model that drifted, where no hosted calibration method serves its family; the residuals are recorded as an open item |
| `no-comparison` | no telemetry sample fell inside the simulated span |

If any hosted verdict is refused or the service cannot be reached, nothing is written.

### Calibration lineage

A lineage directory contains:

- `entries.jsonl`: one `ks-simcal-lineage/1` entry per recalibration. Each entry records the
  mission, telemetry and plugin files by SHA-256, every plugin's residuals and status, and, for
  a hosted plugin, the request hash, the verdict hash, service, key and evidence class, and the
  outcome. It also holds the SHA-256 of the previous entry and its own hash.
- `requests/` and `verdicts/`: the exact request sent and the signed verdict received.
- `signature.json`: Kshana Pro's signed-dossier signature over the whole directory, made with
  the Arksa signer key and renewed on every append.

`simcal verify` re-checks the chain, every entry hash, every request and verdict file, every
verdict's signature and outcome against its entry, and the directory signature. It detects an
edited, removed, reordered or added entry or file. A lineage that does not pass these checks is
not extended.

## Evidence and limits

- Simulator predictions and calibrated parameters are **MODELLED**. Residuals are measured
  against the telemetry supplied and nothing more.
- The tests use a synthetic toy mission (TOY-1) and a local stand-in service that signs its
  answers with a test key. No flight mission's telemetry has been run through this, and no
  recalibration against the production service has been made.
- Today the only hosted calibration method is `selfcal`, and it serves one family: radio
  timing reference simulators, with scenarios in the Kshana Pro format and reference-site
  observations in ns. A drifted `table` or `engine` model (power, thermal, clock, and so on)
  is reported and recorded, not recalibrated, until the hosted service offers a method for
  that family. This is the main follow-up: a hosted method for generic parametric models,
  built in the service and called with the same client, with no change to this crate's
  architecture.
- The posterior parameter names are the service's own, and they are recorded as given. They
  are not written back into the scenario; the calibrated simulator state lives in the service,
  and its lineage head is recorded.
