use crate::util;
use crate::util::rotation_vec;
use anyhow::{Result, anyhow};
use azalea::core::aabb::AABB;
use azalea::core::game_type::GameMode;
use azalea::ecs::entity::Entity;
use azalea::ecs::prelude::With;
use azalea::entity::metadata::CustomName;
use azalea::entity::{EntityKind, LookDirection, Physics};
use azalea::world::MinecraftEntityId;
use azalea::{Client, GameProfileComponent, Vec3};

pub fn entity_by_id(bot: &mut Client, entity_id: i32) -> Option<Entity> {
    bot.entity_by::<With<EntityKind>, (&MinecraftEntityId,)>(|(id,): &(&MinecraftEntityId,)| id.0 == entity_id)
}

pub fn describe_by_id(bot: &mut Client, entity_id: i32) -> Result<String> {
    let entity = entity_by_id(bot, entity_id);
    if let Some(entity) = entity {
        describe(bot, entity)
    } else {
        Ok(format!("<Unknown entity {entity_id}>"))
    }
}

pub fn entity_aabb(bot: &mut Client, entity: Entity) -> AABB {
    bot.entity_component::<Physics>(entity).bounding_box
}

pub fn own_entity_interaction_range(bot: &Client) -> f64 {
    // Attribute technically exists, but is not stored anywhere afaik, yet.
    let is_creative = bot
        .tab_list()
        .get(&bot.uuid())
        .map(|entry| entry.gamemode == GameMode::Creative)
        .unwrap_or(false);

    let creative_mode_entity_range = if is_creative { 2.0 } else { 0.0 };
    3.0 + creative_mode_entity_range
}

pub fn can_interact_with_entity_box(own_eye_pos: Vec3, look_dir: LookDirection, entity_interaction_range: f64, entity_box: AABB) -> bool {
    // Assuming MC 1.20.5+ (below is really messy + I'm too lazy rn)
    intersection(
        entity_box,
        own_eye_pos,
        own_eye_pos
            + rotation_vec(look_dir)
                .normalize()
                .multiply(entity_interaction_range, entity_interaction_range, entity_interaction_range),
    )
    .is_some()
}

pub fn could_interact_with_entity_box(own_eye_pos: Vec3, entity_interaction_range: f64, entity_box: AABB, dist_to_edges: f64) -> bool {
    // Assuming MC 1.20.5+ (below is really messy + I'm too lazy rn)
    let best_look_dir = azalea::direction_looking_at(&own_eye_pos, &util::closest_aabb_pos_towards(own_eye_pos, entity_box, dist_to_edges));
    can_interact_with_entity_box(own_eye_pos, best_look_dir, entity_interaction_range, entity_box)
}

pub fn describe(bot: &mut Client, entity: Entity) -> Result<String> {
    let entity_id = bot.get_entity_component::<MinecraftEntityId>(entity).ok_or(anyhow!("No McId"))?.0;
    let kind = bot.get_entity_component::<EntityKind>(entity).ok_or(anyhow!("No EntityKind"))?;
    let kind = kind.0.to_string().split(":").collect::<Vec<_>>()[1].to_owned();

    if let Some(profile) = bot.get_entity_component::<GameProfileComponent>(entity) {
        return Ok(format!("{} (uuid: {}, id: {entity_id})", profile.name, profile.uuid));
    }

    Ok(match bot.get_entity_component::<CustomName>(entity) {
        Some(CustomName(Some(custom_name))) => {
            format!("{custom_name} (type: {kind}, id: {entity_id})")
        }
        _ => format!("{kind} ({entity_id})"),
    })
}

fn vec3_index(vec: Vec3, index: usize) -> f64 {
    match index {
        0 => vec.x,
        1 => vec.y,
        2 => vec.z,
        _ => panic!("Index for Vec3 out of bounds (0-2)"),
    }
}

/// https://alelievr.github.io/Modern-Rendering-Introduction/AABBIntersection/
pub fn intersection(aabb: AABB, ray_start: Vec3, ray_end: Vec3) -> Option<(Vec3, f64 /*Dist*/)> {
    let ray_dir = ray_end - ray_start;
    let mut tmin: f64 = 0.0; // Start with the minimum distance (can be -FLT_MAX for entire ray)
    let mut tmax: f64 = f64::MAX; // Maximum allowable distance for the ray (segment length or ∞)

    // Iterate over each axis (x, y, z)
    for i in 0..3usize {
        // If the ray is parallel to the slab (AABB plane pair)
        if vec3_index(ray_dir, i).abs() < f64::EPSILON {
            // If the origin is outside the slab, there's no intersection
            if vec3_index(ray_start, i) < vec3_index(aabb.min, i) || vec3_index(ray_start, i) > vec3_index(aabb.max, i) {
                return None;
            }
        } else {
            // Compute the intersection t-values for the near and far planes of the slab
            let ood: f64 = 1.0 / vec3_index(ray_dir, i);
            let mut t1 = (vec3_index(aabb.min, i) - vec3_index(ray_start, i)) * ood;
            let mut t2 = (vec3_index(aabb.max, i) - vec3_index(ray_start, i)) * ood;

            // Ensure t1 is the intersection with the near plane, and t2 with the far plane
            if t1 > t2 {
                (t1, t2) = (t2, t1); // Swap t1 and t2
            }

            // Update tmin and tmax to compute the intersection interval
            tmin = tmin.max(t1);
            tmax = tmax.min(t2);

            // If the interval becomes invalid, there is no intersection
            if tmin > tmax {
                return None;
            }
        }
    }

    // If we reach here, the ray intersects the AABB on all 3 axes
    let q = ray_start + ray_dir * tmin; // Compute the intersection point
    if tmin <= 1.0 {
        // tmin == 1 = One unit of ray_dist
        Some((q, tmin * ray_dir.length()))
    } else {
        None // Too far
    }
}

/*/// https://gamedev.stackexchange.com/a/146362 -> https://tavianator.com/2011/ray_box.html -> https://tavianator.com/2015/ray_box_nan.html#comment-52153
fn intersects(b: AABB, ray_start: Vec3, ray_end: Vec3) -> bool {
    let ray_dir_inv = Vec3::new(
        1.0 / (ray_end.x - ray_start.x),
        1.0 / (ray_end.y - ray_start.y),
        1.0 / (ray_end.z - ray_start.z),
    );
    let mut t1: f64 = (b.min.x - ray_start.x) * ray_dir_inv.x;
    let mut t2: f64 = (b.max.x - ray_start.x) * ray_dir_inv.x;

    let mut tmin: f64 = t1.min(t2);
    let mut tmax: f64 = t1.max(t2);

    // Y
    t1 = (b.min.y - ray_start.y) * ray_dir_inv.y;
    t2 = (b.max.y - ray_start.y) * ray_dir_inv.y;

    tmin = tmin.max(t1.min(t2));
    tmax = tmax.min(t1.max(t2));

    // Z
    t1 = (b.min.z - ray_start.z) * ray_dir_inv.z;
    t2 = (b.max.z - ray_start.z) * ray_dir_inv.z;

    tmin = tmin.max(t1.min(t2));
    tmax = tmax.min(t1.max(t2));

    tmax > tmin.max(0.0)
}*/
