//! The morning the local model is allowed to speak.
//!
//! Rust lays out a few bids the transfer rules would already accept, at fees the
//! market would write. The model picks one option per club, or passes, and may
//! add a sentence. [`apply_director_choices`] writes that fee, or nothing.
//! A pass is remembered so the ordinary market does not bid for that club again
//! the same day.

use crate::game::{DirectorJournalEntry, Game};
use crate::transfers::{
    TransferIntent, TransferIntentOutcome, affordable_user_targets, apply_transfer_intent,
};
use domain::league::FixtureStatus;
use domain::season::TransferWindowStatus;
use serde::Deserialize;

const SPOTLIGHT_CLUBS: usize = 3;
const SPOTLIGHT_PLAYERS: usize = 8;
const ORBIT_CLUBS: usize = 3;
const OPTIONS_PER_CLUB: usize = 3;
const JOURNAL_LIMIT: usize = 8;
const TONE_CHAR_LIMIT: usize = 480;
const PASS: &str = "pass";

/// What the model is allowed to see for one morning. Ids here are the only ids
/// a bid can name; the prompt is this value as JSON.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SpotlightBrief {
    pub date: String,
    pub clubs: Vec<ClubBrief>,
    pub players: Vec<PlayerBrief>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ClubBrief {
    pub id: String,
    pub name: String,
    pub reputation: u32,
    pub transfer_budget: i64,
    pub finance: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PlayerBrief {
    pub id: String,
    pub name: String,
    pub position: String,
    pub market_value: u64,
    pub transfer_listed: bool,
}

/// A bid Rust accepted. The prose call may color it. It may not change `fee`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedBid {
    pub date: String,
    pub club_id: String,
    pub club_name: String,
    pub player_id: String,
    pub player_name: String,
    pub fee: u64,
}

#[derive(Debug, Deserialize)]
struct RawIntent {
    kind: String,
    club_id: String,
    player_id: String,
    #[serde(default)]
    fee: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct IntentBatch {
    intents: Vec<RawIntent>,
}

#[derive(Debug, Deserialize)]
struct NoteBatch {
    notes: Vec<RawNote>,
}

#[derive(Debug, Deserialize)]
struct RawNote {
    player_id: String,
    text: String,
}

/// Clubs in the active leagues, other than the user's, with money to spend,
/// and the user's most valuable players. `None` when a bid could not be an
/// inbox offer: the window is shut, or there is no club and no player to name.
pub fn spotlight_brief(game: &Game) -> Option<SpotlightBrief> {
    if !matches!(
        game.season_context.transfer_window.status,
        TransferWindowStatus::Open | TransferWindowStatus::DeadlineDay
    ) {
        return None;
    }
    let user_team_id = game.manager.team_id.as_deref()?;
    let active = game.active_team_ids();
    let mut clubs: Vec<&domain::team::Team> = game
        .teams
        .iter()
        .filter(|team| team.id != user_team_id)
        .filter(|team| active.as_ref().is_none_or(|ids| ids.contains(&team.id)))
        .filter(|team| team.transfer_budget > 0 && team.finance > 0)
        .collect();
    clubs.sort_by_key(|team| std::cmp::Reverse(team.reputation));
    let clubs: Vec<ClubBrief> = clubs
        .into_iter()
        .take(SPOTLIGHT_CLUBS)
        .map(|team| ClubBrief {
            id: team.id.clone(),
            name: team.name.clone(),
            reputation: team.reputation,
            transfer_budget: team.transfer_budget,
            finance: team.finance,
        })
        .collect();
    if clubs.is_empty() {
        return None;
    }

    let mut players: Vec<&domain::player::Player> = game
        .players
        .iter()
        .filter(|player| player.team_id.as_deref() == Some(user_team_id))
        .collect();
    players.sort_by_key(|player| std::cmp::Reverse(player.market_value));
    let players: Vec<PlayerBrief> = players
        .into_iter()
        .take(SPOTLIGHT_PLAYERS)
        .map(|player| PlayerBrief {
            id: player.id.clone(),
            name: player.full_name.clone(),
            position: format!("{:?}", player.position),
            market_value: player.market_value,
            transfer_listed: player.transfer_listed,
        })
        .collect();
    if players.is_empty() {
        return None;
    }

    Some(SpotlightBrief {
        date: game.clock.current_date.format("%Y-%m-%d").to_string(),
        clubs,
        players,
    })
}

pub fn intent_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["intents"],
        "properties": {
            "intents": {
                "type": "array",
                "maxItems": SPOTLIGHT_CLUBS,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["kind", "club_id", "player_id"],
                    "properties": {
                        "kind": { "type": "string", "enum": ["bid", "pass"] },
                        "club_id": { "type": "string" },
                        "player_id": { "type": "string" },
                        "fee": { "type": "integer", "minimum": 0 }
                    }
                }
            }
        }
    })
}

