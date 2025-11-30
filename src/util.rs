use anyhow::bail;
use azalea::core::aabb::AABB;
use azalea::core::direction::Direction;
use azalea::ecs::entity::Entity;
use azalea::entity::{EyeHeight, LookDirection, Pose, Position};
use azalea::inventory::ItemStack;
use azalea::inventory::components::MaxStackSize;
use azalea::inventory::item::MaxStackSizeExt;
use azalea::pathfinder::goals::{BlockPosGoal, Goal};
use azalea::protocol::packets::game::s_use_item_on::BlockHit;
use azalea::{BlockPos, Client, Vec3};

pub fn own_eye_pos(bot: &Client) -> Option<Vec3> {
    player_eye_pos(bot, bot.entity)
}

pub fn player_eye_pos(bot: &Client, entity: Entity) -> Option<Vec3> {
    let mut bot = bot.clone();
    let pos = bot.get_entity_component::<Position>(entity);
    let pose = bot.get_entity_component::<Pose>(entity);
    let eye_height = bot.get_entity_component::<EyeHeight>(entity);

    if let (Some(pos), Some(pose), Some(eye_height)) = (pos, pose, eye_height) {
        Some(calculate_player_eye_pos(&pos, &pose, &eye_height))
    } else {
        None
    }
}

pub fn calculate_player_eye_pos(pos: &Position, pose: &Pose, eye_height: &EyeHeight) -> Vec3 {
    let y_offset = match pose {
        Pose::FallFlying | Pose::Swimming | Pose::SpinAttack => 0.5f64,
        Pose::Sleeping => 0.25f64,
        Pose::Sneaking => f64::from(*eye_height) * 0.85,
        _ => f64::from(*eye_height),
    };
    Vec3::from(pos) + Vec3::new(0.0, y_offset, 0.0)
}

pub fn max_stack_size(stack: &ItemStack) -> i32 {
    if let Some(data) = stack.as_present() {
        if let Some(max_stack_size) = data.components.get::<MaxStackSize>() {
            max_stack_size.count
        } else {
            data.kind.max_stack_size()
        }
    } else {
        0
    }
}

pub fn aabb_from_blockpos(pos: &BlockPos) -> AABB {
    AABB {
        min: pos.to_vec3_floored(),
        max: pos.to_vec3_floored() + Vec3::new(1.0, 1.0, 1.0),
    }
}

pub fn squared_magnitude(aabb: AABB, pos: Vec3) -> f64 {
    let closest_x = (aabb.min.x - pos.x).max(pos.x - aabb.max.x).max(0.0);
    let closest_y = (aabb.min.y - pos.y).max(pos.y - aabb.max.y).max(0.0);
    let closest_z = (aabb.min.z - pos.z).max(pos.z - aabb.max.z).max(0.0);
    closest_x.powi(2) + closest_y.powi(2) + closest_z.powi(2)
}

/// Move to a position where we can reach the given block.
#[derive(Clone, Debug)]
pub struct InteractableGoal {
    pub pos: BlockPos,
}

impl InteractableGoal {
    pub fn new(pos: BlockPos) -> Self {
        Self { pos }
    }
}

/*
impl Debug for InteractableGoal {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "BetterReachBlockPosGoal {{ pos: {:?}, bot: azalea::Client(?) }}", self.pos)
    }
}
*/

/// Based on ReachBlockPosGoal, but:
/// - Does not do raycasting (so can reach obstructed blocks)
/// - Needs to assume a wide range of potions (full eyepos block) as
///   no more accurate position is provided here.
/// - Assumes an AC, so does not assume extra leeway
/// - Assumes default survival interact range (for blocks)
impl Goal for InteractableGoal {
    fn heuristic(&self, n: BlockPos) -> f32 {
        BlockPosGoal(self.pos).heuristic(n)
    }
    fn success(&self, n: BlockPos) -> bool {
        // only do the expensive check if we're close enough
        let max_pick_range = 6;
        //let actual_pick_range = 4.5;

        let distance = (self.pos - n).length_squared();
        if distance > max_pick_range * max_pick_range {
            return false;
        }

        let eye_box = aabb_from_blockpos(&n.up(1));
        let target_box = aabb_from_blockpos(&self.pos);

        // Attempt to calculate the worst/best magnitude
        // Essentially the distance as if the closest point of the target hitbox was measures, not against a single vec3
        // but against the worst point in a aabb that is the full block the player eye is occupying
        // So: Distance of Closest Point of Target AABB to Furthest Point of Eye AABB
        let closest_x_1 = (target_box.min.x - eye_box.min.x).max(eye_box.min.x - target_box.max.x).max(0.0);
        let closest_x_2 = (target_box.min.x - eye_box.max.x).max(eye_box.max.x - target_box.max.x).max(0.0);
        let chosen_x = closest_x_1.max(closest_x_2);

        let closest_y_1 = (target_box.min.y - eye_box.min.y).max(eye_box.min.y - target_box.max.y).max(0.0);
        let closest_y_2 = (target_box.min.y - eye_box.max.y).max(eye_box.max.y - target_box.max.y).max(0.0);
        let chosen_y = closest_y_1.max(closest_y_2);

        let closest_z_1 = (target_box.min.z - eye_box.min.z).max(eye_box.min.z - target_box.max.z).max(0.0);
        let closest_z_2 = (target_box.min.z - eye_box.max.z).max(eye_box.max.z - target_box.max.z).max(0.0);
        let chosen_z = closest_z_1.max(closest_z_2);

        let best_worst_magnitude = chosen_x.powi(2) + chosen_y.powi(2) + chosen_z.powi(2);

        // Without AC, can safely do 5.5
        //best_worst_magnitude <= 4.5f64.powi(2)
        best_worst_magnitude <= 4.5f64.powi(2)
        //best_worst_magnitude <= if crate::OPTS.grim { 4.5f64.powi(2) } else { 5.5f64.powi(2) } // Could cause issues for old configs
    }
}

