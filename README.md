# arksa-kpro (private)

Arksa-exclusive Kshana Pro ("Arksa-KPro") and Core methods for Arksa, the operating system for space missions.
Owner decision, 14 November 2026: Kshana Pro parts that make Arksa strong are never public and never sold in a
standalone Kshana Pro licence.

## What lives here
- **Arksa-KPro:** every job that designs, proves, approves or operates a spacecraft (spaceborne interference
  exposure, PNT and time passport, covariance quality, threat-aware planning, anomaly attribution, lunar studies,
  mission dossier and ECSS evidence, space RF survey). Unlocked only by an Arksa-signed `arksa` entitlement.
- **Core:** the methods listed in the private trade-secret register. Never shipped.

## Rules
1. Code here runs only on Arksa servers, or in a contracted Arksa Sovereign install. It is never built into
   Kshana Desktop Pro, any public package or any customer-installed binary.
2. Results leave as signed verdicts with their evidence class; intermediate values, tuning and model internals
   do not.
3. Nothing here is disclosed (commit to a public repo, paper, talk, slide, website, issue, chat with outsiders)
   before the related patent application is filed or the method is recorded as a trade secret.
4. Access is limited to named people under confidentiality and IP assignment. Commits as
   ashfordeOU <236818772+ashfordeOU@users.noreply.github.com>; no AI attribution lines.
5. Marketing names outcomes only ("available in Arksa"), never methods, equations, parameters or diagrams.

Plan: kshana-pro docs/plans/ROADMAP-0.42.md §1a and §1b (X1–X8).

## The crate (roadmap 0.42 X3)

One Cargo workspace with one crate, `arksa-kpro` (library and the server binary `arksa-kpro`). It
depends on `kshana-pro` (private, a git dependency pinned to one commit) and on the open engine
`kshana` (the exact version Kshana Pro is built on). `kshana-pro` never depends on this crate; its CI
fails if any Arksa-exclusive term or this crate's name appears in its source
(`scripts/protect/source-guard.sh` there).

What moved here from kshana-pro (14 November 2026):

| Here | What |
|---|---|
| `src/packs/` | the eleven Arksa-exclusive scenario packs: `pro-mbse`, `pro-design-optimise`, `pro-spectrum-coexistence`, `pro-lunar-signal-security`, `pro-beacon-placement`, `pro-lunar-time-budget-study`, `pro-cislunar-observability-study`, `pro-optical-rf-hybrid-study`, `pro-realtime-frame-eop-study`, `pro-beacon-dop`, `pro-surface-beacon-dop-study` |
| `src/mbse/`, `src/design/`, `src/mission/`, `src/spectrum_coexistence/`, `src/lunar_oed.rs`, `src/mrd.rs`, `src/connectors/spacestream.rs` | the modules behind them (Kshana Pro keeps `criterion` and `units`, re-exported here) |
| `src/twin/navdt.rs`, `src/suite/gsn.rs`, `src/suite/vcd.rs` | the NAV-DT twin bundle and the GSN argument and VCD (Kshana Pro's report, trade study and study dossier no longer use them) |
| `src/commands.rs` | the commands `mbse`, `design`, `coexist`, `mission`, `navdt`, `spacestream`, `mrd` |
| `tests/` | their tests, plus `arksa_entitlement.rs` (refusal and acceptance through the hook) |
| `tests/fixtures/mbse/`, `scenarios/`, `papers/`, `docs/` | fixtures, worked scenarios, the committed study CSVs the pack tests compare against, and the docs (MBSE, mission dossier, design optimiser, spectrum coexistence, the D1.4 MRD mapping, the SpaceStream ICD) |

Module paths inside the crate are the ones the code had in Kshana Pro: `src/lib.rs` re-exports the
Kshana Pro modules it builds on (`registry`, `license`, `report_pdf`, `report_brand`, `brand_pdf`,
`pdf_text`, `criterion`, `units`, `study_ledger`, `benefits`, `report`, `util`), and `connectors`,
`suite`, `twin` and `packs` extend the Kshana Pro modules of the same name.

## How it plugs into Kshana Pro

`arksa_kpro::register()` hands the kinds, the command features, the commands and the Arksa issuer
keys to `kshana_pro::extension::register` and then runs Kshana Pro's start-up check of the signed
tier manifest. From then on the Kshana Pro dispatcher (`registry::run`, `run_gated`,
`pro_registry`, `kinds --json`), the service and the command line include them. Registration
unlocks nothing: every id here is listed in Kshana Pro's manifest at the `arksa` tier, by hash, so
each run goes through `kshana_pro::license::check`, which needs an unexpired token carrying the
`arksa` claim signed by an Arksa issuer key. Anything else is refused with the neutral message
"this capability is not available under this licence; it is provided only within Arksa".

The binary registers and then runs Kshana Pro's command line:

```bash
arksa-kpro serve --addr 127.0.0.1:8088 --license <arksa token> [--data-dir <dir>] ...  # the Pro service plus the Arksa kinds
arksa-kpro run <scenario.toml> --license <arksa token>                                # open, Pro and Arksa kinds
arksa-kpro mbse|design|coexist|mission|navdt|spacestream|mrd ...                      # the Arksa commands
```

Issuer key: `KSHANA_PRO_ARKSA_ISSUER_PUBKEY` at build time (64 hex characters, or several separated by
commas). Without it a debug build trusts the placeholder test key (`PLACEHOLDER_ARKSA_ISSUER_PUBKEY`,
private half only in `tests/arksa_entitlement.rs`) and a release build trusts none, so it refuses every
Arksa token. The tier manifest key is Kshana Pro's (its docs/LICENSING-KEYS.md, section 6).

## Building

```bash
cargo build                    # fetches kshana-pro at the pinned rev (needs read access; see CI)
cargo test
```

Against a local kshana-pro checkout instead of the pinned commit, create `.cargo/config.toml`
(gitignored) with:

```toml
[patch."https://github.com/ashfordeOU/kshana-pro"]
kshana-pro = { path = "../kshana-pro" }
```

To move to a newer kshana-pro: change `rev` in `Cargo.toml` to the new commit,
`cargo update -p kshana-pro`, run the tests, commit `Cargo.toml` and `Cargo.lock` together.
When kshana-pro adds, removes or re-tiers a kind, its signed tier manifest changes; the
`arksa_entitlement` tests here fail until both sides agree.

Disk: the open engine is large. Set `CARGO_TARGET_DIR` to a shared directory and
`CARGO_INCREMENTAL=0` on small machines.

## CI

`.github/workflows/ci.yml` runs format, clippy (`-D warnings`) and the full tests on the self-hosted
runner. Cargo fetches the private kshana-pro dependency with the git command line
(`CARGO_NET_GIT_FETCH_WITH_CLI=true`), authenticated by one secret:

* **`KSHANA_PRO_READ_TOKEN`** (owner step): a fine-grained personal access token, or a GitHub App
  installation token, with **Contents: read-only** on `ashfordeOU/kshana-pro` and no other repository
  or permission. Add it under this repository's Settings, Secrets and variables, Actions. The
  workflow writes it into the runner's git configuration only for
  `https://github.com/ashfordeOU/kshana-pro`, and removes it again at the end of the job. Without it
  the job stops at its first step with a message naming the secret.

No other secret is used, and none is in the repository.
