# Operations assurance: test reports, anomaly-detector scorecards, tracking-gap impact

Arksa-exclusive (command `arksa-kpro ops`, under the existing `pro-mission-dossier` feature, which
needs a token with the `arksa` claim). Nothing here is a certification, an acceptance by any agency or
an ESA result, and nothing here claims customers or validation. Every output carries its evidence class.

Abbreviations: AD anomaly detection; DOI digital object identifier; ECSS European Cooperation for Space
Standardization; ESA European Space Agency; ESA-AD the ESA Anomalies Dataset; ESA-ADB the ESA Anomaly
Detection Benchmark; PUS Packet Utilisation Standard (ECSS-E-ST-70-41C); TC telecommand; TM telemetry;
TNR true-negative rate; UTC Coordinated Universal Time.

Why: ESA tender 1-12979 (GT1I-618GD) states "test reports are typically written manually"; 1-13313
states "siloed tools, disconnected data sources" (research 11, finding 6). ESA-ADB says benchmarks for
telemetry anomaly detection are lacking and operators need false-alarm control (research 06, G15). The
NASA OIG (IG-23-016) reports DSN demand up to 40% above supply, Perseverance affected at least seven
times in 21 months and a 33-hour Goldstone outage costing 151.5 hours of tracking (research 13,
finding 3). The scenarios in `scenarios/ops/` are synthetic and illustrative; they do not reproduce
any of those cases.

## Commands

```bash
arksa-kpro ops report scenarios/ops/test-report/procedure.toml scenarios/ops/test-report/run-nominal.csv --out out/report [--sign-key-file key.hex] [--signer Arksa] --dev
arksa-kpro ops ad-score scenarios/ops/ad-scorecard/spec-esa-ad.toml --out out/score --dev
arksa-kpro ops tracking-gap scenarios/ops/tracking-gap/study.toml --out out/gap --dev
arksa-kpro ops verify out/report --dev                       # recompute the body hash
arksa-kpro dossier verify out/report --trust <signer public key>   # check the signature
```

A signing key is a file of 64 hex characters, or `ARKSA_STUDY_SIGNING_KEY`; the output directory is
then signed as a whole with Kshana Pro's dossier signature (`signature.json`, Ed25519). The output
directory must be new or hold only the files the command writes, so a signature never covers stale
files. Each JSON product carries the SHA-256 of its canonical body (`report_sha256`,
`scorecard_sha256`, `impact_sha256`) and the SHA-256 of every input it read.

## 1. Test report from a run log (`src/ops/test_report.rs`)

**Procedure (TOML)**, in the structure of an ECSS-E-ST-70-32C procedure: `[procedure]` (id, title,
version, system under test, facility, requirements), `[[preconditions]]` (preconditions body),
`[[steps]]` (main body; each with `requirements`, `[[steps.telecommands]]` with the verification stages
to be reported, default acceptance and completion, and `[[steps.expect]]`), and `[[confirmation]]`
(confirmation body). A telemetry expectation is a range (`min`/`max`), a value with a tolerance
(`equals`, `tolerance`) or a state (`equals_text`), with a window `within_s` from the step's start
(from the last step's end for the confirmation body; without it, to the step's end) and a `mode`:
`reach` (some sample satisfies it; the time to it is reported) or `hold` (every sample satisfies it).
The file is not PLUTO source; no PLUTO parser is provided.

**Run log (CSV)**: `time_utc,event,step,name,value,detail`, events `run_start` (name = run id,
value = log source), `run_end`, `step_start`, `step_end`, `tc`, `tc_ack` (value = PUS verification
stage `acceptance|start|progress|completion`, detail = `success|failure`), `tm` (name = parameter),
`abort`, `operator`, `note`. Times are `YYYY-MM-DDTHH:MM:SS[.fff]Z`.

