# The hosted verdict service for Arksa: where it runs, what arksa-kpro needs

PRIVATE: Arksa-exclusive. Owner decision of 20 November 2026 (roadmap 0.42 §1i, option B).

Two arksa-kpro commands call a method that runs only in the hosted verdict service (meru),
never in this crate:

| command | method | path |
|---|---|---|
| `arksa-kpro simcal recalibrate` (AK8) | `selfcal` | `POST /verdict/selfcal` |
| `arksa-kpro lunar-time assist request` (AK6) | `lunar-assist` | `POST /verdict/lunar-assist` |

The answer is a signed verdict; arksa-kpro accepts it only after its Ed25519 signature verifies
against a pinned service key (Kshana Pro's `remote_verdict::verify`). meru is never compiled into
Arksa or into this crate.

## Where it runs

| Arksa deployment | Verdict service | meru profile |
|---|---|---|
| Arksa's cloud | the same meru binary as an internal service next to Arksa, on the private network only, no public endpoint | `arksa-internal` |
| Sovereign install | a contracted meru instance alongside the install, provisioned with its own verdict key, service credential and revocation list | `sovereign-contracted` |
| (Kshana Pro customers, not Arksa) | the public meru | `public` |

In both Arksa cases Arksa authenticates with **one service credential**, not per-customer
licences: a vendor-signed token whose claims mark it as the `arksa` service client (meru README,
"Service credential"). meru keeps metering Arksa's calls, per tenant or mission, from the tag
arksa-kpro sends on every call. Setting up meru for each case: meru `deploy/README.md`, section 7.

## What arksa-kpro needs configured

Nothing of this is in the repository; the credential comes from Arksa's secret store.

| | flag | environment | also read |
|---|---|---|---|
| service address | `--service` | `ARKSA_VERDICT_URL` | `KSHANA_VERDICT_URL` (`simcal` only) |
| service credential | `--verdict-license` | `ARKSA_VERDICT_CREDENTIAL_FILE` (a file holding the token, as a secret store mounts it; preferred), `ARKSA_VERDICT_CREDENTIAL` | `KSHANA_VERDICT_LICENSE` (`simcal`), `--license` (`lunar-time assist request`, older form) |
| trusted verdict key(s) | `--trusted-key` | `ARKSA_VERDICT_KEY` (64 hex each, comma separated during a rotation) | `KSHANA_VERDICT_KEY`, then the key built into the release |
| tenant/mission tag | `--tenant` | `ARKSA_TENANT` | `simcal`: the mission file's `tenant`, else its `mission` id; `lunar-time assist request`: required |
| internal CA certificate | `--verdict-ca` | `ARKSA_VERDICT_CA` | the system trust store |

The first source that is set wins, in the order of the columns. The resolution is
`arksa_kpro::verdict_service::resolve`.

* **Address.** `https://` on the private network (an internal certificate authority, given by
  `ARKSA_VERDICT_CA`), or `http://127.0.0.1:<port>` when meru runs as a sidecar on the same host.
  Plain `http://` to any other address is refused, so the credential never crosses a network
  unencrypted.
* **Credential.** Sent as `Authorization: Bearer` to curl on stdin, never on a command line and
  never logged (the configuration's debug output redacts it). Rotation: the vendor issues the next
  token, Arksa's secret store is updated, then the old token is revoked in meru's list.
* **Key.** The public key of the verdict key of the instance called: Arksa's internal instance in
  the cloud, the contracted instance's own key for a Sovereign install.
* **Tenant tag.** 1 to 64 characters of `A-Z a-z 0-9 . _ -`, sent as `X-Meru-Tenant` on every call.
  meru records it in its meter log as given (`arksa-internal`) or as a hash (`sovereign-contracted`
  default; the contract decides). A malformed tag is refused before any call.

Mission file (`ks-simcal-mission/1`) option:

```toml
mission = "TOY-1"
tenant = "tenant-42.toy-1"   # optional; default: the mission id
```

Example, Arksa cloud (sidecar):

```
ARKSA_VERDICT_URL=http://127.0.0.1:8099
ARKSA_VERDICT_CREDENTIAL_FILE=/run/secrets/meru-service-credential
ARKSA_VERDICT_KEY=<the internal instance's verdict public key>
ARKSA_TENANT=<tenant>.<mission>
arksa-kpro simcal recalibrate mission.toml --lineage lineage/
```

## Batches

meru's internal profiles also serve `POST /verdict/batch` (several `calibrate`/`selfcal` requests in
one call, each answered with its own signed verdict). arksa-kpro does not use it yet: `simcal
recalibrate` sends one request per hosted plugin, which is one call for today's missions.

## Tests

`src/verdict_service.rs` (resolution order, the credential file, the CA file, tenant validation and
defaults, URL and key checks) and `tests/simcal.rs`
(`the_service_configuration_and_tenant_tag_reach_the_service`: configuration from the environment,
the credential and the tag received by a loopback stand-in service, from the library, the command
line and the lunar request path). Tests use generated keys and a stand-in; no call was made to a
deployed instance.