pub fn note_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["notes"],
        "properties": {
            "notes": {
                "type": "array",
                "maxItems": SPOTLIGHT_CLUBS,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["player_id", "text"],
                    "properties": {
                        "player_id": { "type": "string" },
                        "text": { "type": "string" }
                    }
                }
            }
        }
    })
}

pub const INTENT_SYSTEM: &str = "You are the transfer desk of a football world. Reply with JSON only, no reasoning. Use only the club_id and player_id values in the user message. Each club appears at most once. kind \"bid\" needs a whole-number fee the club can pay from transfer_budget and finance. kind \"pass\" means the club does not bid. Do not invent clubs, players, or fees you were not given room for.";

pub const NOTE_SYSTEM: &str = "You write one short sentence of color for each accepted bid. Reply with JSON only, no reasoning. Do not mention money, fees, wages, or any number. The game prints the fee itself.";

pub fn intent_user_prompt(brief: &SpotlightBrief) -> String {
    serde_json::to_string(brief).unwrap_or_else(|_| "{}".to_string())
}

pub fn note_user_prompt(bids: &[AcceptedBid]) -> String {
    let facts: Vec<_> = bids
        .iter()
        .map(|bid| {
            serde_json::json!({
                "player_id": bid.player_id,
                "player": bid.player_name,
                "club": bid.club_name,
            })
        })
        .collect();
    serde_json::to_string(&facts).unwrap_or_else(|_| "[]".to_string())
}

/// Read a model reply into intents. A fence, a bare array, one object, or a
/// batch all work. A bid without a fee, and any object that is not the schema,
/// is dropped and writes nothing.
pub fn parse_transfer_intents(body: &str) -> Vec<TransferIntent> {
    let payload = json_payload(body);
    let raw = if let Ok(batch) = serde_json::from_str::<IntentBatch>(payload) {
        batch.intents
    } else if let Ok(list) = serde_json::from_str::<Vec<RawIntent>>(payload) {
        list
    } else if let Ok(one) = serde_json::from_str::<RawIntent>(payload) {
        vec![one]
    } else {
        return Vec::new();
    };
    raw.into_iter().filter_map(raw_to_intent).collect()
}

fn raw_to_intent(raw: RawIntent) -> Option<TransferIntent> {
    match raw.kind.as_str() {
        "pass" => Some(TransferIntent::Pass {
            club_id: raw.club_id,
            player_id: raw.player_id,
        }),
        "bid" => raw.fee.map(|fee| TransferIntent::Bid {
            club_id: raw.club_id,
            player_id: raw.player_id,
            fee,
        }),
        _ => None,
    }
}

/// Apply every parsed intent. Accepted bids come back with the fee Rust stored.
/// Rejections and passes are not in the list; they have already written nothing.
pub fn apply_director_intents(game: &mut Game, intents: Vec<TransferIntent>) -> Vec<AcceptedBid> {
    let date = game.clock.current_date.format("%Y-%m-%d").to_string();
    let mut accepted = Vec::new();
    for intent in intents {
        let (club_id, player_id) = match &intent {
            TransferIntent::Bid {
                club_id, player_id, ..
            } => (club_id.clone(), player_id.clone()),
            TransferIntent::Pass { .. } => {
                apply_transfer_intent(game, intent);
                continue;
            }
        };
        match apply_transfer_intent(game, intent) {
            TransferIntentOutcome::Accepted { fee: stored } => {
                let club_name = game
                    .teams
                    .iter()
                    .find(|team| team.id == club_id)
                    .map(|team| team.name.clone())
                    .unwrap_or(club_id.clone());
                let player_name = game
                    .players
                    .iter()
                    .find(|player| player.id == player_id)
                    .map(|player| player.full_name.clone())
                    .unwrap_or_default();
                accepted.push(AcceptedBid {
                    date: date.clone(),
                    club_id,
                    club_name,
                    player_id,
                    player_name,
                    fee: stored,
                });
            }
            TransferIntentOutcome::Rejected { .. } | TransferIntentOutcome::Passed => {}
        }
    }
    accepted
}

pub fn parse_director_notes(body: &str) -> Vec<(String, String)> {
    let payload = json_payload(body);
    let Ok(batch) = serde_json::from_str::<NoteBatch>(payload) else {
        return Vec::new();
    };
    batch
        .notes
        .into_iter()
        .filter_map(|note| {
            let text = clean_tone(&note.text)?;
            Some((note.player_id, text))
        })
        .collect()
}

