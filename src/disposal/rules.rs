// PRIVATE — Arksa-exclusive (arksa-kpro). Never shipped in Kshana Pro or any public package.
//
//! The rule texts the decision records and the disposal assessment refer to.
//!
//! Every `Verified` entry is quoted word for word from the primary source named in it,
//! read on the `retrieved` date. The PDFs set powers of ten as superscripts; here they
//! are written `10^-4`. Nothing else is paraphrased inside `quote`. An entry whose
//! primary text was not read is `ToConfirm`: its `quote` is empty and its `note` says
//! what is known and from where, so no output can present it as rule text.
//!
//! The EU Space Act entries quote the Commission's **proposal** (COM(2025) 335 final, as
//! transmitted to the Council on 26 June 2025, Council document 10935/25 and its annexes
//! 10935/25 ADD 1). It is not adopted law; the legislative text may change, and its
//! numerical limits are left to implementing acts that do not exist yet.

use serde::Serialize;

/// Whether the quote was checked against the primary source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RuleStatus {
    /// Quoted word for word from the primary source on the `retrieved` date.
    Verified,
    /// Not checked against a primary source: no quote is given.
    ToConfirm,
}

/// One rule text.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RuleText {
    /// Stable identifier the checks refer to.
    pub id: &'static str,
    /// `FCC`, `ESA` or `EU`.
    pub framework: &'static str,
    /// The clause, as the source numbers it.
    pub citation: &'static str,
    /// The words of the source (empty when `ToConfirm`).
    pub quote: &'static str,
    pub source_title: &'static str,
    pub source_url: &'static str,
    /// The date the quote was read (UTC).
    pub retrieved: &'static str,
    pub status: RuleStatus,
    /// What a reader needs to know about the entry's standing.
    pub note: &'static str,
}

const ECFR_25_283: &str =
    "https://www.ecfr.gov/current/title-47/chapter-I/subchapter-B/part-25/subpart-D/section-25.283";
const ECFR_TITLE: &str =
    "Electronic Code of Federal Regulations, 47 CFR 25.283 End-of-life disposal (text in force on 17 November 2026; last amended 89 FR 65223, 9 August 2024)";
const ESSB_URL: &str = "https://technology.esa.int/upload/media/DGHKMZ_6542582e18e33.pdf";
const ESSB_TITLE: &str =
    "ESA Space Debris Mitigation Requirements, ESSB-ST-U-007 Issue 1, 30 October 2023";
const EU_URL: &str = "https://data.consilium.europa.eu/doc/document/ST-10935-2025-INIT/en/pdf";
const EU_ANNEX_URL: &str =
    "https://data.consilium.europa.eu/doc/document/ST-10935-2025-ADD-1/en/pdf";
const EU_TITLE: &str = "Proposal for a Regulation on the safety, resilience and sustainability of space activities in the Union (EU Space Act), COM(2025) 335 final, Council document 10935/25";
const EU_ANNEX_TITLE: &str =
    "Annexes to the EU Space Act proposal, COM(2025) 335 final, Council document 10935/25 ADD 1";
const RETRIEVED: &str = "2026-11-19";
const EU_NOTE: &str =
    "Commission proposal, not adopted law; the text may change in the legislative procedure.";

