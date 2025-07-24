use azalea::BlockPos;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub fn serialize<S>(pos: &BlockPos, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    format!("{}, {}, {}", pos.x, pos.y, pos.z).serialize(serializer)
}

pub fn deserialize<'de, D>(deserializer: D) -> Result<BlockPos, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?.replace(" ", "");
    let parts: Vec<&str> = s.split(',').collect();

    if parts.len() != 3 {
        return Err(serde::de::Error::custom("Invalid BlockPos format"));
    }

    Ok(BlockPos {
        x: parts[0].parse().map_err(serde::de::Error::custom)?,
        y: parts[1].parse().map_err(serde::de::Error::custom)?,
        z: parts[2].parse().map_err(serde::de::Error::custom)?,
    })
}
