// The party deck. Each scenario is a hidden party — split into the `occasion`
// (the event, e.g. "Surprise 30th Birthday") and the `location` (the venue,
// e.g. "Downtown Rooftop") — plus `aliases` used to judge the uninvited
// player's guess, and a pool of `roles` (relations to the party) handed out to
// the invited players. The deck is bundled into the binary so there is nothing
// to download.

use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
pub struct Scenario {
    pub id: String,
    pub occasion: String,
    pub location: String,
    pub aliases: Vec<String>,
    pub roles: Vec<String>,
}

/// The public face of a party (no aliases/roles): what the board shows for each
/// possibility. The active party is one of these, but the board never says which.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Party {
    pub occasion: String,
    pub location: String,
}

const DECK_JSON: &str = include_str!("../scenarios.json");

fn deck() -> Vec<Scenario> {
    serde_json::from_str(DECK_JSON).expect("scenarios.json is malformed")
}

/// Every party in the deck, sorted for a stable board. This is the public
/// list of possibilities shown to all players (Spyfall-style): the active
/// party is one of these, but the list never says which.
pub fn all_parties() -> Vec<Party> {
    let mut parties: Vec<Party> = deck()
        .into_iter()
        .map(|s| Party {
            occasion: s.occasion,
            location: s.location,
        })
        .collect();
    // Sort by venue then occasion so the board order is deterministic.
    parties.sort_by(|a, b| (&a.location, &a.occasion).cmp(&(&b.location, &b.occasion)));
    parties
}

/// Draw a random scenario from the deck.
pub fn draw() -> Scenario {
    let mut d = deck();
    let idx = (0..d.len())
        .collect::<Vec<_>>()
        .choose(&mut rand::thread_rng())
        .copied()
        .unwrap_or(0);
    d.swap_remove(idx)
}

impl Scenario {
    /// Lenient check of an uninvited player's party guess. True when the
    /// normalized guess overlaps the occasion or any alias (either contains the
    /// other), so players don't have to match the exact phrasing. The bare venue
    /// (`location`) is deliberately NOT a candidate: it's generic and shared
    /// across parties (two happen in "a backyard"), so matching it would let a
    /// wrong pick win. The occasion + aliases carry the distinctive identity.
    pub fn guess_matches(&self, guess: &str) -> bool {
        let g = normalize(guess);
        if g.len() < 3 {
            return false;
        }
        std::iter::once(&self.occasion)
            .chain(self.aliases.iter())
            .any(|cand| {
                let c = normalize(cand);
                !c.is_empty() && (g.contains(&c) || c.contains(&g))
            })
    }
}

fn normalize(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c.is_whitespace() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .filter(|w| !matches!(*w, "a" | "an" | "the" | "at" | "in" | "on" | "of" | "party"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scenario() -> Scenario {
        Scenario {
            id: "x".into(),
            occasion: "a surprise 30th birthday".into(),
            location: "a downtown rooftop".into(),
            aliases: vec!["rooftop birthday".into(), "30th birthday".into()],
            roles: vec!["a".into(), "b".into()],
        }
    }

    #[test]
    fn deck_loads_and_has_enough_roles() {
        let d = deck();
        assert!(d.len() >= 10);
        for s in &d {
            assert!(s.roles.len() >= 9, "{} has too few roles", s.id);
            assert!(!s.aliases.is_empty(), "{} has no aliases", s.id);
        }
    }

    #[test]
    fn guess_matching_is_lenient_but_not_trivial() {
        let s = scenario();
        assert!(s.guess_matches("rooftop birthday party"));
        assert!(s.guess_matches("a 30th birthday"));
        assert!(s.guess_matches("ROOFTOP BIRTHDAY!"));
        assert!(s.guess_matches("a surprise 30th birthday"), "the occasion wins");
        assert!(!s.guess_matches("beach wedding"));
        assert!(!s.guess_matches("a"));
        // The bare venue is generic/shared, so naming it alone must not win.
        assert!(!s.guess_matches("a downtown rooftop"));
    }
}
