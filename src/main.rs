// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! `arksa-kpro`: the Arksa server binary. It registers the Arksa-exclusive kinds and
//! commands with Kshana Pro ([`arksa_kpro::register`]) and then runs Kshana Pro's own
//! command line, so `arksa-kpro serve ...` is the Kshana Pro service with the Arksa kinds
//! added, `arksa-kpro run <scenario>` runs open, Pro and Arksa kinds, and the Arksa
//! commands (`mbse`, `design`, `coexist`, `mission`, `navdt`, `spacestream`, `mrd`) are
//! available. Every Arksa kind and command needs a token carrying the `arksa` claim.

use std::process::ExitCode;

fn main() -> ExitCode {
    if let Err(e) = arksa_kpro::register() {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }
    kshana_pro::cli::main()
}
