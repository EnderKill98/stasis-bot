use azalea::ecs::entity::Entity;
use azalea::entity::{EyeHeight, Pose, Position};
use azalea::{Client, Vec3};

pub fn own_eye_pos(bot: &Client) -> Vec3 {
    player_eye_pos(bot, bot.entity)
}

pub fn player_eye_pos(bot: &Client, entity: Entity) -> Vec3 {
    let mut bot = bot.clone();
    let pos = bot.entity_component::<Position>(entity);
    let pose = bot.entity_component::<Pose>(entity);
    let eye_height = bot.entity_component::<EyeHeight>(entity);

    calculate_player_eye_pos(&pos, &pose, &eye_height)
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