/// Put the sentence on the inbox thread for this bid. The offer fee stays
/// `bid.fee`. A sentence that contains a digit is refused, so the model cannot
/// print a second price; the template the market already wrote stays.
pub fn attach_director_note(game: &mut Game, bid: &AcceptedBid, tone: &str) -> bool {
    let Some(tone) = clean_tone(tone) else {
        return false;
    };
    let offer_still_there = game.players.iter().any(|player| {
        player.id == bid.player_id
            && player.transfer_offers.iter().any(|offer| {
                offer.from_team_id == bid.club_id
                    && offer.fee == bid.fee
                    && offer.status == domain::player::TransferOfferStatus::Pending
            })
    });
    if !offer_still_there {
        return false;
    }
    let message_id = format!("transfer_interest_{}", bid.player_id);
    let Some(message) = game
        .messages
        .iter_mut()
        .find(|message| message.id == message_id)
    else {
        return false;
    };
    let fee =
        crate::currency::format_compact_money(bid.fee, crate::currency::DEFAULT_CURRENCY_CODE)
            .unwrap_or_else(|| bid.fee.to_string());
    message.body_key = None;
    message.body = format!(
        "{tone}\n\n{} bid {fee} for {}.",
        bid.club_name, bid.player_name
    );
    true
}

pub(crate) fn clean_tone(tone: &str) -> Option<String> {
    let tone = tone.split_whitespace().collect::<Vec<_>>().join(" ");
    if tone.is_empty()
        || tone.chars().count() > TONE_CHAR_LIMIT
        || tone.chars().any(|c| c.is_ascii_digit())
    {
        return None;
    }
    Some(tone)
}

/// One morning the model is allowed to choose from. Every option is a bid
/// [`apply_transfer_intent`] would accept at the fee already printed on it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DirectorSlate {
    pub date: String,
    pub memory: Vec<DirectorJournalEntry>,
    pub clubs: Vec<DirectorClubSlate>,
    /// English name of the game language, so the model writes in that language.
    pub language: String,
    pub voice: Option<VoiceTask>,
}

/// One fact the model may speak about this morning, besides the bids.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct VoiceTask {
    pub kind: String,
    pub speaker_name: String,
    pub speaker_role: String,
    pub club_id: String,
    pub club_name: String,
    pub player_id: String,
    pub player_name: String,
    pub facts: String,
    pub home_name: String,
    pub away_name: String,
    pub home_goals: u8,
    pub away_goals: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DirectorClubSlate {
    pub club_id: String,
    /// One letter the model echoes instead of the saved team id.
    pub code: String,
    pub name: String,
    pub speaker_name: String,
    /// `manager` or `assistant`. Empty when the save has no named person.
    pub speaker_role: String,
    pub options: Vec<DirectorOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DirectorOption {
    pub id: String,
    pub player_id: String,
    pub player_name: String,
    pub position: String,
    pub fee: u64,
    pub squad_depth: usize,
    pub contract_end: Option<String>,
}

/// The model's pick. `option_id` is `"pass"` or an id from that club's options.
/// The fee is never on this value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectorChoice {
    pub club_id: String,
    pub option_id: String,
    pub reason: String,
    pub text: String,
}

#[derive(Debug, Deserialize)]
struct ChoiceBatch {
    #[serde(default)]
    choices: Vec<RawChoice>,
    #[serde(default)]
    voice: String,
}

#[derive(Debug, Deserialize)]
struct RawChoice {
    club_id: String,
    option_id: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    text: String,
}

pub const CHOICE_SYSTEM: &str = "You write in the language named language in the user message. Keep every person and club name exactly as given. Reply with JSON only, no reasoning. For each club, set club_id to that club's club_id and option_id to one id from its options, or \"pass\". Those ids are short. Do not invent others. reason is squad_need, contract, ambition, or rivalry. text is two or three sentences spoken by that club's speaker about this player, using the position, the contract end, or the squad depth, and memory when that club appears in it. No digits and no fee. Do not invent a person. When voice_kind is not empty, voice is two or three sentences spoken by voice_speaker about voice_facts, with no digits. When voice_kind is empty, voice is an empty string. The fee on an option is what the game will offer if you pick it; you cannot change it.";

/// Clubs in the player's orbit that can legally bid today, plus the last
/// decisions. `None` when the window is shut or nobody can pay for anyone.
pub fn director_slate(game: &Game) -> Option<DirectorSlate> {
    if !matches!(
        game.season_context.transfer_window.status,
        TransferWindowStatus::Open | TransferWindowStatus::DeadlineDay
    ) {
        return None;
    }
    let user_team_id = game.manager.team_id.as_deref()?;
    let date = game.clock.current_date.format("%Y-%m-%d").to_string();
    let opponent = next_opponent_id(game, user_team_id, &date);
    let active = game.active_team_ids();
    let mut clubs: Vec<&domain::team::Team> = game
        .teams
        .iter()
        .filter(|team| team.id != user_team_id)
        .filter(|team| active.as_ref().is_none_or(|ids| ids.contains(&team.id)))
        .filter(|team| team.transfer_budget > 0 && team.finance > 0)
        .collect();
    clubs.sort_by_key(|team| {
        let opponent_first = if Some(team.id.as_str()) == opponent.as_deref() {
            0
        } else {
            1
        };
        (opponent_first, std::cmp::Reverse(team.reputation))
    });
    let mut clubs = clubs
        .into_iter()
        .filter_map(|team| club_slate(game, team))
        .take(ORBIT_CLUBS)
        .collect::<Vec<_>>();
    if clubs.is_empty() {
        return None;
    }
    assign_short_ids(&mut clubs);
    Some(DirectorSlate {
        date,
        memory: recent_memory(game),
        clubs,
        language: crate::director_voice::language_name("en").to_string(),
        voice: None,
    })
}

