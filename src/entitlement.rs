// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! Entitlement gates of the Arksa-exclusive commands. Each one is Kshana Pro's
//! [`kshana_pro::license::check`]: an Arksa-exclusive id needs a verified, unexpired token
//! carrying the `arksa` claim, and the refusal names no kind and no method.

use kshana::api::KshanaError;
use kshana_pro::license::{check, Entitlement};

/// Gate the MBSE and programme tooling (`arksa-kpro mbse ...`). It is the same feature id
/// as the `pro-mbse` scenario pack, so one entitlement covers both.
pub fn ensure_mbse_entitled(ent: &Entitlement, now_unix: i64) -> Result<(), KshanaError> {
    check(ent, crate::packs::mbse_pack::MbseScenario::KIND, now_unix)
}

/// Gate the spaceborne receiver interference exposure study, its per-orbit forecast and the
/// export of matching test scenarios (`arksa-kpro rx-exposure`). It is the same id as the
/// `pro-spaceborne-rx-exposure` kind, so one entitlement covers the kind and the command.
pub fn ensure_rx_exposure_entitled(ent: &Entitlement, now_unix: i64) -> Result<(), KshanaError> {
    check(
        ent,
        crate::packs::spaceborne_rx_exposure_pack::KIND,
        now_unix,
    )
}

/// Gate rideshare object identification (`arksa-kpro rideshare-id`). It runs under the
/// mission feature (`pro-mission-dossier`, Arksa tier) like the other operations commands, so
/// it needs a token with the `arksa` claim and works with the signed tier manifest as it
/// stands; the scenario kind `pro-rideshare-id` waits for the manifest
/// ([`crate::PENDING_MANIFEST_KINDS`]).
pub fn ensure_rideshare_id_entitled(ent: &Entitlement, now_unix: i64) -> Result<(), KshanaError> {
    crate::rideshare::ensure_entitled(ent, now_unix)
}
