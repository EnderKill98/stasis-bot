use anyhow::{Context, anyhow, bail};
use azalea::ResourceLocation;
use azalea::blocks::BlockState;
use azalea::blocks::properties::{NoteBlockNote, Sound};
use azalea::inventory::components::NoteBlockSound;
use std::collections::HashSet;
use std::io::{Cursor, Read};
use std::path::Path;

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
#[repr(u8)]
pub enum NbsInstrument {
    Harp = 0x00,
    Bass,
    Basedrum,
    Snare,
    Hat,
    Guitar,
    Flute,
    Bell,
    Chime,
    Xylophone,
    IronXylophone,
    CowBell,
    Didgeridoo,
    Bit,
    Banjo,
    Pling,
    Zombie,
    Skeleton,
    Creeper,
    Dragon,
    WitherSkeleton,
    Piglin,
    CustomHead,
}

const INSTRUMENTS: &[NbsInstrument] = &[
    NbsInstrument::Harp,
    NbsInstrument::Bass,
    NbsInstrument::Basedrum,
    NbsInstrument::Snare,
    NbsInstrument::Hat,
    NbsInstrument::Guitar,
    NbsInstrument::Flute,
    NbsInstrument::Bell,
    NbsInstrument::Chime,
    NbsInstrument::Xylophone,
    NbsInstrument::IronXylophone,
    NbsInstrument::CowBell,
    NbsInstrument::Didgeridoo,
    NbsInstrument::Bit,
    NbsInstrument::Banjo,
    NbsInstrument::Pling,
    NbsInstrument::Zombie,
    NbsInstrument::Skeleton,
    NbsInstrument::Creeper,
    NbsInstrument::Dragon,
    NbsInstrument::WitherSkeleton,
    NbsInstrument::Piglin,
    NbsInstrument::CustomHead,
];
impl NbsInstrument {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Harp => "harp",
            Self::Bass => "bass",
            Self::Basedrum => "basedrum",
            Self::Snare => "snare",
            Self::Hat => "hat",
            Self::Guitar => "guitar",
            Self::Flute => "flute",
            Self::Bell => "bell",
            Self::Chime => "chime",
            Self::Xylophone => "xylophone",
            Self::IronXylophone => "iron_xylophone",
            Self::CowBell => "cow_bell",
            Self::Didgeridoo => "didgeridoo",
            Self::Bit => "bit",
            Self::Banjo => "banjo",
            Self::Pling => "bling",
            Self::Zombie => "zombie",
            Self::Skeleton => "skeleton",
            Self::Creeper => "creeper",
            Self::Dragon => "dragon",
            Self::WitherSkeleton => "wither_skeleton",
            Self::Piglin => "piglin",
            Self::CustomHead => "custom_head",
        }
    }

    pub fn instrument_id(&self) -> ResourceLocation {
        ResourceLocation {
            namespace: String::from("minecraft"),
            path: String::from(self.name()),
        }
    }

    pub fn sound_id(&self) -> Option<ResourceLocation> {
        // These are also defined inside an azalea registry (SoundEvent),
        // but I'm too lazy to find out how to get the ID from them rn.
        if NbsInstrument::CustomHead == *self {
            return None; // Sound depends on another value
        }

        let name = self.name();
        Some(ResourceLocation {
            namespace: String::from("minecraft"),
            path: format!("block.note_block.{name}"),
        })
    }

    pub fn from_name(name: impl AsRef<str>) -> Option<Self> {
        let name = name.as_ref();
        INSTRUMENTS.iter().find(|instr| instr.name() == name).map(|instr| *instr)
    }

    pub fn from_instrument_id(id: &ResourceLocation) -> Option<Self> {
        if id.namespace != "minecraft" {
            return None;
        }
        INSTRUMENTS.iter().find(|instr| instr.name() == id.path).map(|instr| *instr)
    }

    pub fn from_sound_id(id: &ResourceLocation) -> Option<Self> {
        if id.namespace != "minecraft" || id.path.starts_with("block.note_block.") {
            return None;
        }
        INSTRUMENTS
            .iter()
            .find(|instr| instr.sound_id().map(|s| s.path == id.path).unwrap_or(false))
            .map(|instr| *instr)
    }

    pub fn is_instrument_block_below(&self) -> bool {
        match self {
            NbsInstrument::Zombie
            | NbsInstrument::Skeleton
            | NbsInstrument::Creeper
            | NbsInstrument::Dragon
            | NbsInstrument::WitherSkeleton
            | NbsInstrument::Piglin
            | NbsInstrument::CustomHead => false,
            _ => true,
        }
    }

    pub fn example_instrument_block_name(&self) -> &'static str {
        match self {
            NbsInstrument::Harp => "Air",
            NbsInstrument::Bass => "Oak Planks",
            NbsInstrument::Basedrum => "Stone",
            NbsInstrument::Snare => "Sand",
            NbsInstrument::Hat => "Glass",
            NbsInstrument::Guitar => "Wool",
            NbsInstrument::Flute => "Clay",
            NbsInstrument::Bell => "Gold",
            NbsInstrument::Chime => "Packed Ice",
            NbsInstrument::Xylophone => "Bone",
            NbsInstrument::IronXylophone => "Iron",
            NbsInstrument::CowBell => "Soul Sand",
            NbsInstrument::Didgeridoo => "Pumpkin",
            NbsInstrument::Bit => "Emerald",
            NbsInstrument::Banjo => "Hay",
            NbsInstrument::Pling => "Glowstone",
            NbsInstrument::Zombie => "Zombie Head",
            NbsInstrument::Skeleton => "Skeleton Head",
            NbsInstrument::Creeper => "Creeper Head",
            NbsInstrument::Dragon => "Dragon Head",
            NbsInstrument::WitherSkeleton => "Wither Skeleton Head",
            NbsInstrument::Piglin => "Piglin Head",
            NbsInstrument::CustomHead => "Custom Head",
        }
    }

    pub fn values() -> &'static [Self] {
        INSTRUMENTS
    }
}

