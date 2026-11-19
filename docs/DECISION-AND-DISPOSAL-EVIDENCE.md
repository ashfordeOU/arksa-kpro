# Collision-avoidance decision records and disposal compliance

Arksa-exclusive (feature `pro-mission-dossier`, which needs a token with the `arksa` claim).
Every output is labelled **MODELLED**. Nothing here is a licence decision, a certification
or an acceptance by any authority, and nothing here claims customers or validation.

Abbreviations: CA collision avoidance; CDM conjunction data message (CCSDS 508.0); CFR Code of
Federal Regulations; ESA European Space Agency; EU European Union; FCC Federal Communications
Commission; LEO low Earth orbit; Pc probability of collision; TCA time of closest approach;
TOML Tom's Obvious Minimal Language; UTC Coordinated Universal Time.

## Commands

```bash
arksa-kpro mission decision scenarios/space-safety/decisions/ev-001.toml --out out/ev-001 [--sign-key-file key.hex] [--signer Arksa] --dev
arksa-kpro mission decision-chain out/ev-001 out/ev-002 --dev
arksa-kpro mission thresholds scenarios/space-safety/events.csv --thresholds 1e-4,1e-5,1e-6,3e-7 --period-days 180 [--out t.json] --dev
arksa-kpro mission disposal scenarios/space-safety/disposal-leo-smallsat.toml --out out/disposal [--sign-key-file key.hex] --dev
arksa-kpro dossier verify out/ev-001 --trust <signer public key>
```

A signing key is a file of 64 hex characters, or `ARKSA_STUDY_SIGNING_KEY`. The directory is
then signed as a whole with Kshana Pro's dossier signature (`signature.json`, Ed25519).

## Decision record (`src/decision/record.rs`)

One record per conjunction event: the event as the operator received it (Pc, miss distance,
data sources, CDM identifiers), the threshold policy in force, every option considered (kind,
delta-v, Pc after), the chosen option, the rationale, who decided and when, and the
coordination stated. The record states; it does not decide. Pc and miss distance are the
provider's figures as given, never recomputed. It finds:

* the policy band (above the manoeuvre threshold, monitoring band, below);
* whether the decision follows the policy, is more conservative, or **departs** from it (a
  departure is recorded with its rationale, never refused);
* whether a chosen manoeuvre's stated Pc after the manoeuvre is below the threshold and below
  the Pc before it (cf. EU Space Act proposal Annex IV 1.4(c)(v));
* review notes (one option only, no data source, no secondary effects stated, CA provider not
  informed of an action above threshold, cf. proposal Article 64(5)).

A decision taken at or after TCA is refused. `record_sha256` covers the canonical JSON of the
body; `prev_record_sha256` chains a record to the previous one, and `decision-chain` checks an
ordered list.

CDM parsing is deliberately not here: the conjunction validation work (AK1,
`ak/conjunction-validation`) owns the CCSDS CDM parser. TODO: fill `ConjunctionInput` from it
once it merges.

## Threshold comparator (`src/decision/policy.rs`)

Events CSV `event_id,tca_utc,pc[,post_pc]`. For each threshold, events with `pc >= t` are
manoeuvred (their Pc becomes `post_pc`, else `default_post_pc`, 0 by default); the sum of Pc
stands in for the expected number of collisions. Reported: manoeuvres (per year with
`--period-days`), risk accepted, reduction against no manoeuvre, and the highest threshold
tried that is at most 1e-4 and removes at least 90% of the listed risk, set beside ESA
ESSB-ST-U-007 5.3.3.3 b. That clause's method also counts untracked debris; this comparator
does not, so its reduction is an upper bound.

## Disposal compliance (`src/disposal/`)

Input: spacecraft (mass, drag area, drag coefficient, manoeuvrability, large constellation,
propulsion/batteries/pressure vessels), disposal strategy and orbit, optional operator lifetime
analysis, passivation sources and probability, probabilities of successful disposal and
cumulative collision, casualty expectation. Checks:

| Framework | Clause | What is checked |
|---|---|---|
| FCC | 47 CFR 25.283(e) | lifetime no more than 5 years after end of mission (uncontrolled re-entry below 2000 km) |
| FCC | 47 CFR 25.283(c) | a planned discharge for every declared stored-energy source |
| ESA | ESSB-ST-U-007 5.4.2.3 a | lifetime under 5 years; cumulative collision probability below 1e-3 |
| ESA | 5.4.2.3 c | large constellation: disposal apogee below 375 km |
| ESA | 5.4.1.1 a / b | probability of successful disposal above 0.9 / reliability contribution above 0.95 |
| ESA | 5.3.2.2 b / c | passivation planned; probability of successful passivation at least 0.90 (0.95) |
| ESA | 5.5 b | expected casualties per re-entry below 1e-4 |
| EU | proposal Annex V 3.4, 3.1, 3.5.4 | **disclosure only**: limits are left to implementing acts |

Verdicts: `COMPLIANT` (conservative bound meets the limit), `MARGINAL` (nominal meets it, bound
does not), `NOT COMPLIANT`, `NOT ASSESSED` (input missing), `NOT APPLICABLE`, `INFO`. Bases:
`MODELLED` (screening lifetime), `OPERATOR-SUPPLIED` (operator lifetime analysis), `DECLARED`
(operator probabilities, shown as given).

The lifetime model (`lifetime.rs`) integrates circular-orbit drag decay with the open engine's
Jacchia 1971 density at three constant activity cases (F10.7 70/140/220 sfu). The low case holds
solar minimum for the whole lifetime, so the bound is deliberately pessimistic. It is a
screening estimate, not a filing analysis. Orbits with apogee and perigee more than 50 km apart
are outside it. An operator lifetime takes precedence, and a disagreement with the model about
the five-year limit is flagged.

## Rule texts (`src/disposal/rules.rs`)

Every verified quote was compared word for word with its primary source on 19 Nov 2026:
eCFR 47 CFR 25.283 (text in force 17 Nov 2026), ESA ESSB-ST-U-007 Issue 1 (30 Oct 2023), and the
EU Space Act proposal COM(2025) 335 final (Council doc. 10935/25 and ADD 1). The EU text is a
**proposal, not adopted law**. The FCC July 2026 licensing order (Part 100, 100.201(c)) was not
read in primary form and is marked **to confirm**, with no quote. Tests enforce that a pass or
fail cites a verified rule and that EU rows are never passed or failed.

## Mission dossier

A programme may add:

```toml
[space_safety]
disposal = ["disposal-leo-smallsat.toml"]
decision = ["decisions/ev-001.toml", "decisions/ev-002.toml"]   # in time order
[space_safety.thresholds]
events = "events.csv"
thresholds = [1e-4, 1e-5, 3e-7]
period_days = 180.0
```

The dossier then has a "Space safety" section (JSON, HTML, PDF): the decision table with the
chain check, the threshold table, one compliance table per disposal input, and the rule texts.
Inputs and result documents go under `space-safety/`, are hashed into the request hash and the
body's file list, and are re-hashed by `mission check`. A programme without `[space_safety]`
gives the same dossier as before.

Fixtures in `scenarios/space-safety/` are synthetic and illustrative.
