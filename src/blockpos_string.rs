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

pub mod vec {
    use crate::blockpos_string::BlockPosString;
    use azalea::BlockPos;
    use serde::de::{SeqAccess, Visitor};
    use serde::ser::SerializeSeq;
    use serde::{Deserializer, Serializer};
    use std::fmt;
    use std::str::FromStr;

    pub fn serialize<S>(vec: &Vec<BlockPos>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(vec.len()))?;
        for pos in vec {
            seq.serialize_element(&BlockPosString::from(*pos))?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<BlockPos>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct VecVisitor;

        impl<'de> Visitor<'de> for VecVisitor {
            type Value = Vec<BlockPos>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a sequence of BlockPos strings")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut vec = Vec::new();
                while let Some(s) = seq.next_element::<String>()? {
                    // Deserialize each string into BlockPos using your existing custom deserializer
                    vec.push(BlockPosString::from_str(&s).map_err(serde::de::Error::custom)?.into());
                }
                Ok(vec)
            }
        }

        deserializer.deserialize_seq(VecVisitor)
    }
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