**Report**: per step the ECSS-E-ST-70-32C confirmation status (**confirmed**, **not confirmed**,
**aborted**, or **not executed**), each telecommand's stages and status (verified, failed, not
verified, not sent), each expectation beside the observed value and time; preconditions (latest sample
before the first step), confirmation body (not evaluated after an abort); deviations (telemetry out of
expectation, no telemetry, failed or missing verification stages, unplanned telecommands, steps out of
order, repeated or unknown steps, aborts); and the requirement verification matrix (a requirement is
"verified by test (as logged)" only when every step tracing to it is confirmed; procedure-level ones
need the whole procedure confirmed). Outputs `report.json`, `report.md` (identification and
configuration, summary, preconditions, steps, confirmation, deviations, requirement matrix,
conclusion) and copies of both inputs.

**Evidence class** from the log source: `synthetic`, `simulator`, `software-validation-facility`
give MODELLED; `engineering-model`, `qualification-model`, `flight-model`, `in-orbit` give MEASURED
(as logged). The report states what the log records; the log's authenticity and the test configuration
are the facility's responsibility.

## 2. Anomaly-detector scorecard (`src/ops/ad_scorecard.rs`)

A spec TOML names the benchmark (`format`, `name`, `data_origin` = `public-benchmark`, `operator` or
`synthetic`, `source_doi`, `licence`, the files) and the detector (`name`, `version`, `outputs`, an
optional pre-registered `bars` file and its `bars_sha256`). With `bars_sha256` the scorecard refuses to
score if the bars file changed; without it, it says it cannot show the bars were fixed first. Evidence
class: MEASURED for real data (`public-benchmark`, `operator`), MODELLED for `synthetic`.

### Interval benchmarks: `format = "esa-ad"`

Reads the ESA Anomalies Dataset's own `labels.csv` (`ID,Channel,StartTime,EndTime`) and
`anomaly_types.csv` (`ID,Class,Subclass,Category,Dimensionality,Locality,Length`), plus an evaluation
window and optional channel scope. Detector alarms: `alarm_id,channel,start_utc,end_utc` (rows with
the same `alarm_id` form one multi-channel alarm). Definitions used here (reimplemented from the
ESA-ADB paper's text, not its TimeEval code, so figures can differ from the paper's):

* **Events** are annotation IDs with category `Anomaly` (and `Rare Event` with `rare_events = "count"`),
  restricted to the channels in scope and clipped to the window; an event's extent runs from its
  earliest to its latest channel interval. `Communication Gap` events, and rare events by default, are
  ignored: alarms on them are neither true nor false.
* An alarm **detects** an event when their extents overlap. A **false alarm** overlaps no annotated event.
* `event_recall` = detected / events. `event_precision` = detected / (detected + false alarms).
  `time_tnr` = share of nominal time (window minus every annotated event) not under an alarm.
  `event_precision_corrected` = `event_precision` × `time_tnr` (penalises long alarms).
  `event_f0_5` = F-beta (beta 0.5) of the corrected precision and the recall.
* `channel_precision` / `channel_recall`: per detected event, the share of channels flagged by the
  alarms on it that belong to it (a channel annotated for another event those alarms overlap is not
  counted against it) and the share of its channels flagged; averaged over detected events;
  `channel_f0_5` from the two averages.
* `alarming_precision` = detected events / alarms on events (capped at 1): redundant alarms lower it.
* `false_alarms_per_day` = false alarms / nominal days: the operator false-alarm budget.
* `latency_median_s`, `latency_p90_s`: first alarm start minus event start, 0 when the alarm starts
  first (nearest-rank percentiles). Also reported: recall by `Category`, `Dimensionality`,
  `Locality`, `Length`, and the missed event IDs.

### Segment benchmarks: `format = "opssat-ad"`

Reads OPSSAT-AD's `dataset.csv` (`segment,anomaly,train,...`); predictions `segment,predicted[,score]`
for every evaluated segment (the test split, `train = 0`, by default; `split = "all"` for every row).
Metrics: precision, recall, F1, F0.5, accuracy, balanced accuracy, Matthews correlation, and with a
score for every segment ROC AUC (Mann-Whitney, ties one half) and average precision; also
`false_alarms_per_100_nominal_segments`.

### Bars

```toml
registered_utc = "2025-12-20T00:00:00Z"
[[bar]]
metric = "false_alarms_per_day"
max = 0.5
why = "operator false-alarm budget"
```