/// Every rule text, in a fixed order.
pub const RULES: &[RuleText] = &[
    RuleText {
        id: "FCC-25.283-e",
        framework: "FCC",
        citation: "47 CFR 25.283(e)",
        quote: "For space stations ending their mission in or passing through the low-Earth orbit region below 2000 km altitude and planning disposal through uncontrolled atmospheric re-entry, disposal must be completed as soon as practicable following end of mission, and no later than five years after the end of the mission.",
        source_title: ECFR_TITLE,
        source_url: ECFR_25_283,
        retrieved: RETRIEVED,
        status: RuleStatus::Verified,
        note: "The same paragraph defines end of mission as \"the time at which the individual spacecraft is no longer capable of conducting collision avoidance maneuvers\" and, for spacecraft without that capability, as the point at which it \"has completed its primary mission\". Which satellites the five-year rule applies to by launch date is set in the FCC's 2022 order, not in this paragraph (to confirm per application).",
    },
    RuleText {
        id: "FCC-25.283-c",
        framework: "FCC",
        citation: "47 CFR 25.283(c)",
        quote: "Upon completion of any relocation authorized by paragraph (b) of this section, or any relocation at end-of-life specified in an authorization, or upon a spacecraft otherwise completing its authorized mission, a space station licensee shall ensure, unless prevented by technical failures beyond its control, that stored energy sources on board the satellite are discharged, by venting excess propellant, discharging batteries, relieving pressure vessels, or other appropriate measures.",
        source_title: ECFR_TITLE,
        source_url: ECFR_25_283,
        retrieved: RETRIEVED,
        status: RuleStatus::Verified,
        note: "",
    },
    RuleText {
        id: "FCC-2026-RO",
        framework: "FCC",
        citation: "FCC satellite licensing Report and Order, July 2026 (new Part 100, including 100.201(c) on ephemeris data)",
        quote: "",
        source_title: "Not read in primary form; reported by secondary sources (Global Policy Watch, July 2026; Space Law Journal, 22 October 2026)",
        source_url: "https://www.globalpolicywatch.com/2026/07/fcc-approves-massive-modernization-of-satellite-licensing-regime/",
        retrieved: RETRIEVED,
        status: RuleStatus::ToConfirm,
        note: "To confirm against the order and the Federal Register: whether the five-year disposal certification moves from 47 CFR 25.283(e) into Part 100 and when, and the effective date of 100.201(c), reported as not yet set on 22 October 2026. Until then the checks cite 47 CFR 25.283 as published on eCFR.",
    },
    RuleText {
        id: "ESA-5.4.2.3-a",
        framework: "ESA",
        citation: "ESSB-ST-U-007 5.4.2.3 a (LEO protected region clearance)",
        quote: "The orbit clearance of a spacecraft or launch vehicle orbital element from the LEO protected region shall satisfy both following conditions: 1) the orbit lifetime is less than 5 years starting from either: (a) The orbit injection epoch, if it is injected into an orbit crossing the LEO protected region and has no recurrent manoeuvre capability [...] (b) The end-of-life epoch, if it operates in the LEO protected region and has a recurrent manoeuvre capability 2) the cumulative collision probability from its end of life until re-entry with space objects larger than 1 cm is below 10^-3",
        source_title: ESSB_TITLE,
        source_url: ESSB_URL,
        retrieved: RETRIEVED,
        status: RuleStatus::Verified,
        note: "The LEO protected region is defined in the same standard (3.2) as the shell from the surface of a spherical Earth of equatorial radius 6378 km up to an altitude of 2000 km. These are ESA's requirements for its own missions and for missions that adopt them by contract.",
    },
    RuleText {
        id: "ESA-5.4.2.3-c",
        framework: "ESA",
        citation: "ESSB-ST-U-007 5.4.2.3 c",
        quote: "The disposal orbit of spacecraft part of a large constellation operating in the LEO protected region shall have an apogee below 375 km.",
        source_title: ESSB_TITLE,
        source_url: ESSB_URL,
        retrieved: RETRIEVED,
        status: RuleStatus::Verified,
        note: "The standard (3.2.25) defines a large constellation as a group of at least 100 spacecraft, spares and planned enlargement included.",
    },
    RuleText {
        id: "ESA-5.4.1.1-a",
        framework: "ESA",
        citation: "ESSB-ST-U-007 5.4.1.1 a (probability of successful disposal)",
        quote: "The overall probability of successful disposal of a spacecraft or launch vehicle orbital stage in Earth orbit shall be kept above 0,9 through to end of life, including the contributions from system reliability and from collisions with space debris or meteoroids preventing the successful disposal.",
        source_title: ESSB_TITLE,
        source_url: ESSB_URL,
        retrieved: RETRIEVED,
        status: RuleStatus::Verified,
        note: "",
    },
    RuleText {
        id: "ESA-5.4.1.1-b",
        framework: "ESA",
        citation: "ESSB-ST-U-007 5.4.1.1 b",
        quote: "The contribution from system reliability to the probability of successful disposal of each spacecraft of a constellation in near Earth orbit shall be kept above 0,95 through to the end of life, if the spacecraft is: 1) Part of a large constellation, or 2) Operating in the LEO protected region in an orbit with a natural orbital decay duration longer than 25 years",
        source_title: ESSB_TITLE,
        source_url: ESSB_URL,
        retrieved: RETRIEVED,
        status: RuleStatus::Verified,
        note: "",
    },
    RuleText {
        id: "ESA-5.3.2.2-b",
        framework: "ESA",
        citation: "ESSB-ST-U-007 5.3.2.2 b (passivation)",
        quote: "A spacecraft or launch vehicle orbital stage operating in Earth orbit shall be passivated before the end of life unless a successful controlled re-entry is performed.",
        source_title: ESSB_TITLE,
        source_url: ESSB_URL,
        retrieved: RETRIEVED,
        status: RuleStatus::Verified,
        note: "",
    },
    RuleText {
        id: "ESA-5.3.2.2-c",
        framework: "ESA",
        citation: "ESSB-ST-U-007 5.3.2.2 c",
        quote: "A spacecraft or launch vehicle orbital stage operating in Earth orbit shall be designed to guarantee a probability of successful passivation through to the end of life of: 1) At least 0,90 2) At least 0,95, when operating in the LEO protected region in an orbit with a natural orbital decay duration longer than 25 years 3) At least 0,95, when operating in the GEO protected region",
        source_title: ESSB_TITLE,
        source_url: ESSB_URL,
        retrieved: RETRIEVED,
        status: RuleStatus::Verified,
        note: "",
    },
    RuleText {
        id: "ESA-5.5-b",
        framework: "ESA",
        citation: "ESSB-ST-U-007 5.5 b (re-entry)",
        quote: "The expected number of casualties per re-entry of a spacecraft or launch vehicle orbital stage, including elements thereof, shall be less than 10^-4 through adopting design and operations practices in the following order of preference: 1) Design for demise 2) Controlled re-entry 3) Approval of any other design or operations practice by the approving agent",
        source_title: ESSB_TITLE,
        source_url: ESSB_URL,
        retrieved: RETRIEVED,
        status: RuleStatus::Verified,
        note: "Clause 5.5 a also applies ESSB-ST-U-004 for re-entry, which was not read here.",
    },
    RuleText {
        id: "ESA-5.3.3.3-a",
        framework: "ESA",
        citation: "ESSB-ST-U-007 5.3.3.3 a (collision risk management during operations)",
        quote: "During normal operations of a spacecraft in near-Earth orbit with a recurrent manoeuvre capability, the acceptable collision probability threshold shall be below 10^-4 per conjunction.",
        source_title: ESSB_TITLE,
        source_url: ESSB_URL,
        retrieved: RETRIEVED,
        status: RuleStatus::Verified,
        note: "Its note: \"The 10^-4 collision probability threshold is considered absolute upper bound, and in practice lower threshold is often adopted\".",
    },
    RuleText {
        id: "ESA-5.3.3.3-b",
        framework: "ESA",
        citation: "ESSB-ST-U-007 5.3.3.3 b",
        quote: "During the normal operations of a spacecraft in the LEO protected region with a recurrent manoeuvre capability, on an orbit with an average density of space debris larger than 1 cm above 10^-7 km^-3, the acceptable collision probability threshold shall be the lower of the following values: 1) 10^-4, and 2) The collision probability value such to reduce the annual collision probability by at least 90% with respect to not performing collision avoidance manoeuvres",
        source_title: ESSB_TITLE,
        source_url: ESSB_URL,
        retrieved: RETRIEVED,
        status: RuleStatus::Verified,
        note: "The standard points to MIT-COL-MAN-00279-OPS-SD:2020 for the method, which accounts for debris that cannot be tracked; that method was not read here.",
    },
    RuleText {
        id: "EU-Art15-1",
        framework: "EU",
        citation: "EU Space Act proposal, Article 15(1), second subparagraph",
        quote: "In addition, third country spacecraft operators shall: (a) subscribe to a public or commercial collision avoidance space services provider; (b) ensure that the collision avoidance space services provider referred to in point (a), has the technical means to assess the collision avoidance and complies with the requirements laid down in point 1, of Annex IV; (c) notify to the Agency, in the application for registration in URSO, the name and details of the collision avoidance space services provider referred to in point (a).",
        source_title: EU_TITLE,
        source_url: EU_URL,
        retrieved: RETRIEVED,
        status: RuleStatus::Verified,
        note: EU_NOTE,
    },
    RuleText {
        id: "EU-Art64-5",
        framework: "EU",
        citation: "EU Space Act proposal, Article 64(5)",
        quote: "Upon receipt of a high interest event alert, Union spacecraft operators shall inform without delay the Union CA space services provider of all actions taken to avoid the collision, in accordance with point 2, of Annex IV.",
        source_title: EU_TITLE,
        source_url: EU_URL,
        retrieved: RETRIEVED,
        status: RuleStatus::Verified,
        note: EU_NOTE,
    },
    RuleText {
        id: "EU-AnnexIV-1.4-c",
        framework: "EU",
        citation: "EU Space Act proposal, Annex IV point 1.4(c)(iv) and (v) (CA space services provider)",
        quote: "(iv) providing users with a diverse, user-selectable set of conjunction and CA \"Go/No-Go\" manoeuvre metrics, to assess the collision risk and to develop an appropriate course of action; (v) checking that mitigation actions decrease the risk level of the conjunctions to be mitigated, and do not unduly increase the risk level of other conjunctions.",
        source_title: EU_ANNEX_TITLE,
        source_url: EU_ANNEX_URL,
        retrieved: RETRIEVED,
        status: RuleStatus::Verified,
        note: EU_NOTE,
    },
    RuleText {
        id: "EU-AnnexIV-2.7-a",
        framework: "EU",
        citation: "EU Space Act proposal, Annex IV point 2.7(a)",
        quote: "Union spacecraft operators and the Union CA space services provider referred to in Article 64(1) shall define at the time of spacecraft service registration: (a) as regards the elements related to the safety distance requirement, the limit above which the risk of collision is considered high enough to trigger a High Interest Event alert;",
        source_title: EU_ANNEX_TITLE,
        source_url: EU_ANNEX_URL,
        retrieved: RETRIEVED,
        status: RuleStatus::Verified,
        note: EU_NOTE,
    },
    RuleText {
        id: "EU-AnnexV-3.4",
        framework: "EU",
        citation: "EU Space Act proposal, Annex V points 3.4.1 and 3.4.2 (maximum orbital lifetime before re-entry for LEO)",
        quote: "3.4.1. The Union spacecraft operator of spacecraft in LEO shall disclose the expected time in orbit following: (a) the end of the space mission; (b) the completion of the passivation procedure. 3.4.2. For LEO, the orbital lifetime, after the end of the mission, and before re-entry into the atmosphere, shall be limited in accordance with the requirements set out in the implementing act referred to in Article 70(3), point (c).",
        source_title: EU_ANNEX_TITLE,
        source_url: EU_ANNEX_URL,
        retrieved: RETRIEVED,
        status: RuleStatus::Verified,
        note: "Commission proposal, not adopted law. The numerical lifetime limit is left to an implementing act that does not exist yet, so no pass or fail is given against it: the assessment discloses the expected time in orbit.",
    },
    RuleText {
        id: "EU-AnnexV-3.1",
        framework: "EU",
        citation: "EU Space Act proposal, Annex V points 3.1.1 and 3.1.2 (probability of successful disposal)",
        quote: "3.1.1. Union spacecraft operators shall calculate and adhere to assigned limits on the probability of successful disposal. 3.1.2. The probability of successful disposal shall be high and shall be calculated according to the requirements set out in the implementing act referred to in Article 70(3), point (c).",
        source_title: EU_ANNEX_TITLE,
        source_url: EU_ANNEX_URL,
        retrieved: RETRIEVED,
        status: RuleStatus::Verified,
        note: "Commission proposal, not adopted law. The threshold is left to an implementing act.",
    },
    RuleText {
        id: "EU-AnnexV-3.5.4",
        framework: "EU",
        citation: "EU Space Act proposal, Annex V point 3.5.4 (casualty risk at re-entry)",
        quote: "The probability of casualties per re-entry shall be further specified in the implementing act referred to in Article 70(3), point (c)(iii), considering the following requirements: (a) be as low as possible; (b) be expressed as a maximum probability of having at least one victim (collective risk); (c) include casualties on ground, as well as regards air traffic and maritime traffic;",
        source_title: EU_ANNEX_TITLE,
        source_url: EU_ANNEX_URL,
        retrieved: RETRIEVED,
        status: RuleStatus::Verified,
        note: "Commission proposal, not adopted law. The threshold is left to an implementing act.",
    },
];

