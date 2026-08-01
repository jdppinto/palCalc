use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use crate::types::*;

// The only data files loaded for logic — breeding.json/game_data.json/
// extracted_data.json are stale or redundant (see PLAN.md, Data Files Summary).
const BREEDING_DATA: &str = include_str!("../../../data/breeding_data.json");
const SPECIAL_COMBOS: &str = include_str!("../../../data/special_combos.json");
const PASSIVES: &str = include_str!("../../../data/passive_skills_assignable.json");
const ICON_MAP: &str = include_str!("../../../data/icon_map.json");
// breeding_data.json carries placeholder names (name == tribe key) for a few
// pals its generator couldn't match; this overlay corrects the UI-visible ones.
const NAME_FIXES: &str = include_str!("../../../data/name_fixes.json");

pub struct GameData {
    pub pals: HashMap<TribeKey, PalInfo>,
    /// Child-eligible pals as (rank, order, key), sorted, for closest-rank search.
    pub(crate) eligible: Vec<(i32, i32, TribeKey)>,
    /// Canonical (lexicographically sorted) parent pair → possible children.
    /// Gender requirements are stored relative to the canonical order.
    pub(crate) specials: HashMap<(TribeKey, TribeKey), Vec<BreedOutcome>>,
    pub passives: HashMap<String, PassiveSkill>,
    /// Tribe key → icon filename. Icon presence ≈ "actually obtainable":
    /// all icon-less tribes are child_eligible=false boss/unreleased pals.
    pub icons: HashMap<TribeKey, String>,
    /// Lazily-built full combo table over interned species (used by planner).
    pub(crate) pairs: OnceLock<PairTable>,
}

/// Every unordered species pair's children, over u16-interned species keys.
/// Gender requirements are stored relative to (keys[i], keys[j]) with i <= j.
pub(crate) struct PairTable {
    pub keys: Vec<TribeKey>,
    pub idx: HashMap<TribeKey, u16>,
    pub children: HashMap<(u16, u16), Vec<(u16, Option<Gender>, Option<Gender>)>>,
}

impl GameData {
    pub(crate) fn pair_table(&self) -> &PairTable {
        self.pairs.get_or_init(|| {
            let mut keys: Vec<TribeKey> = self.pals.keys().cloned().collect();
            keys.sort();
            let idx: HashMap<TribeKey, u16> = keys
                .iter()
                .enumerate()
                .map(|(i, k)| (k.clone(), i as u16))
                .collect();
            let n = keys.len();
            let mut children = HashMap::with_capacity(n * (n + 1) / 2);
            for i in 0..n {
                for j in i..n {
                    let outs = self
                        .breed(&keys[i], &keys[j])
                        .expect("interned keys are valid");
                    children.insert(
                        (i as u16, j as u16),
                        outs.iter()
                            .map(|o| (idx[&o.child], o.gender_a, o.gender_b))
                            .collect(),
                    );
                }
            }
            PairTable { keys, idx, children }
        })
    }
}

impl GameData {
    pub fn load() -> Result<Self, serde_json::Error> {
        let mut pals: HashMap<TribeKey, PalInfo> = serde_json::from_str(BREEDING_DATA)?;

        let name_fixes: HashMap<TribeKey, String> = serde_json::from_str(NAME_FIXES)?;
        for (key, name) in name_fixes {
            if let Some(p) = pals.get_mut(&key) {
                p.name = name;
            }
        }

        let raw_specials: HashMap<TribeKey, Vec<SpecialComboRow>> =
            serde_json::from_str(SPECIAL_COMBOS)?;

        let special_children: HashSet<TribeKey> = raw_specials.keys().cloned().collect();

        let mut eligible: Vec<(i32, i32, TribeKey)> = pals
            .values()
            .filter(|p| p.child_eligible && !special_children.contains(&p.key))
            .map(|p| (p.rank, p.order, p.key.clone()))
            .collect();
        eligible.sort();
        assert!(!eligible.is_empty(), "no child-eligible pals in data");
        let mut specials: HashMap<(TribeKey, TribeKey), Vec<BreedOutcome>> = HashMap::new();
        for (child, rows) in &raw_specials {
            for row in rows {
                let (x, y, gx, gy) = if row.a <= row.b {
                    (&row.a, &row.b, parse_gender(&row.ga), parse_gender(&row.gb))
                } else {
                    (&row.b, &row.a, parse_gender(&row.gb), parse_gender(&row.ga))
                };
                specials
                    .entry((x.clone(), y.clone()))
                    .or_default()
                    .push(BreedOutcome {
                        child: child.clone(),
                        gender_a: gx,
                        gender_b: gy,
                    });
            }
        }

        let passives: HashMap<String, PassiveSkill> = serde_json::from_str(PASSIVES)?;
        let icons: HashMap<TribeKey, String> = serde_json::from_str(ICON_MAP)?;

        Ok(Self {
            pals,
            eligible,
            specials,
            passives,
            icons,
            pairs: OnceLock::new(),
        })
    }
}

fn parse_gender(s: &str) -> Option<Gender> {
    match s {
        "M" => Some(Gender::Male),
        "F" => Some(Gender::Female),
        _ => None,
    }
}
