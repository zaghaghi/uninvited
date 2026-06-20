// Prompt construction + reply parsing for the AI players. Every AI action is a
// single stateless `engine.generate(system, user)` call: the system string is
// the agent's private role briefing, the user string is the shared public
// transcript plus the task for this turn. Nothing here knows about Tauri — the
// orchestrator wires these to the engine.

use crate::game::{GameSession, Player, Role};

/// One short sentence, quotes stripped, single line — what we want back from a
/// small model that loves to ramble.
pub fn clean_line(s: &str) -> String {
    let mut t = s.trim();
    // Strip a leading label some models add ("Question:", "Answer:").
    for label in ["Question:", "Answer:", "Q:", "A:"] {
        if let Some(rest) = t.strip_prefix(label) {
            t = rest.trim();
        }
    }
    let first = t.lines().find(|l| !l.trim().is_empty()).unwrap_or(t).trim();
    first.trim_matches(|c| c == '"' || c == '\'' || c == '*').trim().to_string()
}

fn names_csv(session: &GameSession, except: u8) -> String {
    session
        .players
        .iter()
        .filter(|p| p.id != except)
        .map(|p| p.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The public Q&A so far, as plain prose the model can read.
fn transcript_block(session: &GameSession) -> String {
    if session.transcript.is_empty() {
        return "No questions have been asked yet.".to_string();
    }
    let mut out = String::from("Conversation so far:\n");
    for t in &session.transcript {
        let asker = &session.player(t.asker).name;
        let target = &session.player(t.target).name;
        out.push_str(&format!("- {asker} asked {target}: {}\n", t.question));
        out.push_str(&format!("  {target} replied: {}\n", t.answer));
    }
    out
}

fn invited_system(occasion: &str, location: &str, party_role: &str) -> String {
    format!(
        "You are playing a party deduction game. You and the others were all invited to an \
         upcoming party and are chatting BEFORE it happens — nobody is there yet. The party: the \
         occasion is {occasion} and the venue is {location}. Your role among the guests: \
         {party_role}. Everyone here was invited and knows \
         the plan — except ONE impostor who wasn't invited and does NOT know what the party is. \
         Your goal is to spot the impostor from how people talk. \
         The party is still in the FUTURE: talk about getting ready and looking forward to it, \
         never as if you are already there or it already happened. \
         Do NOT give the party away: never state its name, the occasion, who or what it is for, or \
         the venue — the impostor is listening and any of those hands them the answer. Drop only \
         subtle, oblique hints. Keep every reply to ONE short, natural sentence."
    )
}

fn uninvited_system() -> String {
    "You are playing a party deduction game. Everyone is chatting BEFORE an upcoming party that \
     hasn't happened yet. You were NOT invited and do NOT know what the party is; everyone else was \
     invited and knows. Your goal is to blend in so nobody suspects you, while quietly figuring out \
     what the party is from what people say. \
     The party is still in the FUTURE — speak about it as something coming up, never as if you are \
     already there. Stay vague and agreeable, never admit you don't know, and never get caught. \
     Keep every reply to ONE short, natural sentence."
        .to_string()
}

/// System briefing for a player based on their secret role.
fn system_for(session: &GameSession, player: &Player) -> String {
    match &player.role {
        Role::Invited { party_role } => {
            invited_system(&session.scenario.occasion, &session.scenario.location, party_role)
        }
        Role::Uninvited => uninvited_system(),
    }
}

/// An invited or (fallback) uninvited AI asks `target` a question.
pub fn ask_prompt(session: &GameSession, asker: &Player, target_id: u8) -> (String, String) {
    let target = &session.player(target_id).name;
    let user = format!(
        "{}\n\nIt is your turn to ask. Ask {target} ONE short, specific question about the \
         upcoming party that a real invited guest could answer but an impostor would fumble — \
         without giving the party away yourself. Reply with ONLY the question.",
        transcript_block(session)
    );
    (system_for(session, asker), user)
}

/// An invited AI answers a question put to it.
pub fn answer_prompt(
    session: &GameSession,
    answerer: &Player,
    asker_id: u8,
    question: &str,
) -> (String, String) {
    let asker = &session.player(asker_id).name;
    let task = match &answerer.role {
        Role::Invited { party_role } => format!(
            "{asker} just asked you: \"{question}\". Answer in ONE short sentence, in character as \
             {party_role}, as if the party is still coming up. Sound like you were invited, but \
             don't reveal the party — no name, occasion, who/what it's for, or venue; a subtle \
             hint at most."
        ),
        Role::Uninvited => format!(
            "{asker} just asked you: \"{question}\". You don't actually know this party, so give \
             ONE short, vague but plausible answer that avoids specifics. Never admit you weren't \
             invited."
        ),
    };
    let user = format!("{}\n\n{task}", transcript_block(session));
    (system_for(session, answerer), user)
}

/// Voting: an AI names who it thinks the impostor is.
pub fn vote_prompt(session: &GameSession, voter: &Player) -> (String, String) {
    let names = names_csv(session, voter.id);
    let task = match voter.role {
        Role::Invited { .. } => format!(
            "Questioning is over. Based on the whole conversation, who is the impostor that wasn't \
             really invited? Reply with ONLY one name from: {names}."
        ),
        Role::Uninvited => format!(
            "Voting time — to avoid suspicion you must accuse someone. Pick the real guest who \
             seems most suspicious to the others. Reply with ONLY one name from: {names}."
        ),
    };
    let user = format!("{}\n\n{task}", transcript_block(session));
    (system_for(session, voter), user)
}

/// The uninvited AI's own turn: it may keep blending in by asking, or gamble a
/// guess at the party to win outright.
pub fn uninvited_turn_prompt(
    session: &GameSession,
    asker: &Player,
    target_id: u8,
) -> (String, String) {
    let target = &session.player(target_id).name;
    let user = format!(
        "{}\n\nIt's your turn and you still aren't certain what this party is. Choose ONE:\n\
         - If you are confident you know the party, reply exactly: GUESS: <the party>\n\
         - Otherwise, reply exactly: ASK: <a short, casual question to {target} that helps you \
         learn the party without exposing yourself>\n\
         Reply with a single line starting with GUESS: or ASK:.",
        transcript_block(session)
    );
    (system_for(session, asker), user)
}

/// What the uninvited AI decided to do on its turn.
pub enum UninvitedAction {
    Guess(String),
    Ask(String),
}

pub fn parse_uninvited_turn(reply: &str) -> UninvitedAction {
    let t = reply.trim();
    let lower = t.to_lowercase();
    if let Some(pos) = lower.find("guess:") {
        let g = clean_line(&t[pos + "guess:".len()..]);
        if !g.is_empty() {
            return UninvitedAction::Guess(g);
        }
    }
    let q = if let Some(pos) = lower.find("ask:") {
        clean_line(&t[pos + "ask:".len()..])
    } else {
        clean_line(t)
    };
    UninvitedAction::Ask(q)
}

/// Resolve a vote reply to a player id by matching a name. Falls back to a
/// random other player if the model didn't name anyone recognizable.
pub fn parse_vote(session: &GameSession, voter_id: u8, reply: &str) -> u8 {
    let lower = reply.to_lowercase();
    let mut best: Option<(usize, u8)> = None; // (match position, id)
    for p in &session.players {
        if p.id == voter_id {
            continue;
        }
        if let Some(pos) = lower.find(&p.name.to_lowercase()) {
            if best.map_or(true, |(b, _)| pos < b) {
                best = Some((pos, p.id));
            }
        }
    }
    if let Some((_, id)) = best {
        return id;
    }
    use rand::seq::SliceRandom;
    *session
        .other_ids(voter_id)
        .choose(&mut rand::thread_rng())
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_line_strips_noise() {
        assert_eq!(clean_line("  \"Hello there?\"  "), "Hello there?");
        assert_eq!(clean_line("Question: What's up?\nextra"), "What's up?");
        assert_eq!(clean_line("*nods* sure"), "nods* sure".trim_start_matches('*'));
    }

    #[test]
    fn parse_uninvited_turn_detects_guess_and_ask() {
        match parse_uninvited_turn("GUESS: a beach wedding") {
            UninvitedAction::Guess(g) => assert_eq!(g, "a beach wedding"),
            _ => panic!("expected guess"),
        }
        match parse_uninvited_turn("ASK: So how do you know the host?") {
            UninvitedAction::Ask(q) => assert_eq!(q, "So how do you know the host?"),
            _ => panic!("expected ask"),
        }
        // No prefix -> treated as an ask.
        match parse_uninvited_turn("How long have you been here?") {
            UninvitedAction::Ask(q) => assert_eq!(q, "How long have you been here?"),
            _ => panic!("expected ask"),
        }
    }
}