/// The rule text with identifier `id`.
pub fn rule(id: &str) -> Option<&'static RuleText> {
    RULES.iter().find(|r| r.id == id)
}

/// The rule texts with these identifiers, in registry order, each once.
pub fn rules_for<'a>(ids: impl IntoIterator<Item = &'a str>) -> Vec<RuleText> {
    let ids: Vec<&str> = ids.into_iter().collect();
    RULES
        .iter()
        .filter(|r| ids.contains(&r.id))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_unique_and_quotes_follow_the_status() {
        for (i, r) in RULES.iter().enumerate() {
            assert!(
                RULES[i + 1..].iter().all(|x| x.id != r.id),
                "{} twice",
                r.id
            );
            match r.status {
                RuleStatus::Verified => {
                    assert!(!r.quote.is_empty(), "{} is verified but has no quote", r.id);
                    assert!(r.source_url.starts_with("https://"), "{}", r.id);
                }
                RuleStatus::ToConfirm => {
                    assert!(r.quote.is_empty(), "{} is not verified but quotes", r.id);
                    assert!(r.note.to_lowercase().contains("to confirm"), "{}", r.id);
                }
            }
            assert!(["FCC", "ESA", "EU"].contains(&r.framework));
        }
    }

    #[test]
    fn every_eu_entry_says_it_is_a_proposal() {
        for r in RULES.iter().filter(|r| r.framework == "EU") {
            assert!(r.citation.contains("proposal"), "{}", r.id);
            assert!(r.note.contains("not adopted law"), "{}", r.id);
        }
    }
}
