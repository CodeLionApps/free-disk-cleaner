//! What a cleanup will do and which warnings it needs. Thresholds come from settings only.

use crate::settings::Settings;
use fdc_core::safety::Deletable;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    /// Index into the caller's item list.
    pub index: usize,
    /// Bypass the Trash. Only possible for Safe path items (see `Deletable::direct_delete_token`).
    pub direct: bool,
    pub large: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub items: Vec<Planned>,
    pub estimated: u64,
    /// Total above the bulk threshold → summary screen with a clear warning.
    pub bulk: bool,
    /// Direct deletion or a CLI command: cannot be undone, warn regardless of size.
    pub irreversible: bool,
}

pub fn build(selected: &[(usize, &Deletable)], skip_trash: bool, settings: &Settings) -> Plan {
    let large_limit = settings.large_item_bytes();
    let mut plan = Plan::default();
    for &(index, item) in selected {
        let size = item.info().size;
        let direct = skip_trash && item.direct_delete_token().is_some();
        plan.irreversible |= direct || item.path().is_none();
        plan.estimated += size;
        plan.items.push(Planned {
            index,
            direct,
            large: size > large_limit,
        });
    }
    // Biggest first: what matters most is on top of the review list.
    plan.items.sort_by_key(|p| {
        std::cmp::Reverse(
            selected
                .iter()
                .find(|s| s.0 == p.index)
                .map_or(0, |s| s.1.info().size),
        )
    });
    plan.bulk = plan.estimated > settings.bulk_total_bytes();
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::tests::fixture_state;
    use fdc_core::safety::{CommandTarget, DeletableStatus, Target};

    fn plan_for(settings: &Settings, skip_trash: bool, pick: impl Fn(&Deletable) -> bool) -> Plan {
        let (_d, st) = fixture_state(settings.clone());
        let sel: Vec<(usize, &Deletable)> = st
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.as_deletable().map(|d| (i, d)))
            .filter(|(_, d)| pick(d))
            .collect();
        build(&sel, skip_trash, settings)
    }

    #[test]
    fn thresholds_come_from_settings() {
        let defaults = Settings::default();
        let p = plan_for(&defaults, false, |d| d.path().is_some());
        assert!(!p.bulk && p.items.iter().all(|i| !i.large) && !p.irreversible);

        // Fixture items are MBs; lower thresholds so they trip.
        let tiny = Settings {
            large_item_gb: 0.005,
            bulk_total_gb: 0.01,
            ..defaults
        };
        let p = plan_for(&tiny, false, |d| d.path().is_some());
        assert!(p.bulk && p.items.iter().any(|i| i.large) && p.items.iter().any(|i| !i.large));
        assert!(p.estimated > tiny.bulk_total_bytes());
    }

    #[test]
    fn direct_only_for_safe_paths_and_irreversible_warning() {
        let s = Settings::default();
        let p = plan_for(&s, true, |d| d.path().is_some());
        assert!(p.irreversible);
        let (_d, st) = fixture_state(s.clone());
        for pl in &p.items {
            let d = st.items[pl.index].as_deletable().unwrap();
            assert_eq!(pl.direct, d.status() == DeletableStatus::Safe);
        }
        // Trash only: reversible, unless a command item (Docker) is included.
        assert!(!plan_for(&s, false, |d| d.path().is_some()).irreversible);
        let docker = plan_for(&s, false, |d| {
            d.info().target == Target::Command(CommandTarget::DockerBuildCache)
        });
        assert!(docker.irreversible && docker.items.iter().all(|i| !i.direct));
    }
}