impl From<NbsInstrument> for u8 {
    fn from(value: NbsInstrument) -> Self {
        value as u8
    }
}

impl TryFrom<u8> for NbsInstrument {
    type Error = anyhow::Error;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(*NbsInstrument::values()
            .iter()
            .find(|instr| u8::from(**instr) == value)
            .ok_or_else(|| anyhow!("Unknown or unsupported NbsInstrument index"))?)
    }
}

impl TryFrom<NoteBlockSound> for NbsInstrument {
    type Error = anyhow::Error;

    fn try_from(value: NoteBlockSound) -> Result<Self, Self::Error> {
        NbsInstrument::from_sound_id(&value.sound).ok_or_else(|| anyhow!("Unknown or unsupported instrument"))
    }
}

impl From<Sound> for NbsInstrument {
    fn from(value: Sound) -> Self {
        match value {
            Sound::Harp => NbsInstrument::Harp,
            Sound::Bass => NbsInstrument::Bass,
            Sound::Basedrum => NbsInstrument::Basedrum,
            Sound::Snare => NbsInstrument::Snare,
            Sound::Hat => NbsInstrument::Hat,
            Sound::Guitar => NbsInstrument::Guitar,
            Sound::Flute => NbsInstrument::Flute,
            Sound::Bell => NbsInstrument::Bell,
            Sound::Chime => NbsInstrument::Chime,
            Sound::Xylophone => NbsInstrument::Xylophone,
            Sound::IronXylophone => NbsInstrument::IronXylophone,
            Sound::CowBell => NbsInstrument::CowBell,
            Sound::Didgeridoo => NbsInstrument::Didgeridoo,
            Sound::Bit => NbsInstrument::Bit,
            Sound::Banjo => NbsInstrument::Banjo,
            Sound::Pling => NbsInstrument::Pling,
            Sound::Zombie => NbsInstrument::Zombie,
            Sound::Skeleton => NbsInstrument::Skeleton,
            Sound::Creeper => NbsInstrument::Creeper,
            Sound::Dragon => NbsInstrument::Dragon,
            Sound::WitherSkeleton => NbsInstrument::WitherSkeleton,
            Sound::Piglin => NbsInstrument::Piglin,
            Sound::CustomHead => NbsInstrument::CustomHead,
        }
    }
}

