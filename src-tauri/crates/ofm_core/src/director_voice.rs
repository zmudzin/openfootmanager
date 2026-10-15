//! The one sentence the morning director may add beside the transfer picks.
//!
//! Rust already knows the fact: a bid in the journal, yesterday's league score,
//! or a contract letter the day is about to send. The model writes the words,
//! in the game's language, in the voice of a person the save already names.

use chrono::{Datelike, Duration, NaiveDate};
use domain::league::{Fixture, FixtureCompetition, FixtureStatus};
use domain::message::InboxMessage;
use domain::news::{NewsArticle, NewsCategory};
use domain::player::Position;
use domain::staff::StaffRole;

use crate::contracts::{self, ContractWarningStage};
use crate::director::{DirectorSlate, VoiceTask, clean_tone};
use crate::game::{DirectorVoice, Game};
use crate::inbox;

const PAPER: &str = "paper";
const MATCH: &str = "match";
const LETTER: &str = "letter";
const MANAGER: &str = "manager";
const ASSISTANT: &str = "assistant";

pub fn language_name(code: &str) -> &'static str {
    match code {
        "es" => "Spanish",
        "pt" => "Portuguese",
        "pt-BR" => "Brazilian Portuguese",
        "fr" => "French",
        "de" => "German",
        "it" => "Italian",
        "ru" => "Russian",
        "zh-CN" => "Simplified Chinese",
        "cs" => "Czech",
        "tr" => "Turkish",
        "id" => "Indonesian",
        _ => "English",
    }
}

/// Transfer slate when the window is open, plus at most one voice task.
/// `None` when there is nothing to ask.
pub fn morning_slate(game: &Game, language_code: &str) -> Option<DirectorSlate> {
    let mut slate = crate::director::director_slate(game).unwrap_or(DirectorSlate {
        date: game.clock.current_date.format("%Y-%m-%d").to_string(),
        memory: Vec::new(),
        clubs: Vec::new(),
        language: String::new(),
        voice: None,
    });
    slate.language = language_name(language_code).to_string();
    slate.voice = voice_task(game);
    if slate.clubs.is_empty() && slate.voice.is_none() {
        return None;
    }
    Some(slate)
}

pub(crate) fn club_speaker(game: &Game, team_id: &str) -> (String, String) {
    let manager = game
        .managers
        .iter()
        .find(|manager| manager.team_id.as_deref() == Some(team_id))
        .or_else(|| (game.manager.team_id.as_deref() == Some(team_id)).then_some(&game.manager));
    if let Some(manager) = manager {
        let role = if manager.interim { ASSISTANT } else { MANAGER };
        return (manager.full_name(), role.to_string());
    }
    let assistant = game.staff.iter().find(|member| {
        member.team_id.as_deref() == Some(team_id) && member.role == StaffRole::AssistantManager
    });
    assistant
        .map(|member| (member.display_name.clone(), ASSISTANT.to_string()))
        .unwrap_or_default()
}

fn voice_task(game: &Game) -> Option<VoiceTask> {
    paper_task(game)
        .or_else(|| match_task(game))
        .or_else(|| letter_task(game))
}

fn paper_task(game: &Game) -> Option<VoiceTask> {
    if game.clock.current_date.weekday().num_days_from_monday() != 0 {
        return None;
    }
    let today = game.clock.current_date.date_naive();
    let entry = game.director_journal.iter().rev().find(|entry| {
        entry.action == "bid"
            && days_since(today, &entry.date).is_some_and(|days| (0..7).contains(&days))
    })?;
    let (speaker_name, speaker_role) = club_speaker(game, &entry.club_id);
    let position = game
        .players
        .iter()
        .find(|player| player.id == entry.player_id)
        .map(|player| position_word(&player.position))
        .unwrap_or("player");
    Some(VoiceTask {
        kind: PAPER.to_string(),
        speaker_name,
        speaker_role,
        club_id: entry.club_id.clone(),
        club_name: entry.club_name.clone(),
        player_id: entry.player_id.clone(),
        player_name: entry.player_name.clone(),
        facts: format!(
            "{} have asked about {}, a {position}.",
            entry.club_name, entry.player_name
        ),
        home_name: String::new(),
        away_name: String::new(),
        home_goals: 0,
        away_goals: 0,
    })
}

