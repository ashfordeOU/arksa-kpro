# Simulator recalibration from telemetry (`arksa-kpro simcal`, AK8)

PRIVATE: Arksa-exclusive. Runs only on Arksa servers or in a contracted Sovereign install.

## Why

ESA buys a new end-to-end performance simulator for almost every mission. It bought four
in 2025–26, each above €500k. Two tender texts describe the problem: one calls the reference
simulation framework's development process "outdated/heavy/rigid", and another says simulators
"are not fully connected to flight telemetry", so "the lessons learned are not transferred to
the models for the next spacecraft" (internal research 11, finding 1). This track is a
mission-agnostic simulator core. It compares any mission's simulator with its telemetry,
recalibrates the models the hosted service can calibrate (`selfcal` for radio timing reference
simulators, `calibrate` for any simulator with named parameters), and keeps a signed record of
every calibration.

## What it does

```
arksa-kpro simcal plugins
arksa-kpro simcal residuals <mission.toml> [--out <dir>]
arksa-kpro simcal recalibrate <mission.toml> --lineage <dir> --service <https://...> \
    --trusted-key <service public key> [--verdict-license <token>] \
    [--sign-key-file <file>] [--signer <name>] [--utc <iso>]
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
| `hosted` | a model that runs in the hosted verdict service: `method = "selfcal"` (a radio timing reference scenario) or `method = "calibrate"` (named parameters, with a closed-form expression per observable or design runs of the mission's own simulator) | formed by the service |

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

Each `hosted` plugin names its method. The request goes to that method through Kshana Pro's
verdict client (`kshana_pro::remote_verdict::request`, licence token on stdin to curl), and
the answer must be a verdict signed for that same method.

#### `selfcal` (radio timing reference simulators)

For a `selfcal` plugin, the client sends `scenario_toml` (the plugin's scenario, verbatim),
`config` (its `[plugin.calibration]` table), `sites` (its `[[plugin.site]]` tables) and one
observation for each telemetry sample. An observation is the observable's `key` table plus
`value_ns` and `sigma_ns`.

#### `calibrate` (any simulator with named parameters)

`calibrate` is the hosted general parametric calibration: power, thermal, clock, attitude,
propulsion or a whole mission performance simulator, as long as its behaviour depends on named
parameters. A `calibrate` plugin declares:

```toml
[[plugin]]
id = "thermal"
kind = "hosted"
method = "calibrate"
[plugin.calibration]          # optional: subject (echoed by the service), max_iter (1-100)
max_iter = 50
[[plugin.parameter]]          # at most 32; prior, prior_sigma, lower, upper, initial optional
name = "t0_c"                 # (no prior_sigma: a flat prior)
prior = 20.0
prior_sigma = 5.0
[[plugin.parameter]]
name = "rate_c_per_s"
initial = 0.0
lower = -0.01
upper = 0.01
[[plugin.observable]]
name = "battery_temp_c"
tolerance = 0.5
expression = "t0_c + rate_c_per_s * t"   # arithmetic in t (s), pi and the parameters
```

Instead of an `expression` on every observable, the plugin can give a **design**: runs of the
mission's own simulator at chosen parameter sets, each with its predictions as a CSV
`t_s,observable,value` (the same format as a `table` plugin):

```toml
[[plugin.run]]
params = { capacity_wh = 100.0, eff = 0.95 }   # every parameter, nothing else
predictions = "power-run-4.csv"
```

The simulator plugin interface (`SimPlugin`) predicts at the simulator's own settings and
cannot re-run a simulator at new parameter values, so the design runs are declared and made
outside this crate; each run file is recorded among the plugin's sources by SHA-256. A plugin
gives expressions or runs, not both; `scenario`, `[[plugin.site]]` and observable `key` do not
apply.

The request: `subject` (`calibration.subject`, else `<mission>/<plugin>`), `parameters`, `model`
(`{"kind": "expression", "observables": {...}}` or `{"kind": "design", "runs": [{"params",
"predictions": {"<observable>": [[t_s, value], ...]}}]}`), `observations` (one
`{observable, t_s, value, sigma}` per telemetry sample of the plugin; the observable by its name
within the plugin), `tolerances` (each observable's `tolerance`, used by the service for the
fidelity score) and, when `max_iter` is set, `options`. Nothing else is sent.

#### Accepting an answer

An answer is accepted only if all of these hold:

1. it is a signed verdict for the plugin's method (`selfcal` or `calibrate`; a verdict for any
   other method is refused);
2. its Ed25519 signature verifies against a trusted service key;
3. its outcome has `converged`, `posterior` (`{name, value, std_err}`) and `lineage.head`;
4. for `calibrate`, its outcome also has `at_bound` and `not_constrained` (lists of names from
   the posterior), its `correlation`, when present, is one row and column per posterior
   parameter, and its posterior names exactly the parameters requested.

A `calibrate` outcome also records `chi2_reduced_first` and `chi2_reduced_last` (the reduced
chi-square before and after), `n_obs`, `correlation` and, for a design, `surface` (as the service
states it). `at_bound` (parameters that ended on a bound) and `not_constrained` (parameters whose
posterior is not narrower than 90 % of the prior) are kept in the lineage entry, named in the
plugin's note, printed by `simcal recalibrate` and listed in the entry's summary. A flagged
parameter is still recorded as calibrated: the flag says how far the telemetry supports it.

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
- The tests use synthetic toy missions (TOY-1; CALIB-1 for `calibrate`) and a local stand-in service that signs its
  answers with a test key. No flight mission's telemetry has been run through this, and no
  recalibration against the production service has been made.
- Two hosted calibration methods are called: `selfcal` serves one family (radio timing
  reference simulators, with scenarios in the Kshana Pro format and reference-site
  observations in ns); `calibrate` serves any simulator with named parameters, through a
  closed-form expression or declared design runs. A drifted `table` or `engine` model is
  reported and recorded, not recalibrated: to recalibrate it, declare it as a `calibrate`
  plugin (an expression of its behaviour, or runs of it at chosen parameter sets).
- The `calibrate` tests use a stand-in that fits a straight line (expression) or picks the
  closest design run; they check the request, the checks on the answer and the lineage, not
  the hosted method. The method's own checks (recovery of known parameters on synthetic
  simulators) are the service's, and are MODELLED.
- Calibrated parameters are what best explains the telemetry supplied under the model
  supplied; they are not a validation of the simulator.
- `selfcal` posterior parameter names are the service's own, and they are recorded as given;
  `calibrate` posterior names are the plugin's parameters. Neither is written back into a
  scenario or run file; the calibrated values and the service's lineage head are recorded.
