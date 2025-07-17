use crate::task::{Task, TaskOutcome};
use crate::{BotState, EXITCODE_PATHFINDER_DEADLOCKED};
use azalea::entity::Position;
use azalea::pathfinder::PathfinderClientExt;
use azalea::pathfinder::astar::PathfinderTimeout;
use azalea::pathfinder::goals::Goal;
use azalea::pathfinder::{GotoEvent, Pathfinder, StopPathfindingEvent, moves};
use azalea::{Client, Event, Vec3};
use std::fmt::{Debug, Display, Formatter};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

pub fn is_calculating(bot: &Client) -> bool {
    let mut ecs = bot.ecs.lock();
    if let Ok(pathfinder) = ecs.query::<&Pathfinder>().get_mut(&mut *ecs, bot.entity) {
        pathfinder.is_calculating
    } else {
        warn!("Failed to get Pathfinder for self!");
        false
    }
}

pub fn is_pathfinding(bot: &Client) -> bool {
    let mut ecs = bot.ecs.lock();
    if let Ok(pathfinder) = ecs.query::<&Pathfinder>().get_mut(&mut *ecs, bot.entity) {
        pathfinder.is_calculating || pathfinder.goal.is_some()
    } else {
        warn!("Failed to get Pathfinder for self!");
        false
    }
}

pub struct PathfindTask<G: Goal + Debug + Send + Sync + 'static> {
    allow_mining: bool,
    goal: Arc<G>,
    goal_name: String,
    last_is_calculating: bool,
    //started: bool,
    wait_for_pathfind_start: bool,
    last_position: Option<(Instant, Vec3)>,
}

impl<G: Goal + Debug + Send + Sync + 'static> PathfindTask<G> {
    pub fn new(allow_mining: bool, goal: G, goal_name: impl AsRef<str>) -> Self {
        Self {
            allow_mining,
            goal: Arc::new(goal),
            goal_name: goal_name.as_ref().to_string(),
            last_is_calculating: false,
            //started: false,
            wait_for_pathfind_start: false,
            last_position: None,
        }
    }
}

impl<G: Goal + Debug + Send + Sync + 'static> Display for PathfindTask<G> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.last_is_calculating {
            write!(f, "Pathfind (calculating path to {})", self.goal_name)
        } else {
            write!(f, "Pathfind (walking to {})", self.goal_name)
        }
    }
}

impl<G: Goal + Send + Sync + 'static> Task for PathfindTask<G> {
    fn start(&mut self, bot: Client, _bot_state: &BotState) -> anyhow::Result<()> {
        /*if self.started {
            bail!("Already started!");
        }*/
        if is_pathfinding(&bot) {
            bot.ecs.lock().send_event(StopPathfindingEvent {
                entity: bot.entity,
                force: true,
            });
            warn!("Tried to start pathfinding while doing so! Force aborted it current pathfinding operation!");
        }

        bot.ecs.lock().send_event(GotoEvent {
            entity: bot.entity,
            goal: self.goal.clone(),
            successors_fn: moves::default_move,
            allow_mining: self.allow_mining,
            min_timeout: PathfinderTimeout::Time(Duration::from_secs(1)),
            max_timeout: PathfinderTimeout::Time(Duration::from_secs(5)),
        });
        self.last_is_calculating = is_calculating(&bot);
        info!(
            "Pathfinding to \"{}\"{}...",
            self.goal_name,
            if self.last_is_calculating { " (calculating)" } else { "" },
        );
        self.last_position = None;
        self.wait_for_pathfind_start = true;
        //self.started = true;
        Ok(())
    }

    fn handle(&mut self, bot: Client, _bot_state: &BotState, _event: &Event) -> anyhow::Result<TaskOutcome> {
        if let Some(pathfinder) = bot.get_component::<Pathfinder>() {
            self.last_is_calculating = pathfinder.is_calculating;
            if !pathfinder.is_calculating && pathfinder.goal.is_none() {
                if self.wait_for_pathfind_start {
                    let block_pos = bot.component::<Position>().to_block_pos_floor();
                    if self.goal.success(block_pos) {
                        info!("Was already at {}", self.goal_name);
                        return Ok(TaskOutcome::Succeeded);
                    }
                    return Ok(TaskOutcome::Ongoing); // Waiting for event to actually being handled and pathfinding to start
                }
            } else {
                self.wait_for_pathfind_start = false; // Pathfinding started
            }

            // Deadlock tracking
            {
                let current_pos = Vec3::from(&bot.component::<Position>());
                let mut did_move = false;
                if let Some((last_pos_updated, last_pos)) = self.last_position {
                    did_move = current_pos.distance_squared_to(&last_pos) >= 0.2 * 0.2;
                    if !did_move && last_pos_updated.elapsed() > Duration::from_secs(30) {
                        warn!("Didn't meaningfully move at all over 30s. Pathfinding is likely stuck! Nothing left other than to kill this process :(");
                        std::process::exit(EXITCODE_PATHFINDER_DEADLOCKED);
                    }
                }
                if self.last_position.is_none() || did_move {
                    self.last_position = Some((Instant::now(), current_pos));
                }
            }

            if !pathfinder.is_calculating && pathfinder.goal.is_none() {
                // Done
                info!("Arrived at {}!", self.goal_name);
                return Ok(TaskOutcome::Succeeded);
            }
        }
        Ok(TaskOutcome::Ongoing)
    }

    fn stop(&mut self, bot: Client, _bot_state: &BotState) -> anyhow::Result<()> {
        bot.stop_pathfinding();
        Ok(())
    }
}