fn assign_short_ids(clubs: &mut [DirectorClubSlate]) {
    for (index, club) in clubs.iter_mut().enumerate() {
        let code = club_code(index);
        club.code = code.clone();
        for (number, option) in club.options.iter_mut().enumerate() {
            option.id = format!("{code}{}", number + 1);
        }
    }
}

fn club_code(index: usize) -> String {
    let letter = b'a'.saturating_add(u8::try_from(index).unwrap_or(0));
    char::from(letter).to_string()
}

fn club_slate(game: &Game, team: &domain::team::Team) -> Option<DirectorClubSlate> {
    let options = affordable_user_targets(game, &team.id)
        .into_iter()
        .take(OPTIONS_PER_CLUB)
        .map(|target| DirectorOption {
            id: String::new(),
            player_id: target.player_id,
            player_name: target.player_name,
            position: target.position,
            fee: target.fee,
            squad_depth: target.squad_depth,
            contract_end: target.contract_end,
        })
        .collect::<Vec<_>>();
    if options.is_empty() {
        return None;
    }
    let (speaker_name, speaker_role) = crate::director_voice::club_speaker(game, &team.id);
    Some(DirectorClubSlate {
        club_id: team.id.clone(),
        code: team.id.clone(),
        name: team.name.clone(),
        speaker_name,
        speaker_role,
        options,
    })
}

fn recent_memory(game: &Game) -> Vec<DirectorJournalEntry> {
    let skip = game.director_journal.len().saturating_sub(JOURNAL_LIMIT);
    game.director_journal.iter().skip(skip).cloned().collect()
}

fn next_opponent_id(game: &Game, user_team_id: &str, today: &str) -> Option<String> {
    let mut best: Option<(&str, String)> = None;
    for competition in game.competitions.iter().chain(game.league.iter()) {
        for fixture in &competition.fixtures {
            if fixture.status != FixtureStatus::Scheduled || fixture.date.as_str() < today {
                continue;
            }
            let opponent = if fixture.home_team_id == user_team_id {
                fixture.away_team_id.as_str()
            } else if fixture.away_team_id == user_team_id {
                fixture.home_team_id.as_str()
            } else {
                continue;
            };
            if best
                .as_ref()
                .is_none_or(|(date, _)| fixture.date.as_str() < *date)
            {
                best = Some((fixture.date.as_str(), opponent.to_string()));
            }
        }
    }
    best.map(|(_, opponent)| opponent)
}

pub fn choice_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["choices", "voice"],
        "properties": {
            "choices": {
                "type": "array",
                "maxItems": ORBIT_CLUBS,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["club_id", "option_id", "reason", "text"],
                    "properties": {
                        "club_id": { "type": "string" },
                        "option_id": { "type": "string" },
                        "reason": {
                            "type": "string",
                            "enum": ["squad_need", "contract", "ambition", "rivalry"]
                        },
                        "text": { "type": "string" }
                    }
                }
            },
            "voice": { "type": "string" }
        }
    })
}

pub fn choice_user_prompt(slate: &DirectorSlate) -> String {
    let memory: Vec<_> = slate
        .memory
        .iter()
        .map(|entry| {
            serde_json::json!({
                "date": entry.date,
                "club": entry.club_name,
                "player": entry.player_name,
                "action": entry.action,
                "reason": entry.reason,
            })
        })
        .collect();
    let clubs: Vec<_> = slate
        .clubs
        .iter()
        .map(|club| {
            let options: Vec<_> = club
                .options
                .iter()
                .map(|option| {
                    serde_json::json!({
                        "id": option.id,
                        "player": option.player_name,
                        "position": option.position,
                        "fee": option.fee,
                        "squad_depth": option.squad_depth,
                        "contract_end": option.contract_end,
                    })
                })
                .collect();
            serde_json::json!({
                "club_id": club.code,
                "name": club.name,
                "speaker": club.speaker_name,
                "speaker_role": club.speaker_role,
                "options": options,
            })
        })
        .collect();
    let voice = slate.voice.as_ref();
    serde_json::to_string(&serde_json::json!({
        "language": slate.language,
        "date": slate.date,
        "memory": memory,
        "clubs": clubs,
        "voice_kind": voice.map(|task| task.kind.as_str()).unwrap_or(""),
        "voice_speaker": voice.map(|task| task.speaker_name.as_str()).unwrap_or(""),
        "voice_role": voice.map(|task| task.speaker_role.as_str()).unwrap_or(""),
        "voice_club": voice.map(|task| task.club_name.as_str()).unwrap_or(""),
        "voice_facts": voice.map(|task| task.facts.as_str()).unwrap_or(""),
    }))
    .unwrap_or_else(|_| "{}".to_string())
}

