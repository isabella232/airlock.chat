use crate::*;
use core::time::Duration;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::fmt::Display;

// The full game state
#[derive(PartialEq, Clone, Debug)]
pub struct GameState {
  pub status: GameStatus,
  pub settings: Settings,
  pub players: BTreeMap<UUID, Player>,
  pub bodies: Vec<DeadBody>,
}

#[derive(PartialEq, Clone, Debug)]
pub struct Settings {
  pub speed: f64,
  pub kill_distance: f64,
  pub task_distance: f64,
  pub voting_time: Duration,
}
impl Default for Settings {
  fn default() -> Self {
    Settings {
      speed: 2.0,
      task_distance: 32.0,
      kill_distance: 64.0,
      voting_time: Duration::from_secs(120),
    }
  }
}

impl Default for GameState {
  fn default() -> Self {
    Self::new()
  }
}

impl GameState {
  pub fn new() -> Self {
    GameState {
      status: GameStatus::Connecting,
      settings: Settings::default(),
      players: BTreeMap::new(),
      bodies: Vec::new(),
    }
  }

  pub fn simulate(&mut self, elapsed: Duration) -> bool {
    self.status.progress_time(elapsed);
    match &self.status {
      GameStatus::Lobby | GameStatus::Playing(PlayState::Night) => self.simulate_day(elapsed),
      GameStatus::Playing(PlayState::Day(day_state)) => {
        if self.is_day_over(day_state) {
          match day_state.determine_winner_of_election() {
            VoteTarget::Skip => { /* The crew have chosen a strange mercy */ }
            VoteTarget::Player { uuid } => {
              // Kill the lucky winner!
              if let Some(player) = self.players.get_mut(&uuid) {
                player.dead = true;
              }
            }
          }
          // Now it's night!
          self.status = GameStatus::Playing(PlayState::Night);
        }
      }
      GameStatus::Connecting | GameStatus::Disconnected | GameStatus::Won(_) => {
        // Nothing to simulate
      }
    }

    self.status.finished()
  }

  fn is_day_over(&self, day_state: &DayState) -> bool {
    // Day can end after a timer.
    if day_state.time_remaining <= Duration::from_secs(0) {
      return true;
    }
    // Or after all eligable players have recorded a vote.
    self
      .players
      .iter()
      .filter(|(_, p)| p.eligable_to_vote())
      .all(|(uuid, _)| day_state.votes.contains_key(uuid))
  }

  fn simulate_day(&mut self, elapsed: Duration) {
    // elapsed is the time, in milliseconds, that has passed since the
    // last time we simulated.
    // By making our simulations relative to the amount of time that's passed,
    // the game will progress the same regardless of the frame rate, which may
    // vary between 30fps and 144fps even if our performance is perfect!
    let time_steps_passed =
      (elapsed.as_nanos() as f64) / (Duration::from_millis(16).as_nanos() as f64);

    for (_, player) in self.players.iter_mut() {
      let Speed { dx, dy } = player.speed;
      player.position.x += dx * time_steps_passed;
      player.position.y += dy * time_steps_passed;
      // We don't handle inputs.q here because player position may be
      // out of sync, but we _super_ don't want to let life or death
      // get out of sync.
    }
  }

  fn win(&mut self, team: Team) {
    self.status = GameStatus::Won(team);
  }

  pub fn note_game_started(&mut self) -> Result<(), String> {
    if self.status != GameStatus::Lobby {
      return Err(format!(
        "Internal error: got a message to start a game when not in the lobby!? Game status: {:?}",
        self.status
      ));
    }
    self.status = GameStatus::Playing(PlayState::Night);
    let impostor_index = rand::thread_rng().gen_range(0, self.players.len());
    for (i, (_, player)) in self.players.iter_mut().enumerate() {
      if i == impostor_index {
        player.impostor = true;
      }
      player.tasks = (0..6)
        .map(|_| Task {
          finished: false,
          position: Position::random(),
        })
        .collect();
    }

    Ok(())
  }

  pub fn note_death(&mut self, body: DeadBody) -> Result<(), String> {
    for (_, player) in self.players.iter_mut() {
      if player.color == body.color {
        player.dead = true;
      }
    }
    self.bodies.push(body);
    self.check_for_impostor_win();
    Ok(())
  }

  fn check_for_impostor_win(&mut self) {
    let mut impostor_count = 0;
    let mut crew_count = 0;
    for (_, player) in self.players.iter() {
      if player.dead {
        continue;
      }
      if player.impostor {
        impostor_count += 1;
      } else {
        crew_count += 1;
      }
    }
    if impostor_count >= crew_count {
      self.win(Team::Impostors);
    }
  }

  pub fn note_finished_task(
    &mut self,
    player_uuid: UUID,
    finished: FinishedTask,
  ) -> Result<(), String> {
    if let Some(player) = self.players.get_mut(&player_uuid) {
      if let Some(task) = player.tasks.get_mut(finished.index) {
        task.finished = true;
      }
    }
    self.check_for_crew_win();
    Ok(())
  }

  fn check_for_crew_win(&mut self) {
    let all_crew_tasks_finished = self
      .players
      .iter()
      .filter(|(_, p)| !p.impostor)
      .all(|(_, p)| p.tasks.iter().all(|t| t.finished));
    if all_crew_tasks_finished {
      self.win(Team::Crew);
    }
  }

