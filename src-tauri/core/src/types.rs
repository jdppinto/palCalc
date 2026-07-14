use serde::{Deserialize, Serialize};

/// Universal pal identifier, e.g. "SheepBall". Paldeck ids ("001") don't exist
/// for post-launch pals, so tribe keys are the only complete keying scheme.
pub type TribeKey = String;

#[derive(Debug, Clone, Deserialize)]
pub struct PalInfo {
    pub key: TribeKey,
    pub name: String,
    /// Current-generation CombiRank (30..=3080). NOT the launch-era rank scale.
    pub rank: i32,
    /// Tie-break priority for equal rank distance (lower wins).
    pub order: i32,
    #[serde(default)]
    pub zukan: i32,
    /// Whether this pal can appear as a rank-formula child.
    pub child_eligible: bool,
}

/// Row shape of special_combos.json: gender flags are "", "M" or "F".
#[derive(Debug, Clone, Deserialize)]
pub struct SpecialComboRow {
    pub a: TribeKey,
    pub b: TribeKey,
    #[serde(default)]
    pub ga: String,
    #[serde(default)]
    pub gb: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Gender {
    Male,
    Female,
}

/// One possible child of a parent pair. gender_a/gender_b are requirements on
/// the parents in the order the caller passed them (None = any gender).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BreedOutcome {
    pub child: TribeKey,
    pub gender_a: Option<Gender>,
    pub gender_b: Option<Gender>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PassiveSkill {
    pub name: String,
    #[serde(default)]
    pub rank: i32,
    #[serde(default)]
    pub description: String,
}