pub fn parse_director_choices(body: &str) -> Vec<DirectorChoice> {
    parsed_batch(body)
        .map(|batch| batch.choices.into_iter().map(raw_to_choice).collect())
        .unwrap_or_else(|| {
            // A reply cut off at the token limit is not valid JSON. Keep every
            // choice object that did close, and drop the one the cut landed in.
            finished_choice_objects(json_payload(body))
                .into_iter()
                .filter_map(|object| serde_json::from_str::<RawChoice>(object).ok())
                .map(raw_to_choice)
                .collect()
        })
}

pub fn parse_director_voice(body: &str) -> String {
    parsed_batch(body)
        .map(|batch| batch.voice)
        .unwrap_or_default()
}

fn parsed_batch(body: &str) -> Option<ChoiceBatch> {
    serde_json::from_str::<ChoiceBatch>(json_payload(body)).ok()
}

fn raw_to_choice(choice: RawChoice) -> DirectorChoice {
    DirectorChoice {
        club_id: choice.club_id,
        option_id: choice.option_id,
        reason: known_reason(&choice.reason),
        text: choice.text,
    }
}

fn finished_choice_objects(payload: &str) -> Vec<&str> {
    let mut objects = Vec::new();
    let mut index = 0;
    while index < payload.len() {
        let Some(relative) = payload[index..].find('{') else {
            break;
        };
        let start = index + relative;
        match balanced_object_end(&payload[start..]) {
            Some(end) => {
                objects.push(&payload[start..=start + end]);
                index = start + end + 1;
            }
            None => index = start + 1,
        }
    }
    objects
}