impl TryFrom<NbsInstrument> for NoteBlockSound {
    type Error = anyhow::Error;
    fn try_from(value: NbsInstrument) -> Result<Self, Self::Error> {
        value
            .sound_id()
            .map(|sound| NoteBlockSound { sound })
            .ok_or(anyhow!("Can't know sound event of instrument at this point."))
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
#[repr(u8)]
pub enum NbsPitch {
    N00Oct1FSharp = 0x00,
    N01Oct1G,
    N02Oct1GSharp,
    N03Oct1A,
    N04Oct1ASharp,
    N05Oct1B,
    N06Oct1C,
    N07Oct1CSharp,
    N08Oct1D,
    N09Oct1DSharp,
    N10Oct1E,
    N11Oct1F,
    N12Oct2FSharp,
    N13Oct2G,
    N14Oct2GSharp,
    N15Oct2A,
    N16Oct2ASharp,
    N17Oct2B,
    N18Oct2C,
    N19Oct2CSharp,
    N20Oct2D,
    N21Oct2DSharp,
    N22Oct2E,
    N23Oct2F,
    N24Oct3FSharp,
}

const PITCHES: &[NbsPitch] = &[
    NbsPitch::N00Oct1FSharp,
    NbsPitch::N01Oct1G,
    NbsPitch::N02Oct1GSharp,
    NbsPitch::N03Oct1A,
    NbsPitch::N04Oct1ASharp,
    NbsPitch::N05Oct1B,
    NbsPitch::N06Oct1C,
    NbsPitch::N07Oct1CSharp,
    NbsPitch::N08Oct1D,
    NbsPitch::N09Oct1DSharp,
    NbsPitch::N10Oct1E,
    NbsPitch::N11Oct1F,
    NbsPitch::N12Oct2FSharp,
    NbsPitch::N13Oct2G,
    NbsPitch::N14Oct2GSharp,
    NbsPitch::N15Oct2A,
    NbsPitch::N16Oct2ASharp,
    NbsPitch::N17Oct2B,
    NbsPitch::N18Oct2C,
    NbsPitch::N19Oct2CSharp,
    NbsPitch::N20Oct2D,
    NbsPitch::N21Oct2DSharp,
    NbsPitch::N22Oct2E,
    NbsPitch::N23Oct2F,
    NbsPitch::N24Oct3FSharp,
];

impl NbsPitch {
    pub fn ordinal(&self) -> u8 {
        u8::from(*self)
    }

    pub fn right_clicks_for(&self, desired: &NbsPitch) -> u8 {
        if desired.ordinal() < self.ordinal() {
            ((Self::values().len() - 1) as u8 - self.ordinal()) + self.ordinal()
        } else {
            desired.ordinal() - self.ordinal()
        }
    }

    pub fn next(&self) -> NbsPitch {
        let next_ordinal = (self.ordinal() + 1) % Self::values().len() as u8;
        NbsPitch::try_from(next_ordinal).expect("NbsPitch::try_from in NbsPitch::next should never fail.")
    }

    pub fn values() -> &'static [Self] {
        PITCHES
    }
}

impl From<NbsPitch> for u8 {
    fn from(value: NbsPitch) -> Self {
        value as u8
    }
}

impl TryFrom<u8> for NbsPitch {
    type Error = anyhow::Error;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(*NbsPitch::values()
            .iter()
            .find(|pitch| u8::from(**pitch) == value)
            .ok_or(anyhow!("Unknown pitch value (not in range of 0-24)"))?)
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct NbsNote {
    pub instrument: NbsInstrument,
    pub pitch: NbsPitch,
}

impl TryFrom<u16> for NbsNote {
    type Error = anyhow::Error;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Ok(Self {
            instrument: NbsInstrument::try_from((value << 8) as u8)?,
            pitch: NbsPitch::try_from((value & 0xFF) as u8)?,
        })
    }
}

impl TryFrom<BlockState> for NbsNote {
    type Error = anyhow::Error;