Each bar is PASS, FAIL (an undefined metric fails), or NOT APPLICABLE (the benchmark does not produce
that metric); the verdict is FAIL if any bar fails, PASS if at least one passes and none fails.

### Public data: fetched on demand, never committed

| Dataset | Source | Licence | What is read |
|---|---|---|---|
| ESA Anomalies Dataset v1.0 (ESA-ADB) | doi:10.5281/zenodo.12528696 (Zenodo record states CC BY 3.0 IGO); benchmark code github.com/kplabs-pl/ESA-ADB (MIT) | CC BY 3.0 IGO: reuse with attribution | `labels.csv`, `anomaly_types.csv`, `channels.csv` of ESA-Mission1, by HTTP range reads into the 3.7 GB archive; no telemetry |
| OPSSAT-AD | doi:10.5281/zenodo.15108715 | Zenodo record: CC BY 4.0; the LICENSE file in the record is an MIT licence. Both allow reuse with attribution | `dataset.csv` (0.5 MB) |

`python3 -I scripts/fetch_ad_benchmarks.py <dir>` fetches them and checks their SHA-256 against the
files read on 19 November 2026. The two opt-in tests then run with
`ARKSA_ESA_AD_DIR=<dir>/esa-ad/ESA-Mission1 ARKSA_OPSSAT_AD_CSV=<dir>/opssat-ad/dataset.csv cargo test --test ops_assurance -- --ignored`.
On those files: ESA-Mission1 has 200 annotated events (118 anomalies, 78 rare events, 4 communication
gaps); an oracle raising exactly the anomaly intervals scores 1 on recall, corrected precision and
channel metrics, and one alarm over the whole span scores recall 1 and corrected precision 0. OPSSAT-AD
has 2,123 segments, 529 in the test split (113 anomalous). These are checks of the scorer, not results
of any detector. Cite Kotowski et al., arXiv:2406.17826 (ESA-ADB) and Ruszczak et al., Scientific Data
12 (2025), doi:10.1038/s41597-025-05035-3 (OPSSAT-AD) when publishing a scorecard.

## 3. Tracking-gap impact (`src/ops/tracking_gap.rs`)

A study TOML: `[study]` (name, period, `schedule`, `outages`, optional `alternates`) and
`[[missions]]` (onboard `data_generation_kbps`, `recorder_capacity_gbit`, `recorder_initial_gbit`,
optional `max_gap_h`, `min_ranging_passes_per_day`, and `[[missions.critical]]` windows with a required
coverage, default 1). Schedule and alternates CSV:
`pass_id,mission,station,start_utc,end_utc,downlink_kbps,services` (services `TM;TC;RNG`); outages
CSV: `station,start_utc,end_utc,reason`. Outages cut every pass at their station, alternates included.

Per mission, three cases (as scheduled, with outages, with outages plus alternates): contact hours,
contacts and contacts with TC, downlinked data, recorder overflow (science data lost), data still on
board at the end, the longest gap without contact and every gap over the limit (gaps from the period's
start and to its end count), whole UTC days below the ranging minimum, and critical-window coverage.
The recorder model: data generated at a constant rate, downlinked at the highest rate of the contacts
in progress, overflow once the recorder is full. Reported impact: passes hit (lost or shortened, with
the outage reasons), contact lost, science lost (overflow difference), backlog added, new gap
violations, critical windows lost, and the share of the loss the alternates recover. Per station:
outage hours, passes and missions hit. Always MODELLED: the schedule and outages are given, and no
visibility, link budget or network scheduling is computed here.

## Not built (and why)

* No PLUTO (ECSS-E-ST-70-32C language) parser and no procedure execution: the report reads a
  structured procedure and an executor's log.
* No ESA-ADB anomaly detection timing quality curve (ADTQC) and no affiliation-based metrics; the
  metrics above are the ones whose definitions could be written down exactly here.
* No telemetry download or detector training: the scorecard scores outputs against annotations.
* No visibility or scheduling optimisation in the tracking-gap study (inputs are schedules and windows).
* Not yet a mission dossier section; the products are signed directories that can be attached as
  evidence. A `[ops]` dossier section is a follow-up.