pub fn closest_aabb_pos_towards(pos: Vec3, target_box: AABB, dist_to_edges: f64) -> Vec3 {
    let dist_x = ((target_box.max.x - target_box.min.x) / 2.0).min(dist_to_edges);
    let dist_y = ((target_box.max.y - target_box.min.y) / 2.0).min(dist_to_edges);
    let dist_z = ((target_box.max.z - target_box.min.z) / 2.0).min(dist_to_edges);

    // Usually correct, but sometimes not thanks to probably rounding errors
    let from_x = (target_box.min.x + dist_x).min(target_box.max.x - dist_x);
    let to_x = (target_box.min.x + dist_x).max(target_box.max.x - dist_x);
    let from_y = (target_box.min.y + dist_y).min(target_box.max.y - dist_y);
    let to_y = (target_box.min.y + dist_y).max(target_box.max.y - dist_y);
    let from_z = (target_box.min.z + dist_z).min(target_box.max.z - dist_z);
    let to_z = (target_box.min.z + dist_z).max(target_box.max.z - dist_z);

    Vec3::new(pos.x.max(from_x).min(to_x), pos.y.max(from_y).min(to_y), pos.z.max(from_z).min(to_z))
}

pub fn rotation_vec(look_dir: LookDirection) -> Vec3 {
    let pitch_rad: f64 = (look_dir.x_rot as f64).to_radians();
    let yaw_rad: f64 = -(look_dir.y_rot as f64).to_radians();
    let yaw_rad_cos: f64 = yaw_rad.cos();
    let yaw_rad_sin: f64 = yaw_rad.sin();
    let pitch_rad_cos: f64 = pitch_rad.cos();
    let pitch_rad_sin: f64 = pitch_rad.sin();
    Vec3::new(yaw_rad_sin * pitch_rad_cos, -pitch_rad_sin, yaw_rad_cos * pitch_rad_cos)
}

/// Prevent Grim Modulo360 flag (not fully yet)
/// Not sure which is better, yet. Forcing 0 to 360 or -180 to 180
pub fn fix_look_direction(look_direction: LookDirection) -> LookDirection {
    let mut yaw = look_direction.y_rot % 360.0;
    // Force -180 to 180:
    if yaw >= 180.0 {
        yaw -= 360.0;
    }
    // Force 0 to 360:
    /*if yaw < 0.0 {
        yaw += 360.0;
    }*/
    LookDirection::new(yaw, look_direction.x_rot)
}

pub fn fixed_look_direction(yaw: f32, pitch: f32) -> LookDirection {
    fix_look_direction(LookDirection::new(yaw, pitch))
}

/// I might change this, so one place for all
pub fn nice_blockhit_look(eye_pos: &Vec3, block_pos: &BlockPos) -> LookDirection {
    fix_look_direction(azalea::direction_looking_at(eye_pos, &block_pos.center()))
}

/// I might change this, so one place for all
pub fn nice_blockhit(eye_pos: &Vec3, block_pos: &BlockPos) -> anyhow::Result<(LookDirection, BlockHit)> {
    let look_dir = nice_blockhit_look(eye_pos, block_pos);
    let rot_vec = rotation_vec(look_dir);
    let rot_vec_norm = rotation_vec(look_dir).normalize();
    let block_aabb = aabb_from_blockpos(block_pos);

    // Prevent Grim 2.X's RotationPlace flag
    let mut hit_inside_block = None;
    for dist in 0..100 {
        let dist = (dist as f64) / 10.0;
        let maybe_inside_block = eye_pos + &rot_vec_norm.multiply(dist, dist, dist);
        if block_aabb.contains(&maybe_inside_block) {
            hit_inside_block = Some(maybe_inside_block);
            break;
        }
    }
    if hit_inside_block.is_none() {
        bail!("Failed to brute-force a good position inside target block. block_pos is likely too far away from eye_pos!")
    }
    let hit_inside_block = hit_inside_block.unwrap();

    // Prevent Grim 2.X's PositionPlace flag
    // Not exact minecraft, but good enough. Grim only cared about "hidden faces". Not being exact vanilla.
    let hit_side = Direction::nearest(rot_vec).opposite();

    Ok((
        look_dir,
        BlockHit {
            block_pos: *block_pos,
            direction: hit_side,
            location: hit_inside_block,
            inside: true,
            world_border: false,
        },
    ))
}