fn match_task(game: &Game) -> Option<VoiceTask> {
    let user_team_id = game.manager.team_id.as_deref()?;
    let yesterday = (game.clock.current_date.date_naive() - Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    let fixture = league_fixtures(game).into_iter().find(|fixture| {
        fixture.date == yesterday
            && fixture.status == FixtureStatus::Completed
            && fixture.result.is_some()
            && (fixture.home_team_id == user_team_id || fixture.away_team_id == user_team_id)
    })?;
    let result = fixture.result.as_ref()?;
    let opponent_id = if fixture.home_team_id == user_team_id {
        fixture.away_team_id.as_str()
    } else {
        fixture.home_team_id.as_str()
    };
    let (speaker_name, speaker_role) = club_speaker(game, opponent_id);
    let opponent_name = team_name(game, opponent_id);
    let user_name = team_name(game, user_team_id);
    let at_home = fixture.home_team_id == opponent_id;
    let opponent_goals = if at_home {
        result.home_goals
    } else {
        result.away_goals
    };
    let user_goals = if at_home {
        result.away_goals
    } else {
        result.home_goals
    };
    let outcome = match opponent_goals.cmp(&user_goals) {
        std::cmp::Ordering::Greater => "won",
        std::cmp::Ordering::Less => "lost",
        std::cmp::Ordering::Equal => "drew",
    };
    let place = if at_home { "at home" } else { "away" };
    Some(VoiceTask {
        kind: MATCH.to_string(),
        speaker_name,
        speaker_role,
        club_id: opponent_id.to_string(),
        club_name: opponent_name.clone(),
        player_id: String::new(),
        player_name: String::new(),
        facts: format!("{opponent_name} were {place} against {user_name} and {outcome}."),
        home_name: team_name(game, &fixture.home_team_id),
        away_name: team_name(game, &fixture.away_team_id),
        home_goals: result.home_goals,
        away_goals: result.away_goals,
    })
}

fn letter_task(game: &Game) -> Option<VoiceTask> {
    let user_team_id = game.manager.team_id.as_deref()?;
    let today = game.clock.current_date.date_naive();
    // The player's own manager is the user. The letter already comes from the
    // assistant, so the named voice is that assistant, not the user.
    let speaker_name = game
        .staff
        .iter()
        .find(|member| {
            member.team_id.as_deref() == Some(user_team_id)
                && member.role == StaffRole::AssistantManager
        })
        .map(|member| member.display_name.clone())
        .filter(|name| !name.is_empty())?;
    let player = game.players.iter().find(|player| {
        player.team_id.as_deref() == Some(user_team_id) && letter_is_due(game, player, today)
    })?;
    Some(VoiceTask {
        kind: LETTER.to_string(),
        speaker_name,
        speaker_role: ASSISTANT.to_string(),
        club_id: user_team_id.to_string(),
        club_name: team_name(game, user_team_id),
        player_id: player.id.clone(),
        player_name: player.match_name.clone(),
        facts: format!(
            "The assistant is talking to the manager about {}, whose contract is inside its final months.",
            player.match_name
        ),
        home_name: String::new(),
        away_name: String::new(),
        home_goals: 0,
        away_goals: 0,
    })
}

fn letter_is_due(game: &Game, player: &domain::player::Player, today: NaiveDate) -> bool {
    if contracts::has_let_expire_intent(player) {
        return false;
    }
    let Some(stage) = contracts::contract_warning_stage(player.contract_end(), today) else {
        return false;
    };
    if matches!(stage, ContractWarningStage::TwelveMonths) {
        return false;
    }
    let contract_end = player.contract_end().unwrap_or("");
    let id = format!(
        "contract_concern_{}_{}_{}",
        player.id,
        contract_end,
        stage.message_suffix()
    );
    !inbox::already_emitted(game, &id)
}

pub fn remember_voice(game: &mut Game, slate: &DirectorSlate, text: &str) {
    let Some(task) = &slate.voice else {
        return;
    };
    if game.clock.current_date.format("%Y-%m-%d").to_string() != slate.date {
        return;
    }
    let Some(text) = clean_tone(text) else {
        return;
    };
    game.director_voice = Some(DirectorVoice {
        date: slate.date.clone(),
        kind: task.kind.clone(),
        speaker_name: task.speaker_name.clone(),
        speaker_role: task.speaker_role.clone(),
        club_id: task.club_id.clone(),
        club_name: task.club_name.clone(),
        text,
        player_id: task.player_id.clone(),
        player_name: task.player_name.clone(),
        home_name: task.home_name.clone(),
        away_name: task.away_name.clone(),
        home_goals: task.home_goals,
        away_goals: task.away_goals,
    });
}

pub(crate) fn sign_note(
    game: &mut Game,
    player_id: &str,
    club_name: &str,
    player_name: &str,
    speaker_name: &str,
    speaker_role: &str,
    tone: &str,
) {
    if speaker_name.is_empty() {
        return;
    }
    let Some(tone) = clean_tone(tone) else {
        return;
    };
    let message_id = format!("transfer_interest_{player_id}");
    let Some(message) = game
        .messages
        .iter_mut()
        .find(|message| message.id == message_id)
    else {
        return;
    };
    if !message.body.contains(&tone) {
        return;
    }
    let (role_key, body_key, role_word) = if speaker_role == ASSISTANT {
        (
            "be.role.assistantManager",
            "be.msg.transferInterest.directorAssistant",
            "assistant manager",
        )
    } else {
        (
            "be.role.manager",
            "be.msg.transferInterest.directorManager",
            "manager",
        )
    };
    message.sender = speaker_name.to_string();
    message.sender_key = None;
    message.sender_role_key = Some(role_key.to_string());
    message.body_key = Some(body_key.to_string());
    message
        .i18n_params
        .insert("speaker".to_string(), speaker_name.to_string());
    message
        .i18n_params
        .insert("quote".to_string(), tone.clone());
    message
        .i18n_params
        .insert("team".to_string(), club_name.to_string());
    message
        .i18n_params
        .insert("player".to_string(), player_name.to_string());
    message.body = format!(
        "{speaker_name}, {role_word} of {club_name}\n\n{tone}\n\n{club_name} bid for {player_name}."
    );
}

/// True when this Monday's paper should report the real bid instead of a rumour.
pub(crate) fn paper_pending(game: &Game) -> bool {
    let today = game.clock.current_date.format("%Y-%m-%d").to_string();
    game.director_voice
        .as_ref()
        .is_some_and(|voice| voice.kind == PAPER && voice.date == today)
}

pub fn publish_director_voice(game: &mut Game) {
    let today = game.clock.current_date.format("%Y-%m-%d").to_string();
    let Some(voice) = game.director_voice.clone() else {
        return;
    };
    if voice.date != today {
        return;
    }
    match voice.kind.as_str() {
        PAPER => publish_paper(game, &voice),
        MATCH => publish_match(game, &voice),
        LETTER => publish_letter(game, &voice),
        _ => {}
    }
    game.director_voice = None;
}

fn publish_paper(game: &mut Game, voice: &DirectorVoice) {
    let id = format!("director_paper_{}", voice.date);
    if game.news.iter().any(|article| article.id == id) {
        return;
    }
    let date = game.clock.current_date.to_rfc3339();
    game.news.push(voice_article(
        id,
        "be.news.directorPaper.headline",
        voice,
        &[
            ("club", voice.club_name.as_str()),
            ("player", voice.player_name.as_str()),
        ],
        NewsCategory::TransferRumour,
        &date,
    ));
}

fn publish_match(game: &mut Game, voice: &DirectorVoice) {
    let id = format!("director_match_{}", voice.date);
    if game.news.iter().any(|article| article.id == id) {
        return;
    }
    let date = game.clock.current_date.to_rfc3339();
    game.news.push(voice_article(
        id,
        "be.news.directorMatch.headline",
        voice,
        &[
            ("speaker", voice.speaker_name.as_str()),
            ("home", voice.home_name.as_str()),
            ("away", voice.away_name.as_str()),
            ("homeGoals", &voice.home_goals.to_string()),
            ("awayGoals", &voice.away_goals.to_string()),
        ],
        NewsCategory::Editorial,
        &date,
    ));
}

fn voice_article(
    id: String,
    headline_key: &str,
    voice: &DirectorVoice,
    extra: &[(&str, &str)],
    category: NewsCategory,
    date: &str,
) -> NewsArticle {
    let body_key = if voice.speaker_name.is_empty() {
        "be.news.directorVoice.club"
    } else if voice.speaker_role == ASSISTANT {
        "be.news.directorVoice.assistant"
    } else {
        "be.news.directorVoice.manager"
    };
    let mut params = vec![
        ("speaker".to_string(), voice.speaker_name.clone()),
        ("club".to_string(), voice.club_name.clone()),
        ("quote".to_string(), voice.text.clone()),
    ];
    for (key, value) in extra {
        params.push(((*key).to_string(), (*value).to_string()));
    }
    let mut article = NewsArticle::new(
        id,
        String::new(),
        voice.text.clone(),
        String::new(),
        date.to_string(),
        category,
    )
    .with_i18n(
        headline_key,
        body_key,
        "be.source.matchDayPress",
        params.into_iter().collect(),
    );
    if !voice.club_id.is_empty() {
        article = article.with_teams(vec![voice.club_id.clone()]);
    }
    if !voice.player_id.is_empty() {
        article = article.with_players(vec![voice.player_id.clone()]);
    }
    article
}

fn publish_letter(game: &mut Game, voice: &DirectorVoice) {
    let prefix = format!("contract_concern_{}_", voice.player_id);
    let Some(message) = game
        .messages
        .iter_mut()
        .find(|message| message.id.starts_with(&prefix))
    else {
        return;
    };
    apply_letter(message, voice);
}

fn apply_letter(message: &mut InboxMessage, voice: &DirectorVoice) {
    message.body_key = Some("be.msg.contractConcernVoice.body".to_string());
    message
        .i18n_params
        .insert("quote".to_string(), voice.text.clone());
    message.body = voice.text.clone();
    if voice.speaker_name.is_empty() {
        return;
    }
    message.sender = voice.speaker_name.clone();
    message.sender_key = None;
    message.sender_role_key = Some(
        if voice.speaker_role == ASSISTANT {
            "be.role.assistantManager"
        } else {
            "be.role.manager"
        }
        .to_string(),
    );
}

fn league_fixtures(game: &Game) -> Vec<&Fixture> {
    let mut fixtures = Vec::new();
    for league in game.league.iter().chain(
        game.competitions
            .iter()
            .filter(|competition| competition.kind == domain::league::CompetitionType::League),
    ) {
        for fixture in &league.fixtures {
            if fixture.competition == FixtureCompetition::League {
                fixtures.push(fixture);
            }
        }
    }
    fixtures
}

fn team_name(game: &Game, team_id: &str) -> String {
    game.teams
        .iter()
        .find(|team| team.id == team_id)
        .map(|team| team.name.clone())
        .unwrap_or_default()
}

fn days_since(today: NaiveDate, then: &str) -> Option<i64> {
    let then = NaiveDate::parse_from_str(then, "%Y-%m-%d").ok()?;
    Some((today - then).num_days())
}

fn position_word(position: &Position) -> &'static str {
    match position {
        Position::Goalkeeper => "goalkeeper",
        Position::Defender
        | Position::RightBack
        | Position::CenterBack
        | Position::LeftBack
        | Position::RightWingBack
        | Position::LeftWingBack => "defender",
        Position::Midfielder
        | Position::DefensiveMidfielder
        | Position::CentralMidfielder
        | Position::AttackingMidfielder
        | Position::RightMidfielder
        | Position::LeftMidfielder => "midfielder",
        Position::Forward | Position::RightWinger | Position::LeftWinger | Position::Striker => {
            "forward"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::GameClock;
    use crate::director::{DirectorChoice, apply_director_choices, choice_user_prompt};
    use crate::game::DirectorJournalEntry;
    use chrono::{TimeZone, Utc};
    use domain::league::{Fixture, League, MatchResult};
    use domain::manager::Manager;
    use domain::player::{Player, PlayerAttributes, Position};
    use domain::staff::{Staff, StaffAttributes, StaffRole};
    use domain::team::Team;

    fn attributes() -> PlayerAttributes {
        PlayerAttributes {
            pace: 60,
            stamina: 60,
            strength: 60,
            agility: 60,
            passing: 60,
            shooting: 60,
            tackling: 60,
            dribbling: 60,
            defending: 60,
            positioning: 60,
            vision: 60,
            decisions: 60,
            composure: 60,
            aggression: 40,
            teamwork: 60,
            leadership: 40,
            handling: 10,
            reflexes: 10,
            aerial: 40,
        }
    }

    fn team(id: &str, name: &str) -> Team {
        let mut team = Team::new(
            id.to_string(),
            name.to_string(),
            name[..3].to_string(),
            "England".to_string(),
            "Testville".to_string(),
            format!("{name} Ground"),
            20_000,
        );
        team.reputation = 600;
        team.finance = 5_000_000;
        team.transfer_budget = 2_000_000;
        team.wage_budget = 1_000_000;
        team
    }

    fn manager_of(id: &str, first: &str, last: &str, team_id: &str) -> Manager {
        let mut manager = Manager::new(
            id.to_string(),
            first.to_string(),
            last.to_string(),
            "1975-05-05".to_string(),
            "Italy".to_string(),
        );
        manager.team_id = Some(team_id.to_string());
        manager
    }

    fn user_game() -> Game {
        let clock = GameClock::new(Utc.with_ymd_and_hms(2026, 1, 13, 12, 0, 0).unwrap());
        let mut user = Manager::new(
            "mgr-user".to_string(),
            "Alex".to_string(),
            "Boss".to_string(),
            "1980-01-01".to_string(),
            "England".to_string(),
        );
        user.hire("team1".to_string());
        let mut player = Player::new(
            "player-1".to_string(),
            "Lorenzo".to_string(),
            "Lorenzo Costa".to_string(),
            "1998-04-01".to_string(),
            "England".to_string(),
            Position::Forward,
            attributes(),
        );
        player.team_id = Some("team1".to_string());
        player.market_value = 400_000;
        player.stage_wage(8_000);
        player.stage_contract_end(Some("2028-06-30".to_string()));
        let mut game = Game::new(
            clock,
            user,
            vec![team("team1", "Alpha FC"), team("team2", "Beta FC")],
            vec![player],
            vec![],
            vec![],
        );
        game.managers
            .push(manager_of("mgr-beta", "Carlo", "Rossi", "team2"));
        game.season_context.transfer_window.status = domain::season::TransferWindowStatus::Closed;
        game
    }

    fn played_yesterday(game: &mut Game, home_goals: u8, away_goals: u8) {
        let mut fixture = Fixture::default();
        fixture.id = "fix-1".to_string();
        fixture.date = "2026-01-12".to_string();
        fixture.home_team_id = "team2".to_string();
        fixture.away_team_id = "team1".to_string();
        fixture.competition = FixtureCompetition::League;
        fixture.status = FixtureStatus::Completed;
        fixture.result = Some(MatchResult {
            home_goals,
            away_goals,
            ..MatchResult::default()
        });
        let mut league = League::new(
            "league".to_string(),
            "Test League".to_string(),
            2026,
            &["team1".to_string(), "team2".to_string()],
        );
        league.kind = domain::league::CompetitionType::League;
        league.fixtures.push(fixture);
        game.league = Some(league);
    }

    /// Given Spanish is the game language and Beta's manager is Carlo Rossi, when the morning prompt is built, then it names that language and that manager.
    #[test]
    fn the_prompt_names_the_manager_and_the_game_language() {
        let mut game = user_game();
        game.season_context.transfer_window.status = domain::season::TransferWindowStatus::Open;
        let slate = morning_slate(&game, "es").expect("something to ask");
        let prompt = choice_user_prompt(&slate);
        assert!(prompt.contains("Spanish"), "{prompt}");
        assert!(prompt.contains("Carlo Rossi"), "{prompt}");
        assert!(prompt.contains("\"speaker_role\":\"manager\""), "{prompt}");
    }

    /// Given Carlo Rossi manages the bidding club, when his pick is accepted, then the inbox names him as manager.
    #[test]
    fn a_bid_is_signed_by_that_manager() {
        let mut game = user_game();
        game.season_context.transfer_window.status = domain::season::TransferWindowStatus::Open;
        let slate = morning_slate(&game, "en").expect("slate");
        let club = slate
            .clubs
            .iter()
            .find(|club| club.club_id == "team2")
            .expect("beta");
        let option = &club.options[0];
        let text = "We are short of a forward who can hold the ball, and his contract gives us a reason to move this month.".to_string();
        apply_director_choices(
            &mut game,
            &slate,
            &[DirectorChoice {
                club_id: club.code.clone(),
                option_id: option.id.clone(),
                reason: "squad_need".to_string(),
                text: text.clone(),
            }],
        );
        let message = game
            .messages
            .iter()
            .find(|message| message.id == format!("transfer_interest_{}", option.player_id))
            .expect("digest");
        assert_eq!(message.sender, "Carlo Rossi");
        assert_eq!(message.sender_role_key.as_deref(), Some("be.role.manager"));
        assert_eq!(
            message.body_key.as_deref(),
            Some("be.msg.transferInterest.directorManager")
        );
        assert!(message.body.contains("Carlo Rossi"));
        assert_eq!(
            message.i18n_params.get("quote").map(String::as_str),
            Some(text.as_str())
        );
    }

    /// Given Beta beat Alpha yesterday, when the next morning is built with the window shut, then the voice is Carlo Rossi on that match.
    #[test]
    fn the_morning_after_a_league_match_asks_the_opponents_manager() {
        let mut game = user_game();
        played_yesterday(&mut game, 2, 0);
        let slate = morning_slate(&game, "en").expect("voice");
        assert!(slate.clubs.is_empty());
        let voice = slate.voice.expect("match");
        assert_eq!(voice.kind, "match");
        assert_eq!(voice.speaker_name, "Carlo Rossi");
        assert!(voice.facts.contains("won"));
        assert!(!voice.facts.chars().any(|ch| ch.is_ascii_digit()));
        assert_eq!(voice.home_goals, 2);
    }

    /// Given a bid already in the journal and a Monday, when the morning is built, then the paper is about that bid.
    #[test]
    fn a_monday_with_a_real_bid_asks_for_the_paper() {
        let mut game = user_game();
        game.clock = GameClock::new(Utc.with_ymd_and_hms(2026, 1, 12, 12, 0, 0).unwrap());
        game.director_journal.push(DirectorJournalEntry {
            date: "2026-01-10".to_string(),
            club_id: "team2".to_string(),
            club_name: "Beta FC".to_string(),
            player_id: "player-1".to_string(),
            player_name: "Lorenzo Costa".to_string(),
            action: "bid".to_string(),
            reason: "squad_need".to_string(),
        });
        let voice = morning_slate(&game, "de")
            .expect("paper")
            .voice
            .expect("task");
        assert_eq!(voice.kind, "paper");
        assert_eq!(voice.speaker_name, "Carlo Rossi");
        assert!(voice.facts.contains("Lorenzo Costa"));
        assert_eq!(
            morning_slate(&game, "de").expect("slate").language,
            "German"
        );
    }

    /// Given a stored paper voice, when the day publishes it, then the article quotes that manager about that player.
    #[test]
    fn publishing_the_paper_names_the_manager_and_the_player() {
        let mut game = user_game();
        game.clock = GameClock::new(Utc.with_ymd_and_hms(2026, 1, 12, 12, 0, 0).unwrap());
        game.director_voice = Some(DirectorVoice {
            date: "2026-01-12".to_string(),
            kind: "paper".to_string(),
            speaker_name: "Carlo Rossi".to_string(),
            speaker_role: "manager".to_string(),
            club_id: "team2".to_string(),
            club_name: "Beta FC".to_string(),
            text:
                "Costa gives us a forward we do not have, and we have been waiting for this window."
                    .to_string(),
            player_id: "player-1".to_string(),
            player_name: "Lorenzo Costa".to_string(),
            ..DirectorVoice::default()
        });
        assert!(paper_pending(&game));
        publish_director_voice(&mut game);
        let article = game
            .news
            .iter()
            .find(|article| article.id == "director_paper_2026-01-12")
            .expect("article");
        assert_eq!(
            article.headline_key.as_deref(),
            Some("be.news.directorPaper.headline")
        );
        assert_eq!(
            article.body_key.as_deref(),
            Some("be.news.directorVoice.manager")
        );
        assert_eq!(
            article.i18n_params.get("player").map(String::as_str),
            Some("Lorenzo Costa")
        );
        assert!(
            article
                .i18n_params
                .get("quote")
                .unwrap()
                .contains("forward")
        );
        assert!(game.director_voice.is_none());
    }

    /// Given yesterday's score, when the quote is published, then the headline carries that score and the quote has none.
    #[test]
    fn publishing_a_match_quote_keeps_the_score_in_the_headline() {
        let mut game = user_game();
        game.director_voice = Some(DirectorVoice {
            date: "2026-01-13".to_string(),
            kind: "match".to_string(),
            speaker_name: "Carlo Rossi".to_string(),
            speaker_role: "manager".to_string(),
            club_name: "Beta FC".to_string(),
            text: "We controlled the game at home and Alpha never found a way through.".to_string(),
            home_name: "Beta FC".to_string(),
            away_name: "Alpha FC".to_string(),
            home_goals: 2,
            away_goals: 0,
            ..DirectorVoice::default()
        });
        publish_director_voice(&mut game);
        let article = &game.news[0];
        assert_eq!(
            article.headline_key.as_deref(),
            Some("be.news.directorMatch.headline")
        );
        assert_eq!(
            article.i18n_params.get("homeGoals").map(String::as_str),
            Some("2")
        );
        assert!(
            !article
                .i18n_params
                .get("quote")
                .unwrap()
                .chars()
                .any(|ch| ch.is_ascii_digit())
        );
    }

    /// Given a named assistant and a contract letter, when the morning voice is published onto that letter, then the assistant's name is the sender.
    #[test]
    fn a_contract_letter_is_spoken_by_the_named_assistant() {
        let mut game = user_game();
        game.players[0].stage_contract_end(Some("2026-03-20".to_string()));
        let mut assistant = Staff::new(
            "staff-1".to_string(),
            "Marta".to_string(),
            "Keller".to_string(),
            "1982-02-02".to_string(),
            StaffRole::AssistantManager,
            StaffAttributes {
                coaching: 60,
                judging_ability: 60,
                judging_potential: 60,
                physiotherapy: 20,
            },
        );
        assistant.team_id = Some("team1".to_string());
        game.staff.push(assistant);
        let voice = morning_slate(&game, "en")
            .expect("letter")
            .voice
            .expect("task");
        assert_eq!(voice.kind, "letter");
        assert_eq!(voice.speaker_name, "Marta Keller");
        game.messages.push(InboxMessage::new(
            "contract_concern_player-1_2026-03-20_3m".to_string(),
            String::new(),
            String::new(),
            String::new(),
            "2026-01-13".to_string(),
        ));
        game.director_voice = Some(DirectorVoice {
            date: "2026-01-13".to_string(),
            kind: voice.kind,
            speaker_name: voice.speaker_name,
            speaker_role: voice.speaker_role,
            player_id: voice.player_id,
            text: "Costa asked me again after training whether he is in the plans, and he is starting to look around.".to_string(),
            ..DirectorVoice::default()
        });
        publish_director_voice(&mut game);
        let message = &game.messages[0];
        assert_eq!(message.sender, "Marta Keller");
        assert_eq!(
            message.sender_role_key.as_deref(),
            Some("be.role.assistantManager")
        );
        assert_eq!(
            message.body_key.as_deref(),
            Some("be.msg.contractConcernVoice.body")
        );
        assert!(message.i18n_params.get("quote").unwrap().contains("Costa"));
    }

    /// Given a reply that puts a fee in the quote, when the voice is stored, then nothing is kept.
    #[test]
    fn a_quote_that_names_a_fee_is_dropped() {
        let mut game = user_game();
        played_yesterday(&mut game, 1, 1);
        let slate = morning_slate(&game, "en").expect("voice");
        remember_voice(&mut game, &slate, "We drew, and the price is 2 million.");
        assert!(game.director_voice.is_none());
    }
}