fn balanced_object_end(text: &str) -> Option<usize> {
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escape = false;
    for (index, ch) in text.char_indices() {
        if in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn known_reason(reason: &str) -> String {
    match reason {
        "squad_need" | "contract" | "ambition" | "rivalry" => reason.to_string(),
        _ => "none".to_string(),
    }
}

/// Apply the picks. A pass writes a journal line and no offer. A known option
/// writes the slate's fee. An unknown id writes nothing, so the market may
/// still speak for that club.
pub fn apply_director_choices(
    game: &mut Game,
    slate: &DirectorSlate,
    choices: &[DirectorChoice],
) -> Vec<AcceptedBid> {
    if game.clock.current_date.format("%Y-%m-%d").to_string() != slate.date {
        return Vec::new();
    }
    let mut seen = std::collections::HashSet::new();
    let mut accepted = Vec::new();
    for choice in choices {
        if !seen.insert(choice.club_id.clone()) {
            continue;
        }
        let Some(club) = slate
            .clubs
            .iter()
            .find(|club| club.code == choice.club_id || club.club_id == choice.club_id)
        else {
            continue;
        };
        if choice.option_id == PASS {
            remember_pass(game, slate, club, &choice.reason);
            continue;
        }
        if let Some(bid) = accept_option(game, slate, club, choice) {
            accepted.push(bid);
        }
    }
    accepted
}

fn remember_pass(game: &mut Game, slate: &DirectorSlate, club: &DirectorClubSlate, reason: &str) {
    remember(
        game,
        DirectorJournalEntry {
            date: slate.date.clone(),
            club_id: club.club_id.clone(),
            club_name: club.name.clone(),
            player_id: String::new(),
            player_name: String::new(),
            action: PASS.to_string(),
            reason: reason.to_string(),
        },
    );
}

fn accept_option(
    game: &mut Game,
    slate: &DirectorSlate,
    club: &DirectorClubSlate,
    choice: &DirectorChoice,
) -> Option<AcceptedBid> {
    let option = club
        .options
        .iter()
        .find(|option| option.id == choice.option_id)?;
    let TransferIntentOutcome::Accepted { fee } = apply_transfer_intent(
        game,
        TransferIntent::Bid {
            club_id: club.club_id.clone(),
            player_id: option.player_id.clone(),
            fee: option.fee,
        },
    ) else {
        return None;
    };
    let bid = AcceptedBid {
        date: slate.date.clone(),
        club_id: club.club_id.clone(),
        club_name: club.name.clone(),
        player_id: option.player_id.clone(),
        player_name: option.player_name.clone(),
        fee,
    };
    attach_director_note(game, &bid, &choice.text);
    crate::director_voice::sign_note(
        game,
        &option.player_id,
        &club.name,
        &option.player_name,
        &club.speaker_name,
        &club.speaker_role,
        &choice.text,
    );
    remember(
        game,
        DirectorJournalEntry {
            date: slate.date.clone(),
            club_id: club.club_id.clone(),
            club_name: club.name.clone(),
            player_id: option.player_id.clone(),
            player_name: option.player_name.clone(),
            action: "bid".to_string(),
            reason: choice.reason.clone(),
        },
    );
    Some(bid)
}

fn remember(game: &mut Game, entry: DirectorJournalEntry) {
    game.director_journal.push(entry);
    let overflow = game.director_journal.len().saturating_sub(JOURNAL_LIMIT);
    if overflow > 0 {
        game.director_journal.drain(0..overflow);
    }
}

fn json_payload(body: &str) -> &str {
    let trimmed = body.trim();
    let without_open = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed)
        .trim();
    without_open
        .strip_suffix("```")
        .unwrap_or(without_open)
        .trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::GameClock;
    use crate::game::{DirectorJournalEntry, Game};
    use chrono::{TimeZone, Utc};
    use domain::manager::Manager;
    use domain::player::{Player, PlayerAttributes, Position};
    use domain::team::Team;

    fn sample_attributes() -> PlayerAttributes {
        PlayerAttributes {
            pace: 68,
            stamina: 66,
            strength: 64,
            agility: 67,
            passing: 65,
            shooting: 72,
            tackling: 38,
            dribbling: 69,
            defending: 35,
            positioning: 66,
            vision: 63,
            decisions: 61,
            composure: 62,
            aggression: 48,
            teamwork: 58,
            leadership: 44,
            handling: 12,
            reflexes: 14,
            aerial: 40,
        }
    }

    fn make_team(id: &str, name: &str, reputation: u32) -> Team {
        let mut team = Team::new(
            id.to_string(),
            name.to_string(),
            name[..3].to_string(),
            "England".to_string(),
            "Testville".to_string(),
            format!("{name} Ground"),
            25_000,
        );
        team.reputation = reputation;
        team.finance = 5_000_000;
        team.transfer_budget = 5_000_000;
        team.wage_budget = 2_000_000;
        team
    }

    fn game() -> Game {
        let clock = GameClock::new(Utc.with_ymd_and_hms(2026, 1, 12, 12, 0, 0).unwrap());
        let mut manager = Manager::new(
            "mgr-user".to_string(),
            "Alex".to_string(),
            "Boss".to_string(),
            "1980-01-01".to_string(),
            "England".to_string(),
        );
        manager.hire("team1".to_string());
        let mut player = Player::new(
            "player-award".to_string(),
            "Golden".to_string(),
            "Golden Boot".to_string(),
            "1998-04-01".to_string(),
            "England".to_string(),
            Position::Forward,
            sample_attributes(),
        );
        player.team_id = Some("team1".to_string());
        player.market_value = 600_000;
        player.stage_wage(18_000);
        player.morale = 58;
        player.stage_contract_end(Some("2027-06-30".to_string()));
        let mut game = Game::new(
            clock,
            manager,
            vec![
                make_team("team1", "Alpha FC", 620),
                make_team("team2", "Beta FC", 690),
                make_team("team3", "Gamma FC", 500),
            ],
            vec![player],
            vec![],
            vec![],
        );
        game.season_context.transfer_window.status = TransferWindowStatus::Open;
        game
    }

    #[test]
    fn spotlight_names_other_clubs_and_the_users_player() {
        let brief = spotlight_brief(&game()).expect("window is open");
        assert_eq!(brief.date, "2026-01-12");
        assert!(brief.clubs.iter().all(|club| club.id != "team1"));
        assert_eq!(brief.clubs[0].id, "team2");
        assert_eq!(brief.players[0].id, "player-award");
    }

    #[test]
    fn a_shut_window_has_no_brief() {
        let mut game = game();
        game.season_context.transfer_window.status = TransferWindowStatus::Closed;
        assert!(spotlight_brief(&game).is_none());
    }

    #[test]
    fn messy_model_text_becomes_only_well_formed_intents() {
        let body = r#"```json
        {"intents":[
            {"kind":"bid","club_id":"team2","player_id":"player-award","fee":250000},
            {"kind":"bid","club_id":"team3","player_id":"player-award"},
            {"kind":"pass","club_id":"nope","player_id":"nobody"},
            {"kind":"poem","club_id":"team2","player_id":"player-award","fee":1}
        ]}
        ```"#;
        assert_eq!(
            parse_transfer_intents(body),
            vec![
                TransferIntent::Bid {
                    club_id: "team2".into(),
                    player_id: "player-award".into(),
                    fee: 250_000,
                },
                TransferIntent::Pass {
                    club_id: "nope".into(),
                    player_id: "nobody".into(),
                },
            ]
        );
        assert!(parse_transfer_intents("not json at all").is_empty());
    }

    #[test]
    fn an_accepted_model_bid_keeps_its_fee_and_the_note_cannot_rewrite_it() {
        let mut game = game();
        let accepted = apply_director_intents(
            &mut game,
            parse_transfer_intents(
                r#"{"intents":[{"kind":"bid","club_id":"team2","player_id":"player-award","fee":250000}]}"#,
            ),
        );
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].fee, 250_000);
        assert!(attach_director_note(
            &mut game,
            &accepted[0],
            "Beta fancy a forward who scores when it matters."
        ));
        assert_eq!(game.players[0].transfer_offers[0].fee, 250_000);
        let message = game
            .messages
            .iter()
            .find(|message| message.id == "transfer_interest_player-award")
            .expect("digest");
        assert!(message.body_key.is_none());
        assert!(message.body.contains("Beta fancy a forward"));
        let fee =
            crate::currency::format_compact_money(250_000, crate::currency::DEFAULT_CURRENCY_CODE)
                .expect("fee");
        assert!(message.body.contains(&fee));
        assert!(!attach_director_note(
            &mut game,
            &accepted[0],
            "They will pay 9000000."
        ));
        assert_eq!(game.players[0].transfer_offers[0].fee, 250_000);
    }

    fn with_fringe(mut game: Game) -> Game {
        let mut fringe = Player::new(
            "player-fringe".to_string(),
            "Lorenzo".to_string(),
            "Lorenzo Costa".to_string(),
            "1999-02-02".to_string(),
            "England".to_string(),
            Position::Forward,
            sample_attributes(),
        );
        fringe.team_id = Some("team1".to_string());
        fringe.market_value = 700_000;
        fringe.stage_wage(8_000);
        fringe.morale = 70;
        fringe.stage_contract_end(Some("2028-06-30".to_string()));
        game.players.push(fringe);
        let buyer = game
            .teams
            .iter_mut()
            .find(|team| team.id == "team2")
            .expect("buyer");
        buyer.transfer_budget = 1_000_000;
        buyer.finance = 5_000_000;
        game.players
            .iter_mut()
            .find(|player| player.id == "player-award")
            .expect("star")
            .market_value = 3_500_000;
        game
    }

    /// Given a star the buyer cannot afford and a squad player they can, when the slate is built, then only the affordable player is an option.
    #[test]
    fn a_slate_names_the_player_the_club_can_pay() {
        let slate = director_slate(&with_fringe(game())).expect("window is open");
        let beta = slate
            .clubs
            .iter()
            .find(|club| club.club_id == "team2")
            .expect("beta can pay someone");
        assert!(
            beta.options
                .iter()
                .all(|option| option.player_id == "player-fringe")
        );
        assert_eq!(beta.options[0].fee, 700_000);
        assert!(!beta.options[0].id.contains("player-award"));
    }

    /// Given a shut window, when the morning slate is asked for, then there is nothing to send the model.
    #[test]
    fn a_shut_window_has_no_slate() {
        let mut game = with_fringe(game());
        game.season_context.transfer_window.status = TransferWindowStatus::Closed;
        assert!(director_slate(&game).is_none());
    }

    /// Given a slate option, when the model picks it and names a fee of its own, then the offer stores the slate fee and the sentence.
    #[test]
    fn choosing_a_slate_option_writes_that_fee_and_the_sentence() {
        let mut game = with_fringe(game());
        let slate = director_slate(&game).expect("slate");
        let option = &slate.clubs[0].options[0];
        let accepted = apply_director_choices(
            &mut game,
            &slate,
            &[DirectorChoice {
                club_id: slate.clubs[0].club_id.clone(),
                option_id: option.id.clone(),
                reason: "squad_need".to_string(),
                text: "Beta need a forward who can lead the line.".to_string(),
            }],
        );
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].fee, option.fee);
        let offer = &game
            .players
            .iter()
            .find(|player| player.id == option.player_id)
            .expect("player")
            .transfer_offers[0];
        assert_eq!(offer.fee, option.fee);
        let message = game
            .messages
            .iter()
            .find(|message| message.id == format!("transfer_interest_{}", option.player_id))
            .expect("digest");
        assert!(message.body.contains("Beta need a forward"));
        assert_eq!(game.director_journal[0].action, "bid");
        assert_eq!(game.director_journal[0].player_name, "Lorenzo Costa");
    }

    /// Given a pick for an option that was not on the slate, when it is applied, then the save is unchanged.
    #[test]
    fn an_unknown_option_writes_nothing() {
        let mut game = with_fringe(game());
        let slate = director_slate(&game).expect("slate");
        let accepted = apply_director_choices(
            &mut game,
            &slate,
            &[DirectorChoice {
                club_id: "team2".to_string(),
                option_id: "team2:player-award".to_string(),
                reason: "ambition".to_string(),
                text: "A star signing.".to_string(),
            }],
        );
        assert!(accepted.is_empty());
        assert!(
            game.players
                .iter()
                .all(|player| player.transfer_offers.is_empty())
        );
        assert!(game.director_journal.is_empty());
    }

    /// Given a pass, when the ordinary market then runs, then that club does not also open an inbox offer.
    #[test]
    fn a_pass_keeps_that_club_out_of_the_inbox() {
        let mut quiet = listed_target(game());
        let slate = director_slate(&quiet).expect("slate");
        apply_director_choices(
            &mut quiet,
            &slate,
            &[DirectorChoice {
                club_id: "team2".to_string(),
                option_id: "pass".to_string(),
                reason: "squad_need".to_string(),
                text: String::new(),
            }],
        );
        assert!(quiet.players[0].transfer_offers.is_empty());
        crate::transfers::evaluate_transfer_market(&mut quiet);
        assert!(
            quiet.players[0]
                .transfer_offers
                .iter()
                .all(|offer| offer.from_team_id != "team2"),
            "a pass is the club's move for the day"
        );

        let mut spoken = listed_target(game());
        crate::transfers::evaluate_transfer_market(&mut spoken);
        assert!(
            spoken.players[0]
                .transfer_offers
                .iter()
                .any(|offer| offer.from_team_id == "team2"),
            "without a pass the market still bids"
        );
    }

    fn listed_target(mut game: Game) -> Game {
        let player = &mut game.players[0];
        player.transfer_listed = true;
        player.market_value = 600_000;
        player.stage_contract_end(Some("2026-04-01".to_string()));
        game
    }

    /// Given yesterday's bid is in the journal, when today's slate is built, then that decision is in the memory the model sees.
    #[test]
    fn the_next_slate_repeats_yesterdays_decision() {
        let mut game = with_fringe(game());
        game.director_journal.push(DirectorJournalEntry {
            date: "2026-01-11".to_string(),
            club_id: "team2".to_string(),
            club_name: "Beta FC".to_string(),
            player_id: "player-fringe".to_string(),
            player_name: "Lorenzo Costa".to_string(),
            action: "bid".to_string(),
            reason: "squad_need".to_string(),
        });
        let slate = director_slate(&game).expect("slate");
        assert_eq!(slate.memory.len(), 1);
        assert_eq!(slate.memory[0].player_name, "Lorenzo Costa");
        assert_eq!(slate.memory[0].action, "bid");
    }

    /// Given the next opponent can pay and a richer club can too, when the slate is ordered, then the opponent is asked first.
    #[test]
    fn the_next_opponent_is_asked_before_a_richer_club() {
        let mut game = with_fringe(game());
        let gamma = game
            .teams
            .iter_mut()
            .find(|team| team.id == "team3")
            .expect("gamma");
        gamma.transfer_budget = 1_000_000;
        gamma.finance = 5_000_000;
        let mut league = domain::league::League::new(
            "league".to_string(),
            "Test League".to_string(),
            2026,
            &[
                "team1".to_string(),
                "team2".to_string(),
                "team3".to_string(),
            ],
        );
        let mut fixture = domain::league::Fixture::default();
        fixture.date = "2026-01-20".to_string();
        fixture.home_team_id = "team1".to_string();
        fixture.away_team_id = "team3".to_string();
        fixture.status = domain::league::FixtureStatus::Scheduled;
        league.fixtures.push(fixture);
        game.competitions.push(league);
        let slate = director_slate(&game).expect("slate");
        assert_eq!(slate.clubs[0].club_id, "team3");
    }

    /// Given a club and a player with long ids, when the slate is shown to the model, then the ids it must echo are two characters.
    #[test]
    fn the_model_echoes_a_short_option_id() {
        let slate = director_slate(&with_fringe(game())).expect("slate");
        let beta = slate
            .clubs
            .iter()
            .find(|club| club.club_id == "team2")
            .expect("beta");
        assert_eq!(beta.code.len(), 1);
        assert_eq!(beta.options[0].id.len(), 2);
        let prompt = choice_user_prompt(&slate);
        assert!(!prompt.contains("player-fringe"));
        assert!(!prompt.contains("team2"));
        assert!(prompt.contains(&beta.options[0].id));
    }

    /// Given a reply cut off inside the third club, when it is parsed, then the two finished choices remain.
    #[test]
    fn a_cut_off_reply_keeps_the_choices_that_finished() {
        let body = r#"{"choices":[
            {"club_id":"a","option_id":"a1","reason":"squad_need","text":"Need a forward."},
            {"club_id":"b","option_id":"b1","reason":"rivalry","text":"This helps against a rival."},
            {"club_id":"c","option_id":"c1","reason":"ambition","text":"Lorenzo Cos"#;
        let choices = parse_director_choices(body);
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[0].option_id, "a1");
        assert_eq!(choices[1].club_id, "b");
    }
}