  pub fn handle_disconnection(&mut self, disconnected_player: UUID) {
    self.players.remove(&disconnected_player);
    // The game might be over, because we're out of players
    if self.players.is_empty() {
      self.status = GameStatus::Disconnected;
    }
    // The game might be over because the crew has won!
    self.check_for_crew_win();
    // The game might be over because the impostors have won D:
    self.check_for_impostor_win();
    // We might be voting, in which case we want to remove all votes for the
    // disconnected player, so that players can vote for someone else if they wish.
    if let GameStatus::Playing(PlayState::Day(day)) = &mut self.status {
      let mut voters_for_disonnected = Vec::new();
      for (voter, target) in day.votes.iter_mut() {
        if let VoteTarget::Player { uuid } = target {
          if *uuid == disconnected_player {
            voters_for_disonnected.push(*voter);
          }
        }
      }
      for voter in voters_for_disonnected {
        day.votes.remove(&voter);
      }
    }
  }
}

// This is not the proper way to handle dimensions...
pub const WIDTH: f64 = 1024.0;
pub const HEIGHT: f64 = 768.0;

// We don't use a real UUID impl because getting randomness in the browser
// is different than the server, and I got a compiler error about it.
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UUID {
  v: [u8; 16],
}

// Display UUIDs as hex
impl Display for UUID {
  fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
    for byte in self.v.iter() {
      write!(fmt, "{:x?}", byte)?;
    }
    Ok(())
  }
}

impl Debug for UUID {
  fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
    for byte in self.v.iter() {
      write!(fmt, "{:x?}", byte)?;
    }
    Ok(())
  }
}

impl UUID {
  pub fn random() -> UUID {
    UUID { v: rand::random() }
  }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Position {
  pub x: f64,
  pub y: f64,
}

impl Position {
  pub fn distance(self, other: Position) -> f64 {
    ((self.x - other.x).powi(2) + (self.y - other.y).powi(2))
      .sqrt()
      .abs()
  }

  fn random() -> Position {
    let mut rng = rand::thread_rng();
    Position {
      x: rng.gen_range(30.0, WIDTH - 30.0),
      y: rng.gen_range(30.0, HEIGHT - 30.0),
    }
  }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Speed {
  pub dx: f64,
  pub dy: f64,
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Serialize, Deserialize)]
pub enum Color {
  Red,
  Pink,
  Blue,
  Orange,
  White,
  Black,
  Green,
}

impl Color {
  pub fn all() -> &'static [Color] {
    &[
      Color::Red,
      Color::Pink,
      Color::Blue,
      Color::Orange,
      Color::White,
      Color::Black,
      Color::Green,
    ]
  }

  pub fn to_str(&self) -> &'static str {
    match self {
      Color::Red => "#ff0102",
      Color::Pink => "#ff69b4",
      Color::Blue => "#1601ff",
      Color::Orange => "#ffa502",
      Color::White => "#ffffff",
      Color::Black => "#000000",
      Color::Green => "#01ff02",
    }
  }

  pub fn random() -> Color {
    Color::all()[rand::thread_rng().gen_range(0, Color::all().len())]
  }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Task {
  pub position: Position,
  pub finished: bool,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Player {
  pub name: String,
  pub uuid: UUID,
  pub color: Color,
  pub position: Position,
  pub dead: bool,
  pub impostor: bool,
  pub tasks: Vec<Task>,
  pub speed: Speed,
}

impl Player {
  pub fn new(uuid: UUID, name: String, color: Color) -> Player {
    let starting_position_seed: f64 = rand::random();
    Player {
      name,
      uuid,
      color,
      dead: false,
      position: Position {
        x: 275.0 + (100.0 * (starting_position_seed * 2.0 * std::f64::consts::PI).sin()),
        y: 275.0 + (100.0 * (starting_position_seed * 2.0 * std::f64::consts::PI).cos()),
      },
      impostor: false,
      // 6 random tasks
      tasks: vec![],
      speed: Speed { dx: 0.0, dy: 0.0 },
    }
  }

  pub fn eligable_to_vote(&self) -> bool {
    !self.dead
  }
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq)]
pub struct DeadBody {
  pub color: Color,
  pub position: Position,
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum GameStatus {
  Connecting,
  Lobby,
  Playing(PlayState),
  Won(Team),
  Disconnected,
}

impl GameStatus {
  pub fn progress_time(&mut self, elapsed: Duration) {
    if let GameStatus::Playing(PlayState::Day(day_state)) = self {
      day_state.time_remaining = day_state
        .time_remaining
        .checked_sub(elapsed)
        .unwrap_or_else(|| Duration::from_secs(0));
    }
  }
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum PlayState {
  Night,
  Day(DayState),
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct DayState {
  pub votes: BTreeMap<UUID, VoteTarget>,
  pub time_remaining: Duration,
}

impl DayState {
  pub fn determine_winner_of_election(&self) -> VoteTarget {
    // Count the votes by the target.
    let mut vote_count: BTreeMap<VoteTarget, u16> = BTreeMap::new();
    for (_, target) in self.votes.iter() {
      *vote_count.entry(*target).or_insert(0) += 1;
    }
    // The winner is the one with the most votes!
    let mut targets_and_votes = vote_count.iter().collect::<Vec<_>>();
    targets_and_votes.sort_by_key(|(_target, count)| *count);
    if let Some((winner, winner_votes)) = targets_and_votes.get(0) {
      if let Some((_runner_up, runner_up_votes)) = targets_and_votes.get(1) {
        if runner_up_votes == winner_votes {
          // In case of a tie, skip
          return VoteTarget::Skip;
        }
      }
      return **winner;
    }
    // If no one voted, it's skip.
    VoteTarget::Skip
  }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord)]
pub enum VoteTarget {
  Player { uuid: UUID },
  Skip,
}

impl GameStatus {
  pub fn finished(&self) -> bool {
    match self {
      GameStatus::Connecting => false,
      GameStatus::Lobby => false,
      GameStatus::Playing(_) => false,
      GameStatus::Won(_) => true,
      GameStatus::Disconnected => true,
    }
  }
}

#[derive(Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum Team {
  Crew,
  Impostors,
}