    fn try_from(state: BlockState) -> Result<Self, Self::Error> {
        if let Some(sound) = state.property::<Sound>()
            && let Some(note) = state.property::<NoteBlockNote>()
        {
            Ok(NbsNote {
                instrument: NbsInstrument::from(sound),
                pitch: NbsPitch::try_from(note as u8)?,
            })
        } else {
            Err(anyhow!("BlockState does not have properties Sound (Instrument) and Note (Pitch)"))
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct NbsPositionedNote {
    pub tick: u16,
    pub layer: u16,
    pub note: NbsNote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NbsSong {
    pub unique: HashSet<NbsNote>,
    pub notes: Box<[NbsPositionedNote]>,

    pub length_ticks: u16,
    pub height: u16,
    pub tempo: u16,
    pub loop_start_tick: u16,
    pub r#loop: u8,
    pub max_loop_count: u8,

    pub file_name: Option<String>,
    pub name: String,
    pub author: String,
    pub original_author: String,
    pub description: String,
}

impl NbsSong {
    pub fn millis_to_ticks(&self, millis: i64) -> f64 {
        // From NBS Format: The tempo of the song multiplied by 100 (for example, 1225 instead of 12.25). Measured in ticks per second.
        let song_speed = (self.tempo as f64 / 100.0) / 20.0; // 20 Ticks per second (temp / 100 = 20) would be 1x speed
        let one_milli_to_twenty_tick_fraction = 1.0 / 50.0;
        millis as f64 * one_milli_to_twenty_tick_fraction * song_speed
    }

    pub fn ticks_to_millis(&self, ticks: f64) -> f64 {
        // From NBS Format: The tempo of the song multiplied by 100 (for example, 1225 instead of 12.25). Measured in ticks per second.
        let song_speed = (self.tempo as f64 / 100.0) / 20.0; // 20 Ticks per second (temp / 100 = 20) would be 1x speed
        let one_milli_to_twenty_tick_fraction = 1.0 / 50.0;
        ticks / one_milli_to_twenty_tick_fraction / song_speed
    }

    pub fn length_in_seconds(&self) -> f64 {
        self.ticks_to_millis(self.length_ticks as f64) / 1000.0
    }

    pub fn friendly_name(&self) -> String {
        if !self.name.trim().is_empty() {
            String::from(self.name.trim())
        } else if let Some(file_name) = &self.file_name {
            file_name.clone()
        } else if !self.description.trim().is_empty() {
            self.description.replace("\r", "").replace("\n", "")
        } else {
            return "<Unknown Song!>".to_owned();
        }
    }

    pub async fn from_path_async(path: impl AsRef<Path>) -> Result<Self, anyhow::Error> {
        let ref path = path.as_ref();
        let file_name = path
            .file_name()
            .ok_or(anyhow!("Invalid file name"))?
            .to_str()
            .context("File name is not UTF-8")?;
        if !path.exists() || path.is_dir() {
            bail!("Path is missing or a directory!");
        }
        let contents = tokio::fs::read(path).await.context("Read file contents")?;
        Ok(NbsSong::from_reader(&mut Cursor::new(contents), Some(file_name)).context("Parse song")?)
    }

    pub fn from_reader(reader: &mut impl Read, file_name: Option<impl AsRef<str>>) -> Result<NbsSong, anyhow::Error> {
        fn read_int(reader: &mut impl Read) -> std::io::Result<u32> {
            let mut buf = [0u8; 4];
            reader.read_exact(&mut buf)?;
            Ok(u32::from_le_bytes(buf))
        }

        fn read_string(reader: &mut impl Read) -> anyhow::Result<String> {
            let mut bytes = vec![0u8; read_int(reader)? as usize];
            reader.read_exact(&mut bytes)?;
            Ok(String::from_utf8_lossy(&bytes).to_string())
        }

        fn read_short(reader: &mut impl Read) -> std::io::Result<u16> {
            let mut buf = [0u8; 2];
            reader.read_exact(&mut buf)?;
            Ok(u16::from_le_bytes(buf))
        }

        fn read_byte(reader: &mut impl Read) -> std::io::Result<u8> {
            let mut buf = [0u8; 1];
            reader.read_exact(&mut buf)?;
            Ok(buf[0])
        }

        fn skip(reader: &mut impl Read, bytes: usize) -> std::io::Result<()> {
            let mut buf = vec![0u8; bytes];
            reader.read_exact(&mut buf)?;
            Ok(())
        }

        let mut length_ticks = read_short(reader).context("Read length_ticks/new_format")?;
        let new_format = length_ticks == 0;

        if new_format {
            skip(reader, 1 /*Format Version*/ + 1 /*Vanilla Instrument Count*/).context("Skip fields 1")?;
            length_ticks = read_short(reader).context("Read new_format length_ticks")?;
        }

        let height = read_short(reader).context("Read Height")?;
        let name = read_string(reader).context("Read Name")?;
        let author = read_string(reader).context("Read Author")?;
        let original_author = read_string(reader).context("Read Original author")?;
        let description = read_string(reader).context("Read Description")?;
        let tempo = read_short(reader).context("Read Tempo")?;

        skip(reader, 1 /*Auto Saving*/ + 1 /*Auto Saving Duration*/ + 1 /*Time Signature*/ + 4 /*Minutes Spent*/ + 4 /*Left Clicks*/ + 4 /*Right Clicks*/ + 4 /*Blocks Added*/ + 4 /*Blocks Removed*/).context("Skip fields 2")?;
        let _ = read_string(reader).context("Skip import file name")?;

        let mut r#loop = 0;
        let mut max_loop_count = 0;
        let mut loop_start_tick = 0;

        if new_format {
            r#loop = read_byte(reader)?;
            max_loop_count = read_byte(reader)?;
            loop_start_tick = read_short(reader)?;
        }

        let mut unique: HashSet<NbsNote> = HashSet::new();
        let mut notes: Vec<NbsPositionedNote> = Vec::with_capacity(1024);

        let mut tick: i16 = -1;
        loop {
            let mut jumps = read_short(reader).context("Read jumps")?;
            if jumps == 0 {
                break;
            }

            tick = tick.overflowing_add(jumps as i16).0; // Some songs may do this on purpose
            let mut layer: i16 = -1;
            loop {
                jumps = read_short(reader).context("Read jumps (inside layer)")?;
                if jumps == 0 {
                    break;
                }
                layer += jumps as i16;

                let instrument = NbsInstrument::try_from(read_byte(reader).context("Read instrument id")?).context("Parse NbsInstrument")?;
                let pitch = NbsPitch::try_from(((read_byte(reader).context("Read note_id")? as i8) - 33).clamp(0, 24) as u8).context("Parse NbsPitch")?;

                if new_format {
                    skip(reader, 1 /*Velocity*/ + 1 /*Panning*/ + 2 /*Pitch*/)?;
                }

                let note = NbsNote { instrument, pitch };
                unique.insert(note);
                notes.push(NbsPositionedNote {
                    note,
                    layer: layer.max(0) as u16,
                    tick: tick.max(0) as u16,
                });
            }
        }

        Ok(NbsSong {
            unique,
            notes: notes.into_boxed_slice(),

            length_ticks,
            height,
            tempo,
            loop_start_tick,
            r#loop,
            max_loop_count,

            file_name: file_name.map(|s| s.as_ref().to_string()),
            name,
            author,
            original_author,
            description,
        })
    }
}
