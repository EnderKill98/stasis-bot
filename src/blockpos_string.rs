use anyhow::ensure;
use azalea::BlockPos;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{Display, Formatter};
use std::ops::{Deref, DerefMut};
use std::str::FromStr;

pub fn serialize<S: Serializer>(pos: &BlockPos, serializer: S) -> Result<S::Ok, S::Error> {
    format!("{}, {}, {}", pos.x, pos.y, pos.z).serialize(serializer)
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<BlockPos, D::Error> {
    Ok(BlockPosString::from_str(&String::deserialize(d)?).map_err(serde::de::Error::custom)?.into())
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct BlockPosString(pub BlockPos);

impl Deref for BlockPosString {
    type Target = BlockPos;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for BlockPosString {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Display for BlockPosString {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for BlockPosString {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.replace(" ", "");
        let parts: Vec<&str> = s.split(',').collect();

        ensure!(parts.len() == 3, "BlockPos has 3 numbers");
        Ok(BlockPos {
            x: parts[0].parse().map_err(anyhow::Error::new)?,
            y: parts[1].parse().map_err(anyhow::Error::new)?,
            z: parts[2].parse().map_err(anyhow::Error::new)?,
        }
        .into())
    }
}

impl From<BlockPos> for BlockPosString {
    fn from(b: BlockPos) -> Self {
        BlockPosString(b)
    }
}

impl From<BlockPosString> for BlockPos {
    fn from(b: BlockPosString) -> Self {
        b.0
    }
}

impl Serialize for BlockPosString {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        format!("{}, {}, {}", self.x, self.y, self.z).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for BlockPosString {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(BlockPosString::from_str(&String::deserialize(d)?).map_err(serde::de::Error::custom)?)
    }
}
