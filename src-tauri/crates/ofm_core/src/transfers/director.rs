//! One other club's transfer intent, judged by the same rules as the daily market.
//!
//! The caller proposes a bid or a pass. A pass is a decision not to act and writes
//! nothing. A bid becomes the incoming offer the inbox already shows, at the fee
//! that was proposed, or it is refused and the save is left untouched. Refusal is
//! a result, not a failure of the caller.

use super::*;
use crate::contract_wage_policy::BuyerWageFacts;
use crate::squad_floor::group_index;

/// What another club wants to do about one player.
///
/// `kind` is the discriminator a caller sends. The fee on a bid is the fee Rust
/// will store if it accepts; nothing here replaces it with a suggested price.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransferIntent {
    Bid {
        club_id: String,
        player_id: String,
        fee: u64,
    },
    Pass {
        club_id: String,
        player_id: String,
    },
}

/// Why a bid was not applied. The save is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferIntentRejection {
    WindowClosed,
    NoUserClub,
    ClubNotFound,
    OwnClub,
    PlayerNotFound,
    NotYourPlayer,
    PlayerUnavailable,
    AlreadyInTalks,
    CoolingOff,
    QueueFull,
    Funds,
    TransferBudget,
    Wage,
    Reputation,
    SquadFull,
}

/// What Rust did with an intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TransferIntentOutcome {
    Accepted { fee: u64 },
    Rejected { reason: TransferIntentRejection },
    Passed,
}

/// Apply one intent. A pass returns [`TransferIntentOutcome::Passed`] and does
/// not read the world, so a decision not to act cannot be turned into a write
/// by a stale id. A bid is checked in full before any offer or message is created.
pub fn apply_transfer_intent(game: &mut Game, intent: TransferIntent) -> TransferIntentOutcome {
    match intent {
        TransferIntent::Pass { .. } => TransferIntentOutcome::Passed,
        TransferIntent::Bid {
            club_id,
            player_id,
            fee,
        } => apply_bid(game, &club_id, &player_id, fee),
    }
}

fn rejected(reason: TransferIntentRejection) -> TransferIntentOutcome {
    TransferIntentOutcome::Rejected { reason }
}

fn apply_bid(game: &mut Game, club_id: &str, player_id: &str, fee: u64) -> TransferIntentOutcome {
    let Some(user_team_id) = game.manager.team_id.clone() else {
        return rejected(TransferIntentRejection::NoUserClub);
    };
    if let Some(reason) = bid_rejection(game, club_id, player_id, fee) {
        return rejected(reason);
    }
    let buyer_name = game
        .teams
        .iter()
        .find(|team| team.id == club_id)
        .map(|team| team.name.clone())
        .unwrap_or_default();
    let today_text = game.clock.current_date.format("%Y-%m-%d").to_string();
    create_incoming_user_offer(
        game,
        &MarketCandidate {
            player_id: player_id.to_string(),
            owner_team_id: user_team_id,
            score: 0,
            fee,
        },
        club_id,
        &buyer_name,
        &today_text,
    );
    TransferIntentOutcome::Accepted { fee }
}

/// Why this fee would not become an inbox offer. `None` means [`apply_bid`] would write it.
fn bid_rejection(
    game: &Game,
    club_id: &str,
    player_id: &str,
    fee: u64,
) -> Option<TransferIntentRejection> {
    if !transfer_window_is_open(game) {
        return Some(TransferIntentRejection::WindowClosed);
    }
    let Some(user_team_id) = game.manager.team_id.as_deref() else {
        return Some(TransferIntentRejection::NoUserClub);
    };
    if club_id == user_team_id {
        return Some(TransferIntentRejection::OwnClub);
    }
    let Some(buyer) = game.teams.iter().find(|team| team.id == club_id) else {
        return Some(TransferIntentRejection::ClubNotFound);
    };
    let today = game.clock.current_date.date_naive();
    let Some(player) = game.players.iter().find(|player| player.id == player_id) else {
        return Some(TransferIntentRejection::PlayerNotFound);
    };
    if player.team_id.as_deref() != Some(user_team_id) {
        return Some(TransferIntentRejection::NotYourPlayer);
    }
    if player_has_pending_registration(player) {
        return Some(TransferIntentRejection::PlayerUnavailable);
    }
    let approaches = pending_approach_clubs(player);
    if approaches.contains(club_id) {
        return Some(TransferIntentRejection::AlreadyInTalks);
    }
    if clubs_in_rebid_cooldown(player, today).contains(club_id) {
        return Some(TransferIntentRejection::CoolingOff);
    }
    if approaches.len() >= MAX_PENDING_INCOMING_OFFERS_PER_USER_PLAYER {
        return Some(TransferIntentRejection::QueueFull);
    }
    let Ok(fee_i64) = i64::try_from(fee) else {
        return Some(TransferIntentRejection::Funds);
    };
    if buyer.finance < fee_i64 {
        return Some(TransferIntentRejection::Funds);
    }
    if buyer.transfer_budget < fee_i64 {
        return Some(TransferIntentRejection::TransferBudget);
    }
    let depth = squad_position_depths(game)
        .get(club_id)
        .copied()
        .unwrap_or([0; 4])[group_index(&player.position)];
    let wage_facts = BuyerWageFacts::of(game, club_id);
    if !buyer_can_pay_standard_wage(player, buyer, &wage_facts, today) {
        return Some(TransferIntentRejection::Wage);
    }
    let owner_reputation = game
        .teams
        .iter()
        .find(|team| team.id == user_team_id)
        .map(|team| team.reputation)
        .unwrap_or(0);
    if owner_reputation as i32 - buyer.reputation as i32 > MAX_BUYER_REPUTATION_DEFICIT {
        return Some(TransferIntentRejection::Reputation);
    }
    if depth >= POSITION_GROUP_SURPLUS_THRESHOLD {
        return Some(TransferIntentRejection::SquadFull);
    }
    None
}

/// A user player this club could bid for today, at the fee the market would write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AffordableTarget {
    pub player_id: String,
    pub player_name: String,
    pub position: String,
    pub fee: u64,
    pub squad_depth: usize,
    pub contract_end: Option<String>,
    pub score: i32,
}

/// Targets [`apply_bid`] would accept at [`suggested_incoming_fee`], highest appeal first.
pub(crate) fn affordable_user_targets(game: &Game, club_id: &str) -> Vec<AffordableTarget> {
    let Some(user_team_id) = game.manager.team_id.as_deref() else {
        return Vec::new();
    };
    let today = game.clock.current_date.date_naive();
    let mut targets: Vec<AffordableTarget> = game
        .players
        .iter()
        .filter(|player| player.team_id.as_deref() == Some(user_team_id))
        .filter_map(|player| affordable_target(game, club_id, player, today))
        .collect();
    targets.sort_by_key(|target| std::cmp::Reverse(target.score));
    targets
}

fn affordable_target(
    game: &Game,
    club_id: &str,
    player: &domain::player::Player,
    today: chrono::NaiveDate,
) -> Option<AffordableTarget> {
    let fee = suggested_incoming_fee(today, player);
    if bid_rejection(game, club_id, &player.id, fee).is_some() {
        return None;
    }
    let squad_depth = squad_position_depths(game)
        .get(club_id)
        .copied()
        .unwrap_or([0; 4])[group_index(&player.position)];
    Some(AffordableTarget {
        player_id: player.id.clone(),
        player_name: player.full_name.clone(),
        position: format!("{:?}", player.position),
        fee,
        squad_depth,
        contract_end: player.contract_end().map(str::to_string),
        score: incoming_interest_score(today, player),
    })
}
