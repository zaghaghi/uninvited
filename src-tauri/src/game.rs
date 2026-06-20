// Core game state: players, roles, the shared public transcript, and the
// rules (role assignment, vote tally, win conditions). `GameSession` holds the
// full truth — including secrets the human must not see. `GameView` is the
// filtered snapshot we emit to the webview: it reveals the location only to
// invited players (and to everyone at the reveal), and never reveals who the
// uninvited player is until the game ends.

use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};

use crate::scenarios::{self, Scenario};

/// The human is always player 0.
pub const HUMAN_ID: u8 = 0;

/// Rounds of questioning before the vote (each player asks once per round).
pub fn rounds_total() -> u8 {
    std::env::var("UNINVITED_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2)
}

const AI_NAMES: &[&str] = &[
    "Mia", "Leo", "Zoe", "Kai", "Nora", "Theo", "Iris", "Felix", "Ruby", "Otis", "Luna", "Gus",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Invited,
    Uninvited,
}

#[derive(Clone, Debug)]
pub enum Role {
    Invited { party_role: String },
    Uninvited,
}

impl Role {
    pub fn side(&self) -> Side {
        match self {
            Role::Invited { .. } => Side::Invited,
            Role::Uninvited => Side::Uninvited,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Player {
    pub id: u8,
    pub name: String,
    pub is_human: bool,
    pub role: Role,
    pub vote_for: Option<u8>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    pub asker: u8,
    pub target: u8,
    pub question: String,
    pub answer: String,
}

/// The exchange currently in flight, surfaced before its answer exists so the
/// player isn't staring at a blank screen while the answerer is generated.
/// `question: None` means the asker is still composing it; `Some` means it has
/// been asked and we're now waiting on the answer. Carries no secret (the
/// question is public the moment it's asked), so `view()` passes it through.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingTurn {
    pub asker: u8,
    pub target: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Questioning,
    Voting,
    Reveal,
}

/// What input the UI must collect from the human right now.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Awaiting {
    /// Human's turn to ask `target` a question (or guess, if uninvited).
    Question { target: u8 },
    /// Human must answer the question `asker` posed.
    Answer { asker: u8, question: String },
    /// Voting phase: human must pick who is uninvited.
    Vote,
}

/// How the human takes part. `Invited`/`Outsider` are the two classic seats;
/// `Bartender` is a spectator-judge: invited-side (knows the party) but absent
/// from the Q&A chain and the sole voter — the AI guests never vote.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GameMode {
    Invited,
    Outsider,
    Bartender,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameConfig {
    pub mode: GameMode,
    /// Number of AI players (2..=8).
    pub ai_count: u8,
}

#[derive(Clone, Debug)]
pub struct LocationGuess {
    pub player: u8,
    pub guess: String,
    pub correct: bool,
}

pub struct GameSession {
    pub scenario: Scenario,
    pub players: Vec<Player>,
    pub transcript: Vec<Turn>,
    pub phase: Phase,
    pub round: u8,
    pub rounds_total: u8,
    pub mode: GameMode,
    pub awaiting: Option<Awaiting>,
    /// The in-flight question/answer exchange, shown before the answer lands.
    pub pending: Option<PendingTurn>,
    /// The AI currently generating (a question, answer, or vote), so the UI can
    /// show it as thinking. `None` while it's a human's move or between turns.
    pub thinking: Option<u8>,
    pub guess: Option<LocationGuess>,
    pub winner: Option<Side>,
}

impl GameSession {
    /// Build a fresh game: draw a scenario, seat the players, and deal roles.
    pub fn new(cfg: &GameConfig) -> Self {
        let ai_count = cfg.ai_count.clamp(2, 8);
        let scenario = scenarios::draw();
        let mut rng = rand::thread_rng();

        // Pick AI names.
        let mut names: Vec<&str> = AI_NAMES.to_vec();
        names.shuffle(&mut rng);

        let total = ai_count as usize + 1;

        // Decide who is uninvited. Unless the human plays the outsider (in which
        // case it's them), exactly one AI is — true for Invited and Bartender alike.
        let uninvited_id: u8 = if !matches!(cfg.mode, GameMode::Outsider) {
            // some AI in 1..=ai_count
            1 + (0..ai_count).collect::<Vec<_>>().choose(&mut rng).copied().unwrap_or(0)
        } else {
            HUMAN_ID
        };

        // Deal unique party roles to the invited players.
        let mut roles: Vec<String> = scenario.roles.clone();
        roles.shuffle(&mut rng);
        let mut role_iter = roles.into_iter();

        let mut players = Vec::with_capacity(total);
        for id in 0..total as u8 {
            let is_human = id == HUMAN_ID;
            let name = if is_human {
                "You".to_string()
            } else {
                names
                    .get((id - 1) as usize)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("Guest {id}"))
            };
            let role = if id == uninvited_id {
                Role::Uninvited
            } else {
                Role::Invited {
                    party_role: role_iter.next().unwrap_or_else(|| "a guest".to_string()),
                }
            };
            players.push(Player {
                id,
                name,
                is_human,
                role,
                vote_for: None,
            });
        }

        GameSession {
            scenario,
            players,
            transcript: Vec::new(),
            phase: Phase::Questioning,
            round: 1,
            rounds_total: rounds_total(),
            mode: cfg.mode,
            awaiting: None,
            pending: None,
            thinking: None,
            guess: None,
            winner: None,
        }
    }

    pub fn player(&self, id: u8) -> &Player {
        &self.players[id as usize]
    }

    pub fn uninvited_id(&self) -> u8 {
        self.players
            .iter()
            .find(|p| p.role.side() == Side::Uninvited)
            .map(|p| p.id)
            .expect("a session always has one uninvited player")
    }

    pub fn other_ids(&self, except: u8) -> Vec<u8> {
        self.players
            .iter()
            .map(|p| p.id)
            .filter(|&id| id != except)
            .collect()
    }

    /// The human is on the invited side (knows the party) in every mode but
    /// `Outsider`. Drives the secret-filtering in `view()`.
    pub fn human_invited(&self) -> bool {
        !matches!(self.mode, GameMode::Outsider)
    }

    /// In Bartender mode the human only watches the Q&A and is the lone voter.
    pub fn bartender_mode(&self) -> bool {
        matches!(self.mode, GameMode::Bartender)
    }

    /// Record a location guess and decide the game if it is correct.
    pub fn record_guess(&mut self, player: u8, guess: String) -> bool {
        let correct = self.scenario.guess_matches(&guess);
        self.guess = Some(LocationGuess {
            player,
            guess,
            correct,
        });
        if correct {
            self.winner = Some(Side::Uninvited);
            self.phase = Phase::Reveal;
        }
        correct
    }

    /// Tally votes and decide the winner. Invited win only if the plurality
    /// (strict, no tie) lands on the uninvited player; otherwise the uninvited
    /// survives and wins.
    pub fn resolve_votes(&mut self) -> Side {
        let n = self.players.len();
        let mut tally = vec![0u32; n];
        for p in &self.players {
            if let Some(t) = p.vote_for {
                tally[t as usize] += 1;
            }
        }
        let max = tally.iter().copied().max().unwrap_or(0);
        let leaders: Vec<usize> = (0..n).filter(|&i| tally[i] == max && max > 0).collect();
        let winner = if leaders.len() == 1 && leaders[0] as u8 == self.uninvited_id() {
            Side::Invited
        } else {
            Side::Uninvited
        };
        self.winner = Some(winner);
        self.phase = Phase::Reveal;
        winner
    }

    /// Build the snapshot for the webview, filtered to the human's knowledge.
    pub fn view(&self) -> GameView {
        let revealed = self.phase == Phase::Reveal;
        let players: Vec<PlayerView> = self
            .players
            .iter()
            .map(|p| PlayerView {
                id: p.id,
                name: p.name.clone(),
                is_human: p.is_human,
                // Reveal each player's side only at the end.
                side: if revealed { Some(p.role.side()) } else { None },
            })
            .collect();

        let human = self.player(HUMAN_ID);
        let party_role = match &human.role {
            Role::Invited { party_role } => Some(party_role.clone()),
            Role::Uninvited => None,
        };
        // The party (occasion + venue) is known to invited players the whole
        // game, and to everyone once the game is revealed.
        let (occasion, location) = if self.human_invited() || revealed {
            (
                Some(self.scenario.occasion.clone()),
                Some(self.scenario.location.clone()),
            )
        } else {
            (None, None)
        };

        let votes = if matches!(self.phase, Phase::Voting | Phase::Reveal) {
            Some(
                self.players
                    .iter()
                    .filter_map(|p| {
                        p.vote_for.map(|t| VoteView {
                            voter: p.id,
                            // Who voted is public the moment they vote, but the
                            // target stays hidden (rendered "***") until the
                            // reveal so it can't sway the human — who always
                            // sees their own pick.
                            target: if revealed || p.id == HUMAN_ID {
                                Some(t)
                            } else {
                                None
                            },
                        })
                    })
                    .collect(),
            )
        } else {
            None
        };

        GameView {
            phase: self.phase,
            players,
            human_id: HUMAN_ID,
            human_invited: self.human_invited(),
            mode: self.mode,
            party_role,
            occasion,
            location,
            round: self.round,
            rounds_total: self.rounds_total,
            transcript: self.transcript.clone(),
            awaiting: self.awaiting.clone(),
            // Public — the question is shown the instant it's asked, and which
            // AI is thinking is no secret — so these pass through unfiltered.
            pending: self.pending.clone(),
            thinking: self.thinking,
            votes,
            uninvited_id: if revealed { Some(self.uninvited_id()) } else { None },
            winner: if revealed { self.winner } else { None },
            guess: self.guess.as_ref().map(|g| GuessView {
                player: g.player,
                guess: g.guess.clone(),
                correct: g.correct,
            }),
        }
    }
}

// -- frontend-facing snapshot (serde camelCase) ---------------------------

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerView {
    pub id: u8,
    pub name: String,
    pub is_human: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<Side>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoteView {
    pub voter: u8,
    /// The voted-for player, or `None` while the vote is masked (pre-reveal).
    pub target: Option<u8>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuessView {
    pub player: u8,
    pub guess: String,
    pub correct: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameView {
    pub phase: Phase,
    pub players: Vec<PlayerView>,
    pub human_id: u8,
    pub human_invited: bool,
    pub mode: GameMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub party_role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occasion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    pub round: u8,
    pub rounds_total: u8,
    pub transcript: Vec<Turn>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub awaiting: Option<Awaiting>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending: Option<PendingTurn>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub votes: Option<Vec<VoteView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uninvited_id: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub winner: Option<Side>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guess: Option<GuessView>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(human_invited: bool, ai: u8) -> GameConfig {
        GameConfig {
            mode: if human_invited {
                GameMode::Invited
            } else {
                GameMode::Outsider
            },
            ai_count: ai,
        }
    }

    #[test]
    fn human_uninvited_means_human_is_the_outsider() {
        let s = GameSession::new(&cfg(false, 4));
        assert_eq!(s.players.len(), 5);
        assert_eq!(s.uninvited_id(), HUMAN_ID);
        assert_eq!(s.player(HUMAN_ID).role.side(), Side::Uninvited);
    }

    #[test]
    fn human_invited_means_one_ai_is_uninvited() {
        let s = GameSession::new(&cfg(true, 4));
        assert_eq!(s.player(HUMAN_ID).role.side(), Side::Invited);
        let outsiders = s
            .players
            .iter()
            .filter(|p| p.role.side() == Side::Uninvited)
            .count();
        assert_eq!(outsiders, 1);
        assert_ne!(s.uninvited_id(), HUMAN_ID);
    }

    #[test]
    fn invited_players_get_unique_roles() {
        let s = GameSession::new(&cfg(true, 6));
        let mut roles: Vec<String> = s
            .players
            .iter()
            .filter_map(|p| match &p.role {
                Role::Invited { party_role } => Some(party_role.clone()),
                Role::Uninvited => None,
            })
            .collect();
        let total = roles.len();
        roles.sort();
        roles.dedup();
        assert_eq!(roles.len(), total, "roles must be unique");
    }

    #[test]
    fn view_hides_secrets_from_uninvited_human() {
        let s = GameSession::new(&cfg(false, 3));
        let v = s.view();
        assert!(v.location.is_none(), "uninvited human must not see the venue");
        assert!(v.occasion.is_none(), "uninvited human must not see the occasion");
        assert!(v.party_role.is_none());
        assert!(v.uninvited_id.is_none(), "outsider id hidden mid-game");
        assert!(v.players.iter().all(|p| p.side.is_none()));
    }

    #[test]
    fn view_shows_location_to_invited_human_but_not_outsider() {
        let s = GameSession::new(&cfg(true, 3));
        let v = s.view();
        assert!(v.location.is_some(), "invited human sees the venue");
        assert!(v.occasion.is_some(), "invited human sees the occasion");
        assert!(v.party_role.is_some());
        assert!(v.uninvited_id.is_none(), "outsider id hidden mid-game");
    }

    #[test]
    fn correct_guess_makes_uninvited_win() {
        let mut s = GameSession::new(&cfg(false, 3));
        // The occasion (not the generic venue) is the guessable identity.
        let party = s.scenario.occasion.clone();
        assert!(s.record_guess(HUMAN_ID, party));
        assert_eq!(s.winner, Some(Side::Uninvited));
        assert_eq!(s.phase, Phase::Reveal);
    }

    #[test]
    fn plurality_on_outsider_makes_invited_win() {
        let mut s = GameSession::new(&cfg(true, 3));
        let outsider = s.uninvited_id();
        for p in &mut s.players {
            p.vote_for = Some(outsider);
        }
        assert_eq!(s.resolve_votes(), Side::Invited);
    }

    #[test]
    fn tie_lets_outsider_survive() {
        let mut s = GameSession::new(&cfg(true, 3)); // 4 players
        let outsider = s.uninvited_id();
        let others = s.other_ids(outsider);
        // Two votes for the outsider, two votes for someone else -> tie.
        s.players[0].vote_for = Some(outsider);
        s.players[1].vote_for = Some(outsider);
        s.players[2].vote_for = Some(others[0]);
        s.players[3].vote_for = Some(others[0]);
        assert_eq!(s.resolve_votes(), Side::Uninvited);
    }

    fn bartender_cfg(ai: u8) -> GameConfig {
        GameConfig {
            mode: GameMode::Bartender,
            ai_count: ai,
        }
    }

    #[test]
    fn bartender_is_invited_side_with_one_ai_outsider() {
        let s = GameSession::new(&bartender_cfg(4));
        assert!(s.bartender_mode());
        assert!(s.human_invited(), "bartender knows the party");
        assert_eq!(s.player(HUMAN_ID).role.side(), Side::Invited);
        assert_ne!(s.uninvited_id(), HUMAN_ID);
        let outsiders = s
            .players
            .iter()
            .filter(|p| p.role.side() == Side::Uninvited)
            .count();
        assert_eq!(outsiders, 1);
        // The bartender sees the party, just like any invited guest.
        assert!(s.view().occasion.is_some());
    }

    #[test]
    fn bartenders_lone_vote_decides() {
        // Only the human votes in this mode; the single vote is a plurality.
        let mut s = GameSession::new(&bartender_cfg(3));
        let outsider = s.uninvited_id();
        s.players[HUMAN_ID as usize].vote_for = Some(outsider);
        assert_eq!(s.resolve_votes(), Side::Invited, "right call catches the outsider");

        let mut s = GameSession::new(&bartender_cfg(3));
        let innocent = s.other_ids(s.uninvited_id())[0];
        s.players[HUMAN_ID as usize].vote_for = Some(innocent);
        assert_eq!(s.resolve_votes(), Side::Uninvited, "wrong call lets them slip away");
    }
}
